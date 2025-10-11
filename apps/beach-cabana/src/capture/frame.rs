use std::time::SystemTime;

/// Basic frame representation that future encoders can consume.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Frame {
    pub timestamp: SystemTime,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum PixelFormat {
    Rgba8888,
    Bgra8888,
}
