use super::VideoEncoder;
use crate::capture::{Frame, PixelFormat};
use anyhow::{anyhow, Context, Result};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation_sys::base::{kCFAllocatorDefault, CFAllocatorRef, CFRelease, CFTypeRef, OSStatus};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::string::CFStringRef;
use core_media::block_buffer::CMBlockBuffer;
use core_media::format_description::{CMVideoFormatDescription, kCMVideoCodecType_H264};
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_media::time::{CMTime, kCMTimeInvalid};
use core_video::pixel_buffer::{kCVPixelFormatType_32BGRA, CVPixelBuffer, CVPixelBufferRef};
use core_video::r#return::kCVReturnSuccess;
use std::ffi::c_void;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};
use crossbeam_channel::Sender;

const VT_ENCODE_INFO_FRAME_DROPPED: u32 = 0x0000_0001;
const START_CODE: &[u8] = &[0, 0, 0, 1];

type VTCompressionSessionRef = *mut c_void;
type VTEncodeInfoFlags = u32;

type VTCompressionOutputCallback = Option<
    unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        OSStatus,
        VTEncodeInfoFlags,
        CMSampleBufferRef,
    ),
>;

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTCompressionSessionCreate(
        allocator: CFAllocatorRef,
        width: i32,
        height: i32,
        codec_type: u32,
        encoder_specification: CFDictionaryRef,
        source_image_buffer_attributes: CFDictionaryRef,
        compressed_data_allocator: CFAllocatorRef,
        output_callback: VTCompressionOutputCallback,
        output_callback_ref_con: *mut c_void,
        compression_session_out: *mut VTCompressionSessionRef,
    ) -> OSStatus;

    fn VTCompressionSessionPrepareToEncodeFrames(session: VTCompressionSessionRef) -> OSStatus;

    fn VTCompressionSessionEncodeFrame(
        session: VTCompressionSessionRef,
        image_buffer: CVPixelBufferRef,
        presentation_time_stamp: CMTime,
        duration: CMTime,
        frame_properties: CFDictionaryRef,
        source_frame_ref_con: *mut c_void,
        info_flags_out: *mut VTEncodeInfoFlags,
    ) -> OSStatus;

    fn VTCompressionSessionCompleteFrames(session: VTCompressionSessionRef, complete_until: CMTime) -> OSStatus;

    fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);

    fn VTSessionSetProperty(
        session: VTCompressionSessionRef,
        property_key: CFStringRef,
        property_value: CFTypeRef,
    ) -> OSStatus;
}

pub struct VideoToolboxEncoder {
    session: VTCompressionSessionRef,
    sink: Arc<Mutex<EncoderSink>>,
    sink_refcon: *const Mutex<EncoderSink>,
    frame_index: i64,
    timescale: i32,
    frame_duration: CMTime,
    cleaned: bool,
}

struct EncoderSink {
    writer: BufWriter<File>,
    parameter_sets_written: bool,
    frames_encoded: u64,
    frames_dropped: u64,
    chunk_tx: Option<Sender<Vec<u8>>>,
}

impl VideoToolboxEncoder {
    pub fn new(path: &std::path::Path, width: u32, height: u32, fps: u32) -> Result<Self> {
        Self::new_with_chunks(Some(path), width, height, fps, None)
    }

    pub fn new_with_chunks(
        path: Option<&std::path::Path>,
        width: u32,
        height: u32,
        fps: u32,
        chunk_tx: Option<Sender<Vec<u8>>>,
    ) -> Result<Self> {
        if fps == 0 { return Err(anyhow!("fps must be greater than zero")); }

        let file = File::create(path.unwrap_or_else(|| std::path::Path::new("/dev/null")))?;
        let sink = Arc::new(Mutex::new(EncoderSink { writer: BufWriter::new(file), parameter_sets_written: false, frames_encoded: 0, frames_dropped: 0, chunk_tx }));
        let sink_refcon = Arc::into_raw(Arc::clone(&sink));

        let mut session: VTCompressionSessionRef = null_mut();
        let status = unsafe {
            VTCompressionSessionCreate(
                kCFAllocatorDefault,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264 as u32,
                std::ptr::null(),
                std::ptr::null(),
                kCFAllocatorDefault,
                Some(encode_output_callback),
                sink_refcon as *mut c_void,
                &mut session,
            )
        };
        if status != 0 { return Err(anyhow!("VTCompressionSessionCreate failed: {}", status)); }

        let timescale = 1000i32;
        let frame_duration = CMTime::make((timescale as i64) / (fps as i64), timescale);
        let mut enc = Self { session, sink, sink_refcon, frame_index: 0, timescale, frame_duration, cleaned: false };

        // Tune encoder for low-latency
        enc.set_bool("Realtime", true).ok();
        enc.set_bool("AllowFrameReordering", false).ok();
        enc.set_bool("H264EntropyMode", true).ok(); // CABAC if possible
        enc.set_num("MaxKeyFrameIntervalDuration", 2).ok();
        enc.set_num("ExpectedFrameRate", fps as i64).ok();
        enc.prepare()?;
        Ok(enc)
    }

    fn set_bool(&mut self, key: &str, value: bool) -> Result<()> {
        let k = CFString::new(key);
        let v = if value { CFBoolean::true_value() } else { CFBoolean::false_value() };
        let status = unsafe { VTSessionSetProperty(self.session, k.as_concrete_TypeRef(), v.as_CFTypeRef()) };
        if status != 0 { return Err(anyhow!("VTSessionSetProperty failed for {}: {}", key, status)); }
        Ok(())
    }

    fn set_num(&mut self, key: &str, value: i64) -> Result<()> {
        let k = CFString::new(key);
        let v = CFNumber::from(value);
        let status = unsafe { VTSessionSetProperty(self.session, k.as_concrete_TypeRef(), v.as_CFType().as_CFTypeRef()) };
        if status != 0 { return Err(anyhow!("VTSessionSetProperty failed for {}: {}", key, status)); }
        Ok(())
    }

    fn prepare(&mut self) -> Result<()> {
        let status = unsafe { VTCompressionSessionPrepareToEncodeFrames(self.session) };
        if status != 0 { return Err(anyhow!("VTCompressionSessionPrepareToEncodeFrames failed: {}", status)); }
        Ok(())
    }

    fn convert_to_pixel_buffer(&self, frame: &Frame) -> Result<CVPixelBuffer> {
        let width = i32::try_from(frame.width).context("width overflow")? as usize;
        let height = i32::try_from(frame.height).context("height overflow")? as usize;

        let pixel_buffer = CVPixelBuffer::new(frame.width as i32, frame.height as i32, kCVPixelFormatType_32BGRA)
            .map_err(|e| anyhow!("CVPixelBufferCreate failed: {:?}", e))?;

        if unsafe { pixel_buffer.lock_base_address(0) } != kCVReturnSuccess {
            return Err(anyhow!("CVPixelBufferLockBaseAddress returned non-success status"));
        }

        let base = unsafe { pixel_buffer.get_base_address() } as *mut u8;
        if base.is_null() {
            let _ = pixel_buffer.unlock_base_address(0);
            return Err(anyhow!("CVPixelBufferGetBaseAddress returned null"));
        }

        let dst_bytes_per_row = pixel_buffer.get_bytes_per_row();
        let src_stride = width * 4;

        match frame.pixel_format {
            PixelFormat::Bgra8888 => {
                for row in 0..height {
                    let src_start = row * src_stride;
                    let src_end = src_start + src_stride;
                    let src_row = &frame.data[src_start..src_end];
                    let dst_row = unsafe { std::slice::from_raw_parts_mut(base.add(row * dst_bytes_per_row), src_stride) };
                    dst_row.copy_from_slice(src_row);
                }
            }
            PixelFormat::Rgba8888 => {
                for row in 0..height {
                    let src_start = row * src_stride;
                    let src_end = src_start + src_stride;
                    let src_row = &frame.data[src_start..src_end];
                    let dst_row = unsafe { std::slice::from_raw_parts_mut(base.add(row * dst_bytes_per_row), src_stride) };
                    for (dst, chunk) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
                        dst[0] = chunk[2];
                        dst[1] = chunk[1];
                        dst[2] = chunk[0];
                        dst[3] = chunk[3];
                    }
                }
            }
        }

        let status = pixel_buffer.unlock_base_address(0);
        if status != kCVReturnSuccess { warn!(status, "CVPixelBufferUnlockBaseAddress returned non-success status"); }
        Ok(pixel_buffer)
    }

    fn cleanup(&mut self) {
        if self.session.is_null() { return; }

        unsafe { VTCompressionSessionInvalidate(self.session); CFRelease(self.session as CFTypeRef); }
        self.session = null_mut();

        if !self.sink_refcon.is_null() {
            unsafe { Arc::from_raw(self.sink_refcon); }
            self.sink_refcon = null();
        }

        self.cleaned = true;
    }
}

impl VideoEncoder for VideoToolboxEncoder {
    fn write_frame(&mut self, frame: &Frame) -> Result<()> {
        let pixel_buffer = self.convert_to_pixel_buffer(frame)?;
        let presentation_time = CMTime::make(self.frame_index, self.timescale);
        self.frame_index += 1;
        let mut info_flags: VTEncodeInfoFlags = 0;
        let status = unsafe { VTCompressionSessionEncodeFrame(self.session, pixel_buffer.as_concrete_TypeRef(), presentation_time, self.frame_duration, null(), null_mut(), &mut info_flags) };
        if status != 0 { return Err(anyhow!("VTCompressionSessionEncodeFrame failed with status {}", status)); }
        if info_flags & VT_ENCODE_INFO_FRAME_DROPPED != 0 { if let Ok(mut guard) = self.sink.lock() { guard.frames_dropped += 1; } }
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        let flush_time = unsafe { kCMTimeInvalid };
        let status = unsafe { VTCompressionSessionCompleteFrames(self.session, flush_time) };
        if status != 0 { warn!(status, "VTCompressionSessionCompleteFrames returned error status"); }
        unsafe { VTCompressionSessionInvalidate(self.session); CFRelease(self.session as CFTypeRef); }
        self.session = null_mut();
        if !self.sink_refcon.is_null() { unsafe { Arc::from_raw(self.sink_refcon); } self.sink_refcon = null(); }
        Ok(())
    }
}

unsafe extern "C" fn encode_output_callback(
    output_callback_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: OSStatus,
    info_flags: VTEncodeInfoFlags,
    sample_buffer: CMSampleBufferRef,
) {
    if status != 0 { warn!(status, "VideoToolbox encode callback returned error"); return; }
    let sink: &Mutex<EncoderSink> = &*(output_callback_ref_con as *const Mutex<EncoderSink>);
    let sb = CMSampleBuffer::wrap_under_get_rule(sample_buffer);
    if let Some(fd) = sb.format_description() { if let Ok(ps) = fd.parameter_set_at_index(0) { if let Ok(sps) = fd.parameter_set_at_index(1) { if let Ok(mut guard) = sink.lock() { if !guard.parameter_sets_written { let _ = guard.writer.write_all(START_CODE); let _ = guard.writer.write_all(ps.as_slice()); let _ = guard.writer.write_all(START_CODE); let _ = guard.writer.write_all(sps.as_slice()); guard.parameter_sets_written = true; } } } } }
    if let Some(data_buf) = sb.data_buffer() { if let Ok((data_ptr, data_len)) = data_buf.data_pointer() { let bytes = unsafe { std::slice::from_raw_parts(data_ptr as *const u8, data_len as usize) }; let chunks = annex_b_chunks(bytes); if let Ok(mut guard) = sink.lock() { for chunk in chunks { let _ = guard.writer.write_all(START_CODE); let _ = guard.writer.write_all(chunk); if let Some(tx) = guard.chunk_tx.as_ref() { let _ = tx.send(START_CODE.iter().copied().chain(chunk.iter().copied()).collect()); } guard.frames_encoded += 1; } } } }
}

fn annex_b_chunks(avcc: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= avcc.len() {
        let size = u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
        i += 4;
        if i + size > avcc.len() { break; }
        out.push(&avcc[i..i + size]);
        i += size;
    }
    out
}

