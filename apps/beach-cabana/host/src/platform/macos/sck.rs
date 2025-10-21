use crate::capture::{Frame, PixelFormat};
use crate::desktop::ScreenCaptureDescriptor;
use crate::platform::WindowApiError;
use core_foundation::base::{CFType, TCFType};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_media::time::CMTime;
use core_video::pixel_buffer::{CVPixelBuffer, kCVPixelFormatType_32BGRA};
use core_video::r#return::kCVReturnSuccess;
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use dispatch2::{Queue, QueueAttribute};
use image::{ImageBuffer, Rgba};
use objc2::{
    class, sel,
    declare_class, msg_send, msg_send_id, mutability,
    rc::Id,
    runtime::ProtocolObject,
    ClassType, DeclaredClass,
};
use objc2_foundation::{
    CGPoint, CGRect, CGSize, NSArray, NSData, NSError, NSObject, NSObjectProtocol,
    NSKeyedUnarchiver, NSUInteger,
};
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
use std::ffi::c_void;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::sync::Once;
use tracing::{debug, info, warn};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSApplicationLoad() -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSInitialize();
}

#[repr(C)]
#[derive(Debug, Default)]
struct ProcessSerialNumber {
    high: u32,
    low: u32,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn GetCurrentProcess(psn: *mut ProcessSerialNumber) -> i32;
    fn TransformProcessType(psn: *mut ProcessSerialNumber, transform_state: i32) -> i32;
}

const PROCESS_TRANSFORM_TO_FOREGROUND_APPLICATION: i32 = 1;

fn ensure_appkit_initialized() {
    static APPKIT_INIT: Once = Once::new();
    APPKIT_INIT.call_once(|| unsafe {
        info!("invoking CGSInitialize()");
        CGSInitialize();
        info!("CGSInitialize completed");
        let mut psn = ProcessSerialNumber::default();
        let status = GetCurrentProcess(&mut psn);
        info!(status, psn_high = psn.high, psn_low = psn.low, "GetCurrentProcess");
        let transform_status =
            TransformProcessType(&mut psn, PROCESS_TRANSFORM_TO_FOREGROUND_APPLICATION);
        info!(transform_status, "TransformProcessType to foreground");
        info!("invoking NSApplicationLoad()");
        if !NSApplicationLoad() {
            warn!("NSApplicationLoad returned false; screen capture may fail");
        } else {
            info!("NSApplicationLoad returned true");
        }
        let app: Id<NSObject> = msg_send_id![class!(NSApplication), sharedApplication];
        info!("obtained NSApplication.sharedApplication()");
        // Reduce AppKit expectations about a main menu by switching to Accessory policy
        // for our headless/CLI usage. This helps avoid NSMenu assertions when running
        // under test harnesses or without a proper app bundle.
        #[allow(non_camel_case_types)]
        type NSApplicationActivationPolicy = isize; // 0=Regular, 1=Accessory, 2=Prohibited
        let _: bool = msg_send![&app, setActivationPolicy: 1 as NSApplicationActivationPolicy];
        info!("set NSApplication activation policy to Accessory");
        let _: () = msg_send![&app, finishLaunching];
        info!("sent finishLaunching to NSApplication");
    });
}

#[derive(Debug)]
enum StreamEvent {
    Frame(Frame),
    Blank { status: Option<i64> },
    Error(String),
    Stopped,
}

#[derive(Debug)]
enum ProcessResult {
    Frame(Frame),
    Blank { status: Option<i64> },
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
        fn did_output_sample_buffer(&self, stream: &SCStream, sample_buffer: *mut CMSampleBufferRef, of_type: SCStreamOutputType) {
            let ivars = self.ivars();
            let sender = &ivars.sender;
            let sb = unsafe { CMSampleBuffer::wrap_under_create_rule(*sample_buffer) };
            match process_sample_buffer(&sb) {
                Ok(ProcessResult::Frame(frame)) => {
                    let _ = sender.send(StreamEvent::Frame(frame));
                }
                Ok(ProcessResult::Blank { status }) => {
                    let _ = sender.send(StreamEvent::Blank { status });
                }
                Err(err) => {
                    let _ = sender.send(StreamEvent::Error(err));
                }
            }
        }
    }

    unsafe impl SCStreamDelegate for StreamDelegate {
        #[method(stream:didStopWithError:)]
        fn did_stop_with_error(&self, _stream: &SCStream, error: Option<&NSError>) {
            let ivars = self.ivars();
            let sender = &ivars.sender;
            let message = error.map(|e| format!("{:?}", e)).unwrap_or_default();
            let _ = sender.send(StreamEvent::Error(message));
            let _ = sender.send(StreamEvent::Stopped);
        }
    }
);

pub struct SckStream {
    stream: Id<SCStream>,
    delegate: Id<StreamDelegate>,
    sender: Sender<StreamEvent>,
    receiver: Receiver<StreamEvent>,
}

impl SckStream {
    pub fn new(target: &str) -> Result<Self, WindowApiError> {
        ensure_appkit_initialized();

        let content = fetch_shareable_content()?;
        let selection = select_target(&content, target)?;
        Self::from_selection(selection)
    }

    pub fn from_descriptor(descriptor: &ScreenCaptureDescriptor) -> Result<Self, WindowApiError> {
        ensure_appkit_initialized();

        let filter = decode_filter(&descriptor.filter_blob)?;
        let configuration = match decode_stream_configuration(descriptor.stream_config_blob.as_ref())? {
            Some(cfg) => cfg,
            None => {
                let content = fetch_shareable_content()?;
                let selection = select_target(&content, &descriptor.target_id)?;
                selection.configuration
            }
        };

        let selection = TargetSelection { filter, configuration };
        Self::from_selection(selection)
    }

    fn from_selection(selection: TargetSelection) -> Result<Self, WindowApiError> {
        let (tx, rx) = unbounded::<StreamEvent>();
        let delegate =
            StreamDelegate::alloc().set_ivars(StreamDelegateIvars { sender: tx.clone() });
        let queue = Queue::create("beach.cabana.sck", QueueAttribute::Serial);

        let stream = SCStream::new_with_filter_configuration_and_delegate_queue(
            &selection.filter,
            &selection.configuration,
            Some(&*delegate),
            Some(&queue),
        )
        .map_err(|err| {
            WindowApiError::CaptureFailed(format!("failed to create SCStream: {:?}", err))
        })?;

        let output: ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*delegate);
        stream
            .add_stream_output_type_sample_handler_queue(
                &output,
                SCStreamOutputType::Screen,
                None,
                &queue,
            )
            .map_err(|err| {
                WindowApiError::CaptureFailed(format!(
                    "failed to add SCStreamOutput: {:?}",
                    err
                ))
            })?;

        Ok(Self {
            stream,
            delegate,
            sender: tx,
            receiver: rx,
        })
    }

    pub fn start(&self) -> Result<(), WindowApiError> {
        self
            .stream
            .start_capture()
            .map_err(|err| WindowApiError::CaptureFailed(format!("failed to start SCStream: {:?}", err)))
    }

    pub fn next_frame(&self, timeout: Duration) -> Result<Frame, WindowApiError> {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(WindowApiError::CaptureFailed("ScreenCaptureKit next_frame timeout".into()));
            }
            let remaining = deadline.saturating_duration_since(now);
            match self.receiver.recv_timeout(remaining) {
                Ok(StreamEvent::Frame(frame)) => return Ok(frame),
                Ok(StreamEvent::Blank { status }) => {
                    debug!(?status, "ScreenCaptureKit reported blank frame; awaiting next sample");
                }
                Ok(StreamEvent::Error(message)) => {
                    warn!(%message, "ScreenCaptureKit sample error");
                }
                Ok(StreamEvent::Stopped) => {
                    return Err(WindowApiError::CaptureFailed("ScreenCaptureKit stream stopped".into()))
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(WindowApiError::CaptureFailed("ScreenCaptureKit stream disconnected".into()))
                }
            }
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
        let _ = self.stream.remove_stream_output(output, SCStreamOutputType::Screen);
        let _ = self.sender.send(StreamEvent::Stopped);
    }
}

struct TargetSelection {
    filter: Id<SCContentFilter>,
    configuration: Id<SCStreamConfiguration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParsedTarget {
    Display(u32),
    Window(u32),
}

fn parse_target_identifier(raw: &str) -> Result<ParsedTarget, WindowApiError> {
    if let Some(rest) = raw.strip_prefix("display:") {
        let id_str = rest.rsplit(':').next().unwrap_or(rest);
        let id = id_str
            .parse::<u32>()
            .map_err(|_| WindowApiError::InvalidIdentifier(raw.to_string()))?;
        return Ok(ParsedTarget::Display(id));
    }
    if let Some(rest) = raw.strip_prefix("window:") {
        let id_str = rest.rsplit(':').next().unwrap_or(rest);
        let id = id_str
            .parse::<u32>()
            .map_err(|_| WindowApiError::InvalidIdentifier(raw.to_string()))?;
        return Ok(ParsedTarget::Window(id));
    }
    if let Some(rest) = raw.strip_prefix("application:") {
        return Err(WindowApiError::CaptureFailed(format!(
            "application target '{}' is not yet supported",
            rest
        )));
    }
    if let Ok(id) = raw.parse::<u32>() {
        return Ok(ParsedTarget::Window(id));
    }
    Err(WindowApiError::InvalidIdentifier(raw.to_string()))
}

fn decode_filter(bytes: &[u8]) -> Result<Id<SCContentFilter>, WindowApiError> {
    if bytes.is_empty() {
        return Err(WindowApiError::CaptureFailed(
            "ScreenCaptureKit descriptor missing filter data".into(),
        ));
    }
    unsafe {
        let data =
            NSData::dataWithBytes_length(bytes.as_ptr() as *mut c_void, bytes.len() as NSUInteger);
        NSKeyedUnarchiver::unarchivedObjectOfClass_fromData_error(
            SCContentFilter::class(),
            &data,
        )
        .map(|object| object.cast::<SCContentFilter>())
        .map_err(|err| {
            WindowApiError::CaptureFailed(format!(
                "failed to decode ScreenCaptureKit filter: {:?}",
                err
            ))
        })
    }
}

fn decode_stream_configuration(
    bytes: Option<&Vec<u8>>,
) -> Result<Option<Id<SCStreamConfiguration>>, WindowApiError> {
    let Some(blob) = bytes else {
        return Ok(None);
    };
    if blob.is_empty() {
        return Ok(None);
    }
    unsafe {
        let data =
            NSData::dataWithBytes_length(blob.as_ptr() as *mut c_void, blob.len() as NSUInteger);
        NSKeyedUnarchiver::unarchivedObjectOfClass_fromData_error(
            SCStreamConfiguration::class(),
            &data,
        )
        .map(|object| Some(object.cast::<SCStreamConfiguration>()))
        .map_err(|err| {
            WindowApiError::CaptureFailed(format!(
                "failed to decode ScreenCaptureKit stream configuration: {:?}",
                err
            ))
        })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn parse_display_identifiers() {
        assert_eq!(
            parse_target_identifier("display:123").unwrap(),
            ParsedTarget::Display(123)
        );
        assert_eq!(
            parse_target_identifier("display:external:456").unwrap(),
            ParsedTarget::Display(456)
        );
    }

    #[test]
    fn parse_window_identifiers() {
        assert_eq!(
            parse_target_identifier("window:42").unwrap(),
            ParsedTarget::Window(42)
        );
        assert_eq!(
            parse_target_identifier("window:com.apple.TextEdit:987").unwrap(),
            ParsedTarget::Window(987)
        );
        assert_eq!(
            parse_target_identifier("987").unwrap(),
            ParsedTarget::Window(987)
        );
    }

    #[test]
    fn reject_invalid_targets() {
        assert!(parse_target_identifier("display:abc").is_err());
        assert!(parse_target_identifier("foo").is_err());
    }
}

fn fetch_shareable_content() -> Result<Id<SCShareableContent>, WindowApiError> {
    let (tx, rx) = bounded::<Result<Id<SCShareableContent>, String>>(1);
    SCShareableContent::get_shareable_content_with_completion_closure(move |content, error| {
        let result = match content {
            Some(value) => Ok(value),
            None => Err(error.map(|err| format!("{:?}", err)).unwrap_or_else(|| "ScreenCaptureKit returned no content".into())),
        };
        let _ = tx.send(result);
    });

    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(content)) => Ok(content),
        Ok(Err(message)) => Err(WindowApiError::CaptureFailed(message)),
        Err(RecvTimeoutError::Timeout) => Err(WindowApiError::CaptureFailed("ScreenCaptureKit content enumeration timed out".into())),
        Err(RecvTimeoutError::Disconnected) => Err(WindowApiError::CaptureFailed("ScreenCaptureKit content enumeration channel closed".into())),
    }
}

fn select_target(content: &SCShareableContent, target: &str) -> Result<TargetSelection, WindowApiError> {
    match parse_target_identifier(target)? {
        ParsedTarget::Display(id) => {
            let displays = content.displays();
            for index in 0..displays.len() {
                if let Some(display) = displays.get(index) {
                    if display.display_id() == id {
                        return Ok(build_display_selection(display));
                    }
                }
            }
            Err(WindowApiError::InvalidIdentifier(format!(
                "display identifier {} not found",
                target
            )))
        }
        ParsedTarget::Window(id) => {
            let windows = content.windows();
            for index in 0..windows.len() {
                if let Some(window) = windows.get(index) {
                    if window.window_id() == id {
                        return Ok(build_window_selection(content, window));
                    }
                }
            }
            Err(WindowApiError::InvalidIdentifier(format!(
                "window identifier {} not found",
                target
            )))
        }
    }
}

fn build_window_selection(content: &SCShareableContent, window: &SCWindow) -> TargetSelection {
    let frame: CGRect = window.frame();
    debug!(
        window_id = window.window_id(),
        width = frame.size.width,
        height = frame.size.height,
        x = frame.origin.x,
        y = frame.origin.y,
        "building ScreenCaptureKit window selection"
    );
    if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
        warn!(
            window_id = window.window_id(),
            width = frame.size.width,
            height = frame.size.height,
            "window has non-positive dimensions; ScreenCaptureKit may fail"
        );
    }
    let displays = content.displays();
    let zero: i64 = 0;
    let config = SCStreamConfiguration::new();
    config.set_shows_cursor(true);
    config.set_captures_shadows(true);
    config.set_width(frame.size.width as i64);
    config.set_height(frame.size.height as i64);
    config.set_color_space_name(Some(unsafe { &*CFString::wrap_under_get_rule(kCGColorSpaceSRGB) }));
    config.set_background_color(Some(unsafe { &*CGColorCreateGenericRGB(0.0, 0.0, 0.0, 0.0) }));
    let filter = SCContentFilter::new_with_desktop_independent_window(&window, Some(&displays));
    TargetSelection { filter, configuration: config }
}

fn build_display_selection(display: &SCDisplay) -> TargetSelection {
    let frame: CGRect = display.frame();
    debug!(
        display_id = display.display_id(),
        width = frame.size.width,
        height = frame.size.height,
        x = frame.origin.x,
        y = frame.origin.y,
        "building ScreenCaptureKit display selection"
    );
    let config = SCStreamConfiguration::new();
    config.set_shows_cursor(true);
    config.set_captures_shadows(true);
    config.set_width(frame.size.width as i64);
    config.set_height(frame.size.height as i64);
    config.set_color_space_name(Some(unsafe { &*CFString::wrap_under_get_rule(kCGColorSpaceSRGB) }));
    config.set_background_color(Some(unsafe { &*CGColorCreateGenericRGB(0.0, 0.0, 0.0, 0.0) }));
    let filter = SCContentFilter::new_with_display(display);
    TargetSelection { filter, configuration: config }
}

fn process_sample_buffer(sb: &CMSampleBuffer) -> Result<ProcessResult, String> {
    if sb.num_samples() == 0 {
        return Ok(ProcessResult::Blank { status: None });
    }
    if let Some(status) = sb.output_decode_time_stamp().map(CMTime::value) {
        if status == 0 {
            return Ok(ProcessResult::Blank { status: Some(status) });
        }
    }
    let Some(att) = sb.format_description() else {
        return Err("missing format description".into());
    };
    let Some(img_desc) = att.as_video_format_description() else {
        return Err("missing video format description".into());
    };
    let Some(image_buffer) = sb.image_buffer() else { return Err("missing image buffer".into()); };
    let some_pb: CVPixelBuffer = image_buffer; // type alias
    let pb = some_pb;
    let (width, height) = (pb.width(), pb.height());
    if pb.pixel_format_type() != kCVPixelFormatType_32BGRA {
        return Err("unsupported pixel format".into());
    }
    if unsafe { CVPixelBufferLockBaseAddress(pb.as_concrete_TypeRef(), 0) } != kCVReturnSuccess {
        return Err("failed to lock pixel buffer".into());
    }
    let base = unsafe { CVPixelBufferGetBaseAddress(pb.as_concrete_TypeRef()) as *const u8 };
    let stride = pb.bytes_per_row() as usize;
    let len = stride * height as usize;
    let mut buf = vec![0u8; len];
    unsafe { std::ptr::copy_nonoverlapping(base, buf.as_mut_ptr(), len) };
    unsafe { CVPixelBufferUnlockBaseAddress(pb.as_concrete_TypeRef(), 0) };

    // Convert BGRA to RGBA
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        let row = &buf[y * stride..y * stride + (width as usize * 4)];
        for px in row.chunks_exact(4) {
            rgba.push(px[2]);
            rgba.push(px[1]);
            rgba.push(px[0]);
            rgba.push(px[3]);
        }
    }

    let frame = Frame {
        timestamp: SystemTime::now(),
        width,
        height,
        pixel_format: PixelFormat::Rgba8888,
        data: rgba,
    };
    Ok(ProcessResult::Frame(frame))
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGColorSpaceSRGB: *const CFString;
    fn CGColorCreateGenericRGB(r: f64, g: f64, b: f64, a: f64) -> *const CFType;
    fn CVPixelBufferGetBaseAddress(pb: core_video::pixel_buffer::CVPixelBufferRef) -> *const std::ffi::c_void;
}

pub fn stream_window(target: &str, frames: u32, interval: Duration, output_dir: Option<PathBuf>) -> Result<(PathBuf, Vec<PathBuf>), WindowApiError> {
    let stream = SckStream::new(target)?;
    stream.start()?;
    let base = if let Some(dir) = output_dir { dir } else {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        std::env::temp_dir().join(format!("beach-cabana-sck-stream-{}-{}", target.replace(':', "_"), ts))
    };
    fs::create_dir_all(&base).map_err(|e| WindowApiError::CaptureFailed(e.to_string()))?;

    let mut paths = Vec::new();
    for i in 0..frames {
        let frame = stream.next_frame(Duration::from_secs(2))?;
        let path = base.join(format!("frame_{:03}.png", i));
        write_frame_png(&frame, &path).map_err(|e| WindowApiError::CaptureFailed(e.to_string()))?;
        if i + 1 < frames { thread::sleep(interval); }
        paths.push(path);
    }
    Ok((base, paths))
}

fn write_frame_png(frame: &Frame, path: &Path) -> Result<(), String> {
    let Some(buf) = ImageBuffer::<Rgba<u8>, _>::from_vec(frame.width, frame.height, frame.data.clone()) else {
        return Err("failed to build RGBA buffer".into());
    };
    buf.save(path).map_err(|e| e.to_string())
}
