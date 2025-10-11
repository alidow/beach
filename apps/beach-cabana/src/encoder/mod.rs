use crate::capture::Frame;
use anyhow::Result;
use gif::{Encoder, Frame as GifFrame, Repeat};
use std::fs::File;
use std::path::Path;

pub trait VideoEncoder {
    fn write_frame(&mut self, frame: &Frame) -> Result<()>;
    fn finish(self) -> Result<()>;
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
