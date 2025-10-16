use super::VideoEncoder;
use crate::capture::{Frame, PixelFormat};
use anyhow::{anyhow, Context, Result};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation_sys::base::{kCFAllocatorDefault, CFAllocatorRef, CFRelease, CFTypeRef, OSStatus};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::string::CFStringRef;
use core_media::block_buffer::CMBlockBuffer;
use core_media::format_description::{CMFormatDescription, CMVideoFormatDescription, kCMVideoCodecType_H264};
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_media::time::{CMTime, kCMTimeInvalid};
use core_video::pixel_buffer::{kCVPixelFormatType_32BGRA, kCVReturnSuccess, CVPixelBuffer, CVPixelBufferRef};
use std::ffi::c_void;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

const VT_ENCODE_INFO_FRAME_DROPPED: u32 = 0x0000_0001;

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
extern "C" {
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
}

impl VideoToolboxEncoder {
    pub fn new(path: &std::path::Path, width: u32, height: u32, fps: u32) -> Result<Self> {
        if fps == 0 {
            return Err(anyhow!("fps must be greater than zero"));
        }

        let file = File::create(path)?;
        let sink = Arc::new(Mutex::new(EncoderSink {
            writer: BufWriter::new(file),
            parameter_sets_written: false,
            frames_encoded: 0,
            frames_dropped: 0,
        }));
        let sink_refcon = Arc::into_raw(Arc::clone(&sink));

        let mut session: VTCompressionSessionRef = null_mut();
        let status = unsafe {
            VTCompressionSessionCreate(
                kCFAllocatorDefault,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264,
                null(),
                null(),
                null(),
                Some(output_callback),
                sink_refcon as *mut c_void,
                &mut session,
            )
        };

        if status != 0 || session.is_null() {
            unsafe {
                Arc::from_raw(sink_refcon);
            }
            return Err(anyhow!(
                "VTCompressionSessionCreate failed with status {}",
                status
            ));
        }

        let mut encoder = Self {
            session,
            sink,
            sink_refcon,
            frame_index: 0,
            timescale: fps as i32,
            frame_duration: CMTime::make(1, fps as i32),
            cleaned: false,
        };

        encoder.configure_session(fps)?;

        let status = unsafe { VTCompressionSessionPrepareToEncodeFrames(session) };
        if status != 0 {
            encoder.cleanup();
            return Err(anyhow!(
                "VTCompressionSessionPrepareToEncodeFrames failed with status {}",
                status
            ));
        }

        Ok(encoder)
    }

    fn configure_session(&self, fps: u32) -> Result<()> {
        let key_realtime = CFString::from_static_string("RealTime");
        set_property_boolean(self.session, &key_realtime, true)?;

        let key_expected_fps = CFString::from_static_string("ExpectedFrameRate");
        set_property_number(self.session, &key_expected_fps, fps as f64)?;

        let bitrate = ((4_000_000u64 * fps as u64) / 30).max(1_000_000);
        let key_bitrate = CFString::from_static_string("AverageBitRate");
        set_property_number(self.session, &key_bitrate, bitrate as f64)?;

        let key_key_interval = CFString::from_static_string("MaxKeyFrameIntervalDuration");
        set_property_number(self.session, &key_key_interval, 2.0)?;

        let key_profile = CFString::from_static_string("ProfileLevel");
        let profile_value = if fps <= 30 {
            CFString::from_static_string("H264_Main_AutoLevel")
        } else if fps <= 60 {
            CFString::from_static_string("H264_High_AutoLevel")
        } else {
            CFString::from_static_string("H264_Baseline_AutoLevel")
        };
        set_property_cfstring(self.session, &key_profile, &profile_value)?;

        Ok(())
    }

    fn convert_to_pixel_buffer(&self, frame: &Frame) -> Result<CVPixelBuffer> {
        let width = frame.width as usize;
        let height = frame.height as usize;
        let pixel_buffer = CVPixelBuffer::new(kCVPixelFormatType_32BGRA, width, height, None)
            .map_err(|status| anyhow!("CVPixelBuffer::new failed: {}", status))?;

        let status = unsafe { pixel_buffer.lock_base_address(0) };
        if status != kCVReturnSuccess {
            return Err(anyhow!(
                "CVPixelBufferLockBaseAddress failed: {}",
                status
            ));
        }

        let base = pixel_buffer.get_base_address() as *mut u8;
        if base.is_null() {
            unsafe {
                pixel_buffer.unlock_base_address(0);
            }
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
                    let dst_row =
                        unsafe { std::slice::from_raw_parts_mut(base.add(row * dst_bytes_per_row), src_stride) };
                    dst_row.copy_from_slice(src_row);
                }
            }
            PixelFormat::Rgba8888 => {
                for row in 0..height {
                    let src_start = row * src_stride;
                    let src_end = src_start + src_stride;
                    let src_row = &frame.data[src_start..src_end];
                    let dst_row =
                        unsafe { std::slice::from_raw_parts_mut(base.add(row * dst_bytes_per_row), src_stride) };
                    for (dst, chunk) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
                        dst[0] = chunk[2];
                        dst[1] = chunk[1];
                        dst[2] = chunk[0];
                        dst[3] = chunk[3];
                    }
                }
            }
        }

        let status = unsafe { pixel_buffer.unlock_base_address(0) };
        if status != kCVReturnSuccess {
            warn!(
                status,
                "CVPixelBufferUnlockBaseAddress returned non-success status"
            );
        }

        Ok(pixel_buffer)
    }

    fn cleanup(&mut self) {
        if self.session.is_null() {
            return;
        }

        unsafe {
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session as CFTypeRef);
        }
        self.session = null_mut();

        if !self.sink_refcon.is_null() {
            unsafe {
                Arc::from_raw(self.sink_refcon);
            }
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
        let status = unsafe {
            VTCompressionSessionEncodeFrame(
                self.session,
                pixel_buffer.as_concrete_TypeRef(),
                presentation_time,
                self.frame_duration,
                null(),
                null_mut(),
                &mut info_flags,
            )
        };

        if status != 0 {
            return Err(anyhow!(
                "VTCompressionSessionEncodeFrame failed with status {}",
                status
            ));
        }

        if info_flags & VT_ENCODE_INFO_FRAME_DROPPED != 0 {
            if let Ok(mut guard) = self.sink.lock() {
                guard.frames_dropped += 1;
            }
        }

        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        let flush_time = unsafe { kCMTimeInvalid };
        let status = unsafe { VTCompressionSessionCompleteFrames(self.session, flush_time) };
        if status != 0 {
            warn!(
                status,
                "VTCompressionSessionCompleteFrames returned error status"
            );
        }

        unsafe {
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session as CFTypeRef);
        }
        self.session = null_mut();

        if !self.sink_refcon.is_null() {
            unsafe {
                Arc::from_raw(self.sink_refcon);
            }
            self.sink_refcon = null();
        }

        if let Ok(mut guard) = self.sink.lock() {
            guard.writer.flush()?;
            debug!(
                frames_encoded = guard.frames_encoded,
                frames_dropped = guard.frames_dropped,
                "VideoToolbox encoder finalized"
            );
        }

        self.cleaned = true;
        Ok(())
    }
}

impl Drop for VideoToolboxEncoder {
    fn drop(&mut self) {
        if !self.cleaned {
            self.cleanup();
        }
    }
}

unsafe extern "C" fn output_callback(
    output_callback_ref_con: *mut c_void,
    source_frame_ref_con: *mut c_void,
    status: OSStatus,
    info_flags: VTEncodeInfoFlags,
    sample_buffer_ref: CMSampleBufferRef,
) {
    let _ = source_frame_ref_con;
    let sink_arc = Arc::from_raw(output_callback_ref_con as *const Mutex<EncoderSink>);
    let sample = CMSampleBuffer::wrap_under_get_rule(sample_buffer_ref);
    let result = process_sample(status, info_flags, &sample, &sink_arc);
    Arc::into_raw(Arc::clone(&sink_arc));
    drop(sink_arc);

    if let Err(err) = result {
        warn!(error = %err, "VideoToolbox callback processing failed");
    }
}

fn process_sample(
    status: OSStatus,
    info_flags: VTEncodeInfoFlags,
    sample_buffer: &CMSampleBuffer,
    sink: &Arc<Mutex<EncoderSink>>,
) -> Result<()> {
    if status != 0 {
        warn!(status, "VideoToolbox reported non-zero status for sample");
    }
    if !sample_buffer.is_valid() || !sample_buffer.is_data_ready() {
        return Err(anyhow!("sample buffer not valid or ready"));
    }

    let block_buffer = sample_buffer
        .get_data_buffer()
        .ok_or_else(|| anyhow!("sample missing block buffer"))?;

    let format_desc = sample_buffer
        .get_format_description()
        .ok_or_else(|| anyhow!("sample missing format description"))?;
    let video_desc = format_desc
        .downcast::<CMVideoFormatDescription>()
        .ok_or_else(|| anyhow!("format description is not video"))?;

    let mut guard = sink
        .lock()
        .map_err(|_| anyhow!("VideoToolbox encoder sink poisoned"))?;

    if !guard.parameter_sets_written {
        write_parameter_sets(&mut guard.writer, &video_desc)
            .context("failed to write H264 parameter sets")?;
        guard.parameter_sets_written = true;
    }

    write_sample(&mut guard.writer, &block_buffer)?;
    guard.frames_encoded += 1;

    if info_flags & VT_ENCODE_INFO_FRAME_DROPPED != 0 {
        guard.frames_dropped += 1;
    }

    Ok(())
}

fn write_parameter_sets(
    writer: &mut BufWriter<File>,
    format_desc: &CMVideoFormatDescription,
) -> Result<()> {
    let mut index = 0;
    loop {
        let (parameter_set, total_sets, _) = format_desc
            .get_h264_parameter_set_at_index(index)
            .map_err(|status| anyhow!("parameter set fetch failed with status {}", status))?;

        if parameter_set.is_empty() {
            break;
        }

        writer.write_all(super::videotoolbox::START_CODE)?;
        writer.write_all(parameter_set)?;

        if index + 1 >= total_sets {
            break;
        }
        index += 1;
    }
    Ok(())
}

fn write_sample(writer: &mut BufWriter<File>, block_buffer: &CMBlockBuffer) -> Result<()> {
    let length = block_buffer.get_data_length();
    let mut offset = 0usize;
    while offset + 4 <= length {
        let mut size_bytes = [0u8; 4];
        block_buffer
            .copy_data_bytes(offset, &mut size_bytes)
            .map_err(|status| anyhow!("CMBlockBufferCopyDataBytes failed: {}", status))?;
        offset += 4;
        let nal_length = u32::from_be_bytes(size_bytes) as usize;
        if nal_length == 0 || offset + nal_length > length {
            break;
        }
        let mut nal = vec![0u8; nal_length];
        block_buffer
            .copy_data_bytes(offset, &mut nal)
            .map_err(|status| anyhow!("CMBlockBufferCopyDataBytes failed: {}", status))?;
        offset += nal_length;
        writer.write_all(super::videotoolbox::START_CODE)?;
        writer.write_all(&nal)?;
    }
    Ok(())
}

fn set_property_boolean(session: VTCompressionSessionRef, key: &CFString, value: bool) -> Result<()> {
    let cf_value = if value {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    set_property(session, key, cf_value.as_concrete_TypeRef() as CFTypeRef)
}

fn set_property_number(session: VTCompressionSessionRef, key: &CFString, value: f64) -> Result<()> {
    let number = CFNumber::from(value);
    set_property(session, key, number.as_concrete_TypeRef() as CFTypeRef)
}

fn set_property_cfstring(session: VTCompressionSessionRef, key: &CFString, value: &CFString) -> Result<()> {
    set_property(
        session,
        key,
        value.as_concrete_TypeRef() as CFTypeRef,
    )
}

fn set_property(session: VTCompressionSessionRef, key: &CFString, value: CFTypeRef) -> Result<()> {
    let status = unsafe { VTSessionSetProperty(session, key.as_concrete_TypeRef(), value) };
    if status != 0 {
        Err(anyhow!(
            "VTSessionSetProperty failed for {} (status {})",
            key.to_string(),
            status
        ))
    } else {
        Ok(())
    }
}

// Public constant for reuse inside module without making START_CODE pub in parent scope.
const START_CODE: &[u8] = &[0, 0, 0, 1];
