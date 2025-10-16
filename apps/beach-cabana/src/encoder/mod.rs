use crate::capture::Frame;
use anyhow::Result;
use gif::{Encoder, Frame as GifFrame, Repeat};
use std::fs::File;
use std::path::Path;

#[cfg(all(target_os = "macos", feature = "cabana_sck"))]
mod videotoolbox;
#[cfg(all(target_os = "macos", feature = "cabana_sck"))]
pub use videotoolbox::VideoToolboxEncoder;

pub trait VideoEncoder {
    fn write_frame(&mut self, frame: &Frame) -> Result<()>;
    fn finish(self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Frame, PixelFormat};
    use std::fs;
    use std::time::SystemTime;

    #[test]
    fn gif_encoder_writes_file() {
        let path = std::env::temp_dir().join(format!(
            "cabana-test-{}.gif",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        let mut encoder = GifVideoEncoder::new(&path, 2, 2, 5).expect("encoder");
        let frame = Frame {
            timestamp: SystemTime::now(),
            width: 2,
            height: 2,
            pixel_format: PixelFormat::Rgba8888,
            data: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255],
        };
        encoder.write_frame(&frame).expect("write");
        encoder.finish().expect("finish");

        let metadata = fs::metadata(&path).expect("metadata");
        assert!(metadata.len() > 0);
        let _ = fs::remove_file(path);
    }
}

pub struct GifVideoEncoder {
    encoder: Encoder<File>,
    delay_hundredths: u16,
}

impl GifVideoEncoder {
    pub fn new<P: AsRef<Path>>(path: P, width: u32, height: u32, fps: u32) -> Result<Self> {
        let width = u16::try_from(width).unwrap_or(u16::MAX);
        let height = u16::try_from(height).unwrap_or(u16::MAX);
        let file = File::create(path)?;
        let mut encoder = Encoder::new(file, width, height, &[])?;
        encoder.set_repeat(Repeat::Infinite)?;
        let delay = if fps == 0 { 10 } else { (100 / fps.max(1)) as u16 };
        Ok(Self {
            encoder,
            delay_hundredths: delay.max(1),
        })
    }
}

impl VideoEncoder for GifVideoEncoder {
    fn write_frame(&mut self, frame: &Frame) -> Result<()> {
        let mut rgba = frame.data.clone();
        let mut gif_frame = GifFrame::from_rgba_speed(frame.width as u16, frame.height as u16, &mut rgba, 10);
        gif_frame.delay = self.delay_hundredths;
        self.encoder.write_frame(&gif_frame)?;
        Ok(())
    }

    fn finish(self) -> Result<()> {
        let _ = self.encoder.into_inner()?;
        Ok(())
    }
}
