#[cfg(feature = "cabana_sck")]
pub fn stream_window(
    target: &str,
    frames: u32,
    interval: std::time::Duration,
    output_dir: Option<std::path::PathBuf>,
) -> Result<(std::path::PathBuf, Vec<std::path::PathBuf>), crate::platform::WindowApiError> {
    let _ = (target, frames, interval, output_dir);
    Err(crate::platform::WindowApiError::CaptureFailed(
        "ScreenCaptureKit streaming bridge not yet implemented".into(),
    ))
}
