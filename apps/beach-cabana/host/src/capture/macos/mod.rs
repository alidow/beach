use crate::capture::{BoxedProducer, Frame, FrameProducer, PixelFormat};
use crate::desktop::ScreenCaptureDescriptor;
use crate::platform::WindowApiError;
use std::time::{Duration, SystemTime};

#[cfg(feature = "cabana_sck")]
use tracing::{info, warn};

#[cfg(feature = "cabana_sck")]
use crate::platform::macos::sck;

/// Temporary CoreGraphics-based frame producer until ScreenCaptureKit lands.
pub struct CoreGraphicsProducer {
    target: String,
}

impl CoreGraphicsProducer {
    pub fn new(target: impl Into<String>) -> Self {
        Self { target: target.into() }
    }
}

impl FrameProducer for CoreGraphicsProducer {
    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn next_frame(&mut self) -> anyhow::Result<Frame> {
        let temp_dir = std::env::temp_dir().join(format!(
            "beach-cabana-cg-frame-{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let (dir, frames) = crate::platform::macos::stream_window_coregraphics(
            &self.target,
            1,
            Duration::from_millis(0),
            Some(temp_dir),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;

        let frame_path = frames
            .into_iter()
            .last()
            .ok_or_else(|| anyhow::anyhow!("no frame captured"))?;

        let png_bytes = std::fs::read(&frame_path)?;
        let image = image::load_from_memory(&png_bytes)?.into_rgba8();
        let (width, height) = image.dimensions();
        let data = image.into_raw();

        // Clean up temporary frame file and directory.
        let _ = std::fs::remove_file(&frame_path);
        let _ = std::fs::remove_dir_all(dir);

        Ok(Frame {
            timestamp: SystemTime::now(),
            width,
            height,
            pixel_format: PixelFormat::Rgba8888,
            data,
        })
    }

    fn stop(&mut self) {}
}

#[cfg(feature = "cabana_sck")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ActiveBackend {
    Uninitialized,
    ScreenCaptureKit,
    CoreGraphics,
}

#[cfg(feature = "cabana_sck")]
pub struct AdaptiveProducer {
    target: String,
    sck: Option<ScreenCaptureKitProducer>,
    attempted_sck: bool,
    core: CoreGraphicsProducer,
    active: ActiveBackend,
}

#[cfg(feature = "cabana_sck")]
impl AdaptiveProducer {
    fn new(target: String, sck: Option<ScreenCaptureKitProducer>) -> Self {
        let core = CoreGraphicsProducer::new(target.clone());
        Self {
            target,
            sck,
            attempted_sck: false,
            core,
            active: ActiveBackend::Uninitialized,
        }
    }

    fn promote_core(&mut self) -> anyhow::Result<()> {
        self.core.start()?;
        info!(
            capture.target = %self.target,
            capture.backend = "coregraphics",
            "capture backend activated"
        );
        self.active = ActiveBackend::CoreGraphics;
        Ok(())
    }

    fn handle_sck_failure(&mut self) {
        if let Some(sck) = self.sck.as_mut() {
            sck.stop();
        }
        self.sck = None;
        self.active = ActiveBackend::Uninitialized;
        self.attempted_sck = true;
    }
}

#[cfg(feature = "cabana_sck")]
impl FrameProducer for AdaptiveProducer {
    fn start(&mut self) -> anyhow::Result<()> {
        match self.active {
            ActiveBackend::ScreenCaptureKit | ActiveBackend::CoreGraphics => return Ok(()),
            ActiveBackend::Uninitialized => {}
        }

        if !self.attempted_sck {
            if let Some(sck) = self.sck.as_mut() {
                match sck.start() {
                    Ok(()) => {
                        self.attempted_sck = true;
                        self.active = ActiveBackend::ScreenCaptureKit;
                        info!(
                            capture.target = %self.target,
                            capture.backend = "screencapturekit",
                            "capture backend activated"
                        );
                        return Ok(());
                    }
                    Err(err) => {
                        if let Some(window_err) = err.downcast_ref::<WindowApiError>() {
                            match window_err {
                                WindowApiError::InvalidIdentifier(_) |
                                WindowApiError::EnumerationFailed(_) => {
                                    return Err(err);
                                }
                                _ => {}
                            }
                        }
                        let message = err.to_string();
                        warn!(
                            target = %self.target,
                            error = %message,
                            "ScreenCaptureKit start failed; using CoreGraphics fallback"
                        );
                        info!(
                            capture.target = %self.target,
                            capture.backend = "coregraphics",
                            capture.reason = %message,
                            "falling back to CoreGraphics capture"
                        );
                        self.handle_sck_failure();
                    }
                }
            }
        }

        self.promote_core()
    }

    fn next_frame(&mut self) -> anyhow::Result<Frame> {
        loop {
            match self.active {
                ActiveBackend::ScreenCaptureKit => {
                    let Some(sck) = self.sck.as_mut() else {
                        self.active = ActiveBackend::Uninitialized;
                        continue;
                    };
                    match sck.next_frame() {
                        Ok(frame) => return Ok(frame),
                        Err(err) => {
                            if let Some(window_err) = err.downcast_ref::<WindowApiError>() {
                                match window_err {
                                    WindowApiError::InvalidIdentifier(_) |
                                    WindowApiError::EnumerationFailed(_) => {
                                        return Err(err);
                                    }
                                    _ => {}
                                }
                            }
                            let message = err.to_string();
                            warn!(
                                target = %self.target,
                                error = %message,
                                "ScreenCaptureKit frame failed; switching to CoreGraphics fallback"
                            );
                            info!(
                                capture.target = %self.target,
                                capture.backend = "coregraphics",
                                capture.reason = %message,
                                "falling back to CoreGraphics capture"
                            );
                            self.handle_sck_failure();
                        }
                    }
                }
                ActiveBackend::CoreGraphics => return self.core.next_frame(),
                ActiveBackend::Uninitialized => {
                    self.start()?;
                }
            }
        }
    }

    fn stop(&mut self) {
        match self.active {
            ActiveBackend::ScreenCaptureKit => {
                if let Some(sck) = self.sck.as_mut() {
                    sck.stop();
                }
                if self.sck.is_some() {
                    self.attempted_sck = false;
                }
            }
            ActiveBackend::CoreGraphics => {
                self.core.stop();
            }
            ActiveBackend::Uninitialized => {}
        }
        self.active = ActiveBackend::Uninitialized;
    }
}

#[cfg(feature = "cabana_sck")]
pub struct ScreenCaptureKitProducer {
    descriptor: ScreenCaptureDescriptor,
    stream: Option<sck::SckStream>,
}

#[cfg(feature = "cabana_sck")]
impl ScreenCaptureKitProducer {
    pub fn new(descriptor: ScreenCaptureDescriptor) -> Result<Self, WindowApiError> {
        if !descriptor.has_filter() {
            return Err(WindowApiError::CaptureFailed(
                "ScreenCaptureKit descriptor missing serialized filter".into(),
            ));
        }
        Ok(Self {
            descriptor,
            stream: None,
        })
    }
}

#[cfg(feature = "cabana_sck")]
impl FrameProducer for ScreenCaptureKitProducer {
    fn start(&mut self) -> anyhow::Result<()> {
        if self.stream.is_none() {
            let mut stream = sck::SckStream::from_descriptor(&self.descriptor)?;
            stream.start()?;
            self.stream = Some(stream);
        }
        Ok(())
    }

    fn next_frame(&mut self) -> anyhow::Result<Frame> {
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("ScreenCaptureKit stream not started"))?;
        stream
            .next_frame(Duration::from_secs(2))
            .map_err(anyhow::Error::from)
    }

    fn stop(&mut self) {
        self.stream = None;
    }
}

#[cfg(not(feature = "cabana_sck"))]
#[allow(dead_code)]
pub struct ScreenCaptureKitProducer;

#[cfg(not(feature = "cabana_sck"))]
#[allow(dead_code)]
impl ScreenCaptureKitProducer {
    pub fn new(_descriptor: ScreenCaptureDescriptor) -> Result<Self, WindowApiError> {
        Err(WindowApiError::CaptureFailed(
            "ScreenCaptureKit producer not implemented".into(),
        ))
    }
}

#[cfg(not(feature = "cabana_sck"))]
impl FrameProducer for ScreenCaptureKitProducer {
    fn start(&mut self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "ScreenCaptureKit producer start not implemented"
        ))
    }

    fn next_frame(&mut self) -> anyhow::Result<Frame> {
        Err(anyhow::anyhow!(
            "ScreenCaptureKit producer next_frame not implemented"
        ))
    }

    fn stop(&mut self) {}
}

#[cfg(all(target_os = "macos", feature = "cabana_sck"))]
pub fn create_producer_from_descriptor(
    descriptor: &ScreenCaptureDescriptor,
) -> Result<BoxedProducer, WindowApiError> {
    let descriptor_clone = descriptor.clone();
    let target = descriptor_clone.target_id.clone();
    let mut sck = None;
    if descriptor_clone.has_filter() {
        match ScreenCaptureKitProducer::new(descriptor_clone.clone()) {
            Ok(producer) => {
                sck = Some(producer);
            }
            Err(err) => {
                warn!(
                    target = %descriptor_clone.target_id,
                    error = %err,
                    "ScreenCaptureKit descriptor rejected; defaulting to CoreGraphics"
                );
            }
        }
    }
    Ok(Box::new(AdaptiveProducer::new(target, sck)))
}

#[cfg(all(target_os = "macos", feature = "cabana_sck"))]
pub fn create_producer(target: impl Into<String>) -> Result<BoxedProducer, WindowApiError> {
    let descriptor = ScreenCaptureDescriptor::legacy(target.into());
    create_producer_from_descriptor(&descriptor)
}

#[cfg(all(target_os = "macos", not(feature = "cabana_sck")))]
pub fn create_producer_from_descriptor(
    descriptor: &ScreenCaptureDescriptor,
) -> Result<BoxedProducer, WindowApiError> {
    Ok(Box::new(CoreGraphicsProducer::new(descriptor.target_id.clone())))
}

#[cfg(all(target_os = "macos", not(feature = "cabana_sck")))]
pub fn create_producer(target: impl Into<String>) -> Result<BoxedProducer, WindowApiError> {
    Ok(Box::new(CoreGraphicsProducer::new(target.into())))
}
