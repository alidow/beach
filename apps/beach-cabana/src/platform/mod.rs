use serde::Serialize;
use thiserror::Error;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub mod macos;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Window,
    Display,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    pub identifier: String,
    pub title: String,
    pub application: String,
    pub kind: WindowKind,
}

impl WindowInfo {
    #[cfg(not(target_os = "macos"))]
    pub fn sample(identifier: &str, title: &str, application: &str, kind: WindowKind) -> Self {
        Self {
            identifier: identifier.to_string(),
            title: title.to_string(),
            application: application.to_string(),
            kind,
        }
    }
}

#[derive(Debug, Error)]
pub enum WindowApiError {
    #[error("window enumeration failed: {0}")]
    EnumerationFailed(String),
    #[allow(dead_code)]
    #[error("window preview is not yet implemented for this platform")]
    PreviewNotImplemented,
    #[error("invalid target identifier: {0}")]
    InvalidIdentifier(String),
    #[error("failed to capture preview: {0}")]
    CaptureFailed(String),
}

pub fn enumerate_windows() -> Result<Vec<WindowInfo>, WindowApiError> {
    #[cfg(target_os = "macos")]
    {
        return macos::enumerate_windows();
    }

    #[cfg(not(target_os = "macos"))]
    {
        tracing::warn!("enumerating windows is not implemented yet for this platform; returning placeholder entries");
        Ok(vec![
            WindowInfo::sample(
                "sample-window",
                "Sample Window (implement adapters)",
                "beach-cabana",
                WindowKind::Window,
            ),
            WindowInfo::sample(
                "sample-display",
                "Sample Display (implement adapters)",
                "beach-cabana",
                WindowKind::Display,
            ),
        ])
    }
}

pub fn preview_window(window_id: &str) -> Result<std::path::PathBuf, WindowApiError> {
    #[cfg(target_os = "macos")]
    {
        return macos::preview_window(window_id);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = window_id;
        tracing::warn!("preview is not implemented yet; skipping");
        Err(WindowApiError::PreviewNotImplemented)
    }
}
