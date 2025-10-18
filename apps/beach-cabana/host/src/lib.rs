// Beach Cabana host/engine library
// Exposes capture, platform, encoder, security, noise, webrtc, and fixture modules
// for reuse by CLI and native apps.

pub mod capture;
pub mod encoder;
pub mod fixture;
pub mod platform;
pub mod noise;
#[cfg(feature = "webrtc")]
pub mod webrtc;
pub mod security;
