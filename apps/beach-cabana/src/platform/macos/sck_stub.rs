use super::super::WindowApiError;
use std::path::PathBuf;
use std::time::Duration;

#[allow(dead_code)]
pub fn stream_window(
    target: &str,
    frames: u32,
    interval: Duration,
    output_dir: Option<PathBuf>,
) -> Result<(PathBuf, Vec<PathBuf>), WindowApiError> {
    let _ = (target, frames, interval, output_dir);
    Err(WindowApiError::CaptureFailed(
        "ScreenCaptureKit bridge not available (build without cabana_sck or implement the bridge)".into(),
    ))
}
