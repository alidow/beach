//! Capture abstractions for Cabana.
//!
//! The goal is to converge both the legacy CoreGraphics snapshot pipeline and
//! the forthcoming ScreenCaptureKit bridge behind a single trait so higher
//! layers (CLI, future WebRTC encoder) can consume frames without knowing which
//! API produced them.

pub use frame::{Frame, PixelFormat};

mod frame;

#[cfg(target_os = "macos")]
pub trait FrameProducer {
    fn start(&mut self) -> anyhow::Result<()>;
    fn next_frame(&mut self) -> anyhow::Result<Frame>;
    fn stop(&mut self);
}

#[cfg(target_os = "macos")]
pub type BoxedProducer = Box<dyn FrameProducer + Send>;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::create_producer;
