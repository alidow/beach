use crate::capture::{Frame, PixelFormat};
use crate::platform::WindowApiError;
use core_foundation::base::TCFType;
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_media::time::CMTime;
use core_video::pixel_buffer::{CVPixelBuffer, kCVPixelFormatType_32BGRA};
use core_video::r#return::kCVReturnSuccess;
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use dispatch2::{Queue, QueueAttribute};
use image::{ImageBuffer, Rgba};
use objc2::{
    declare_class, msg_send_id, mutability,
    rc::Id,
    runtime::ProtocolObject,
    ClassType, DeclaredClass,
};
use objc2_foundation::{CGRect, CGSize, NSArray, NSError, NSObject, NSObjectProtocol};
use screen_capture_kit::{
    shareable_content::{SCDisplay, SCShareableContent, SCWindow},
    stream::{
        SCContentFilter, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
        SCStreamOutputType,
    },
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
enum StreamEvent {
    Frame(Frame),
    Error(String),
    Stopped,
}

struct StreamDelegateIvars {
    sender: Sender<StreamEvent>,
}

declare_class!(
    struct StreamDelegate;

    unsafe impl ClassType for StreamDelegate {
        type Super = NSObject;
        type Mutability = mutability::Mutable;
        const NAME: &'static str = "BeachCabanaStreamDelegate";
    }

    impl DeclaredClass for StreamDelegate {
        type Ivars = StreamDelegateIvars;
    }

    unsafe impl NSObjectProtocol for StreamDelegate {}

    unsafe impl SCStreamOutput for StreamDelegate {
        #[method(stream:didOutputSampleBuffer:ofType:)]
        unsafe fn stream_did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: CMSampleBufferRef,
            of_type: SCStreamOutputType,
        ) {
            if of_type != SCStreamOutputType::Screen {
                return;
            }
            let sender = self.ivars().sender.clone();
            if let Err(message) = process_sample_buffer(sender.clone(), sample_buffer) {
                let _ = sender.send(StreamEvent::Error(message));
            }
        }
    }

    unsafe impl SCStreamDelegate for StreamDelegate {
        #[method(stream:didStopWithError:)]
        unsafe fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            let sender = self.ivars().sender.clone();
            let _ = sender.send(StreamEvent::Error(format!("{:?}", error)));
        }
    }

);

impl StreamDelegate {
    fn with_sender(sender: Sender<StreamEvent>) -> Id<Self> {
        unsafe {
            let this = Self::alloc().set_ivars(StreamDelegateIvars { sender });
            msg_send_id![super(this), init]
        }
    }
}

pub struct SckStream {
    stream: Id<SCStream>,
    filter: Id<SCContentFilter>,
    configuration: Id<SCStreamConfiguration>,
    delegate: Id<StreamDelegate>,
    queue: Queue,
    receiver: Receiver<StreamEvent>,
    sender: Sender<StreamEvent>,
}

impl SckStream {
    pub fn new(target: &str) -> Result<Self, WindowApiError> {
        let content = fetch_shareable_content()?;
        let selection = select_target(&content, target)?;

        let (sender, receiver) = unbounded();
        let delegate = StreamDelegate::with_sender(sender.clone());
        let delegate_ref = ProtocolObject::from_ref(&*delegate);

        let stream = SCStream::init_with_filter(
            SCStream::alloc(),
            &selection.filter,
            &selection.configuration,
            delegate_ref,
        );

        let queue = Queue::new("com.beach.cabana.sck", QueueAttribute::Serial);
        let output = ProtocolObject::from_ref(&*delegate);
        stream
            .add_stream_output(output, SCStreamOutputType::Screen, &queue)
            .map_err(|err| WindowApiError::CaptureFailed(format!("add_stream_output failed: {:?}", err)))?;

        let (start_tx, start_rx) = bounded::<Option<String>>(1);
        stream.start_capture(move |error| {
            let payload = error.map(|err| format!("{:?}", err));
            let _ = start_tx.send(payload);
        });

        match start_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Some(message)) => {
                let _ = stream.stop_capture(|_| {});
                return Err(WindowApiError::CaptureFailed(format!(
                    "ScreenCaptureKit startCapture failed: {}",
                    message
                )));
            }
            Ok(None) => {}
            Err(RecvTimeoutError::Timeout) => {
                let _ = stream.stop_capture(|_| {});
                return Err(WindowApiError::CaptureFailed(
                    "ScreenCaptureKit startCapture timed out".into(),
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = stream.stop_capture(|_| {});
                return Err(WindowApiError::CaptureFailed(
                    "ScreenCaptureKit startCapture channel closed unexpectedly".into(),
                ));
            }
        }

        Ok(Self {
            stream,
            filter: selection.filter,
            configuration: selection.configuration,
            delegate,
            queue,
            receiver,
            sender,
        })
    }

    pub fn next_frame(&self, timeout: Duration) -> Result<Frame, WindowApiError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(StreamEvent::Frame(frame)) => Ok(frame),
            Ok(StreamEvent::Error(message)) => Err(WindowApiError::CaptureFailed(message)),
            Ok(StreamEvent::Stopped) => Err(WindowApiError::CaptureFailed(
                "ScreenCaptureKit stream stopped".into(),
            )),
            Err(RecvTimeoutError::Timeout) => Err(WindowApiError::CaptureFailed(
                "ScreenCaptureKit frame timeout".into(),
            )),
            Err(RecvTimeoutError::Disconnected) => Err(WindowApiError::CaptureFailed(
                "ScreenCaptureKit stream disconnected".into(),
            )),
        }
    }
}

impl Drop for SckStream {
    fn drop(&mut self) {
        let (stop_tx, stop_rx) = bounded::<Option<String>>(1);
        self.stream.stop_capture(move |error| {
            let payload = error.map(|err| format!("{:?}", err));
            let _ = stop_tx.send(payload);
        });
        let _ = stop_rx.recv_timeout(Duration::from_secs(2));

        let output = ProtocolObject::from_ref(&*self.delegate);
        let _ = self
            .stream
            .remove_stream_output(output, SCStreamOutputType::Screen);
        let _ = self.sender.send(StreamEvent::Stopped);
    }
}

struct TargetSelection {
    filter: Id<SCContentFilter>,
    configuration: Id<SCStreamConfiguration>,
}

fn fetch_shareable_content() -> Result<Id<SCShareableContent>, WindowApiError> {
    let (tx, rx) = bounded::<Result<Id<SCShareableContent>, String>>(1);
    SCShareableContent::get_shareable_content_with_completion_closure(move |content, error| {
        let result = match content {
            Some(value) => Ok(value),
            None => Err(error
                .map(|err| format!("{:?}", err))
                .unwrap_or_else(|| "ScreenCaptureKit returned no content".into())),
        };
        let _ = tx.send(result);
    });

    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(content)) => Ok(content),
        Ok(Err(message)) => Err(WindowApiError::CaptureFailed(message)),
        Err(RecvTimeoutError::Timeout) => Err(WindowApiError::CaptureFailed(
            "ScreenCaptureKit content enumeration timed out".into(),
        )),
        Err(RecvTimeoutError::Disconnected) => Err(WindowApiError::CaptureFailed(
            "ScreenCaptureKit content enumeration channel closed".into(),
        )),
    }
}

fn select_target(
    content: &SCShareableContent,
    target: &str,
) -> Result<TargetSelection, WindowApiError> {
    if let Some(display_str) = target.strip_prefix("display:") {
        let id: u32 = display_str
            .parse()
            .map_err(|_| WindowApiError::InvalidIdentifier(target.to_string()))?;
        let displays = content.displays();
        for index in 0..displays.len() {
            if let Some(display) = displays.get(index) {
                if display.display_id() == id {
                    return Ok(build_display_selection(display));
                }
            }
        }
        return Err(WindowApiError::InvalidIdentifier(format!(
            "display identifier {} not found",
            target
        )));
    }

    let window_id: u32 = target
        .parse()
        .map_err(|_| WindowApiError::InvalidIdentifier(target.to_string()))?;
    let windows = content.windows();
    for index in 0..windows.len() {
        if let Some(window) = windows.get(index) {
            if window.window_id() == window_id {
                return Ok(build_window_selection(window));
            }
        }
    }

    Err(WindowApiError::InvalidIdentifier(format!(
        "window identifier {} not found",
        target
    )))
}

fn build_window_selection(window: &SCWindow) -> TargetSelection {
    let filter =
        SCContentFilter::init_with_desktop_independent_window(SCContentFilter::alloc(), window);
    let frame: CGRect = window.frame();
    let configuration = make_configuration(frame.size);
    TargetSelection { filter, configuration }
}

fn build_display_selection(display: &SCDisplay) -> TargetSelection {
    let empty = NSArray::new();
    let filter =
        SCContentFilter::init_with_display_exclude_windows(SCContentFilter::alloc(), display, &empty);
    let configuration = make_configuration(CGSize::new(
        display.width() as f64,
        display.height() as f64,
    ));
    TargetSelection { filter, configuration }
}

fn make_configuration(size: CGSize) -> Id<SCStreamConfiguration> {
    let width = size.width.max(1.0).round() as usize;
    let height = size.height.max(1.0).round() as usize;

    let configuration = SCStreamConfiguration::new();
    configuration.set_width(width);
    configuration.set_height(height);
    configuration.set_pixel_format(kCVPixelFormatType_32BGRA);
    configuration.set_queue_depth(4);
    configuration.set_show_cursor(true);
    configuration.set_scales_to_fit(true);
    configuration.set_minimum_frame_interval(CMTime::make(1, 60));
    configuration
}

fn process_sample_buffer(
    sender: Sender<StreamEvent>,
    sample_buffer: CMSampleBufferRef,
) -> Result<(), String> {
    let sample = unsafe { CMSampleBuffer::wrap_under_get_rule(sample_buffer) };
    let Some(image_buffer) = sample.get_image_buffer() else {
        return Err("sample buffer missing image buffer".into());
    };
    let Some(pixel_buffer) = image_buffer.downcast::<CVPixelBuffer>() else {
        return Err("unsupported pixel buffer type".into());
    };
    let frame = convert_pixel_buffer(&pixel_buffer)?;
    sender
        .send(StreamEvent::Frame(frame))
        .map_err(|err| err.to_string())
}

fn convert_pixel_buffer(pixel_buffer: &CVPixelBuffer) -> Result<Frame, String> {
    if pixel_buffer.get_pixel_format() != kCVPixelFormatType_32BGRA {
        return Err(format!(
            "unexpected pixel format: {}",
            pixel_buffer.get_pixel_format()
        ));
    }
    let width = pixel_buffer.get_width();
    let height = pixel_buffer.get_height();
    if width == 0 || height == 0 {
        return Err("pixel buffer has empty dimensions".into());
    }

    let status = pixel_buffer.lock_base_address(0);
    if status != kCVReturnSuccess {
        return Err(format!("lock_base_address failed: {}", status));
    }

    let result = unsafe {
        let base = pixel_buffer.get_base_address() as *const u8;
        if base.is_null() {
            Err("pixel buffer base address is null".into())
        } else {
            let stride = width * 4;
            let bytes_per_row = pixel_buffer.get_bytes_per_row();
            let mut data = vec![0u8; stride * height];
            for row in 0..height {
                let src = base.add(row * bytes_per_row);
                let dst = data.as_mut_ptr().add(row * stride);
                ptr::copy_nonoverlapping(src, dst, stride);
            }
            Ok(Frame {
                timestamp: SystemTime::now(),
                width: width as u32,
                height: height as u32,
                pixel_format: PixelFormat::Bgra8888,
                data,
            })
        }
    };

    let _ = pixel_buffer.unlock_base_address(0);
    result
}

fn write_frame_png(frame: &Frame, path: &Path) -> Result<(), WindowApiError> {
    let mut rgba = frame.data.clone();
    for chunk in rgba.chunks_mut(4) {
        chunk.swap(0, 2);
    }
    let buffer = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_vec(frame.width, frame.height, rgba)
        .ok_or_else(|| WindowApiError::CaptureFailed("failed to prepare RGBA buffer".into()))?;
    buffer
        .save(path)
        .map_err(|err| WindowApiError::CaptureFailed(err.to_string()))
}

pub fn stream_window(
    target: &str,
    frames: u32,
    interval: Duration,
    output_dir: Option<PathBuf>,
) -> Result<(PathBuf, Vec<PathBuf>), WindowApiError> {
    if frames == 0 {
        return Err(WindowApiError::CaptureFailed(
            "frames must be greater than zero".into(),
        ));
    }

    let stream = SckStream::new(target)?;

    let base_dir = if let Some(dir) = output_dir {
        dir
    } else {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let sanitized = target.replace(':', "_");
        env::temp_dir().join(format!(
            "beach-cabana-sck-stream-{}-{}",
            sanitized, timestamp
        ))
    };

    fs::create_dir_all(&base_dir).map_err(|err| {
        WindowApiError::CaptureFailed(format!(
            "failed to create output dir {}: {}",
            base_dir.display(),
            err
        ))
    })?;

    let mut paths = Vec::with_capacity(frames as usize);
    for index in 0..frames {
        let frame = stream.next_frame(Duration::from_secs(2))?;
        let path = base_dir.join(format!("frame_{:03}.png", index));
        write_frame_png(&frame, &path)?;
        paths.push(path);

        if index + 1 < frames && !interval.is_zero() {
            thread::sleep(interval);
        }
    }

    Ok((base_dir, paths))
}
