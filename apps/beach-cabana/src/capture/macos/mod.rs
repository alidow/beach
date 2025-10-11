use crate::capture::{BoxedProducer, Frame, FrameProducer, PixelFormat};
use crate::platform::WindowApiError;
use std::time::{Duration, SystemTime};

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
        let (dir, frames) = crate::platform::macos::stream_window(
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

/// Placeholder for the real ScreenCaptureKit producer. Will be wired up once
/// the Swift/Obj-C bridge is implemented.
#[allow(dead_code)]
pub struct ScreenCaptureKitProducer;

#[allow(dead_code)]
impl ScreenCaptureKitProducer {
    pub fn new(_target: impl Into<String>) -> Result<Self, WindowApiError> {
        Err(WindowApiError::CaptureFailed(
            "ScreenCaptureKit producer not implemented".into(),
        ))
    }
}

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

#[cfg(target_os = "macos")]
pub fn create_producer(target: impl Into<String>) -> Result<BoxedProducer, WindowApiError> {
    let target = target.into();

    #[cfg(feature = "cabana_sck")]
    {
        if let Ok(producer) = ScreenCaptureKitProducer::new(target.clone()) {
            return Ok(Box::new(producer));
        }
    }

    Ok(Box::new(CoreGraphicsProducer::new(target)))
}
