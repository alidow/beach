use super::{WindowApiError, WindowInfo, WindowKind};
use core_foundation::base::TCFType;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::number::CFNumberGetTypeID;
use core_foundation_sys::string::CFStringGetTypeID;
use core_graphics::display::{CGDisplayCreateImage, CGGetActiveDisplayList, CGRectNull, CGDirectDisplayID};
use core_graphics::image::CGImage;
use core_graphics::window::{
    kCGNullWindowID, kCGWindowImageDefault, kCGWindowLayer, kCGWindowName, kCGWindowNumber,
    kCGWindowOwnerName, kCGWindowListExcludeDesktopElements, kCGWindowListOptionIncludingWindow,
    kCGWindowListOptionOnScreenOnly, CGWindowListCopyWindowInfo, CGWindowListCreateImage,
};
use foreign_types::ForeignType;
use image::{ImageBuffer, Rgba};
use std::env;
use std::fs;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(feature = "cabana_sck")]
pub mod sck;
#[cfg(not(feature = "cabana_sck"))]
mod sck {
    pub fn stream_window(
        target: &str,
        frames: u32,
        interval: std::time::Duration,
        output_dir: Option<std::path::PathBuf>,
    ) -> Result<(std::path::PathBuf, Vec<std::path::PathBuf>), super::WindowApiError> {
        super::sck_stub::stream_window(target, frames, interval, output_dir)
    }
}

#[cfg(not(feature = "cabana_sck"))]
mod sck_stub;

pub mod permissions {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ScreenRecordingStatus { Granted, Denied }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    pub fn status() -> ScreenRecordingStatus {
        let granted = unsafe { CGPreflightScreenCaptureAccess() };
        if granted { ScreenRecordingStatus::Granted } else { ScreenRecordingStatus::Denied }
    }

    pub fn request_access() -> bool {
        unsafe { CGRequestScreenCaptureAccess() }
    }
}

const MAX_DISPLAYS: usize = 16;

pub fn enumerate_windows() -> Result<Vec<WindowInfo>, WindowApiError> {
    let mut windows = collect_windows()?;
    windows.extend(collect_displays()?);
    Ok(windows)
}

pub fn preview_window(target: &str) -> Result<PathBuf, WindowApiError> {
    let image = capture_target(target)?;
    save_image(&image, target)
}

#[allow(dead_code)]
pub fn stream_window(
    target: &str,
    frames: u32,
    interval: Duration,
    output_dir: Option<PathBuf>,
) -> Result<(PathBuf, Vec<PathBuf>), WindowApiError> {
    #[cfg(feature = "cabana_sck")]
    {
        match sck::stream_window(target, frames, interval, output_dir.clone()) {
            Ok(result) => return Ok(result),
            Err(err) => {
                tracing::warn!(
                    target = target,
                    error = %err,
                    "ScreenCaptureKit stream failed; falling back to CoreGraphics"
                );
            }
        }
    }

    stream_window_coregraphics(target, frames, interval, output_dir)
}

#[allow(dead_code)]
pub(crate) fn stream_window_coregraphics(
    target: &str,
    frames: u32,
    interval: Duration,
    output_dir: Option<PathBuf>,
) -> Result<(PathBuf, Vec<PathBuf>), WindowApiError> {
    if frames == 0 {
        return Err(WindowApiError::CaptureFailed("frames must be greater than zero".into()));
    }

    let base_dir = if let Some(dir) = output_dir {
        dir
    } else {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let sanitized = target.replace(':', "_");
        env::temp_dir().join(format!("beach-cabana-stream-{}-{}", sanitized, timestamp))
    };
    fs::create_dir_all(&base_dir).map_err(|err| {
        WindowApiError::CaptureFailed(format!("failed to create output dir {}: {}", base_dir.display(), err))
    })?;

    let mut paths = Vec::with_capacity(frames as usize);
    for index in 0..frames {
        let image = capture_target(target)?;
        let frame_path = base_dir.join(format!("frame_{:03}.png", index));
        write_image(&image, &frame_path)?;
        paths.push(frame_path);

        if index + 1 < frames {
            thread::sleep(interval);
        }
    }

    Ok((base_dir, paths))
}

fn capture_target(target: &str) -> Result<CGImage, WindowApiError> {
    if let Some(display_id) = target.strip_prefix("display:") {
        let id = display_id
            .parse::<u32>()
            .map_err(|_| WindowApiError::InvalidIdentifier(target.to_string()))?;
        capture_display(id)
    } else {
        let window_id = target
            .parse::<u32>()
            .map_err(|_| WindowApiError::InvalidIdentifier(target.to_string()))?;
        capture_window(window_id)
    }
}

fn capture_display(display_id: u32) -> Result<CGImage, WindowApiError> {
    let image_ref = unsafe { CGDisplayCreateImage(display_id as CGDirectDisplayID) };
    if image_ref.is_null() {
        return Err(WindowApiError::CaptureFailed(format!(
            "CGDisplayCreateImage returned null for {}",
            display_id
        )));
    }
    Ok(unsafe { CGImage::from_ptr(image_ref) })
}

fn capture_window(window_id: u32) -> Result<CGImage, WindowApiError> {
    let rect = unsafe { CGRectNull };
    let image_ref = unsafe {
        CGWindowListCreateImage(
            rect,
            kCGWindowListOptionIncludingWindow,
            window_id,
            kCGWindowImageDefault,
        )
    };
    if image_ref.is_null() {
        return Err(WindowApiError::CaptureFailed(format!(
            "CGWindowListCreateImage returned null for window {}",
            window_id
        )));
    }
    Ok(unsafe { CGImage::from_ptr(image_ref) })
}

fn save_image(image: &CGImage, target: &str) -> Result<PathBuf, WindowApiError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let sanitized = target.replace(':', "_");
    let path = env::temp_dir().join(format!(
        "beach-cabana-preview-{}-{}.png",
        sanitized, timestamp
    ));
    write_image(image, &path)?;
    Ok(path)
}

fn write_image(image: &CGImage, path: &Path) -> Result<(), WindowApiError> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let bytes_per_row = image.bytes_per_row() as usize;

    let data = image.data();
    let slice = data.bytes();

    if bytes_per_row < width * 4 {
        return Err(WindowApiError::CaptureFailed(format!(
            "unexpected bytes_per_row {} for width {}",
            bytes_per_row, width
        )));
    }

    if slice.len() < bytes_per_row * height {
        return Err(WindowApiError::CaptureFailed(format!(
            "image buffer too small ({} < {})",
            slice.len(),
            bytes_per_row * height
        )));
    }

    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        let src_offset = y * bytes_per_row;
        let dst_offset = y * width * 4;
        for x in 0..width {
            let src = src_offset + x * 4;
            let dst = dst_offset + x * 4;
            // Source is BGRA, convert to RGBA.
            rgba[dst] = slice[src + 2];
            rgba[dst + 1] = slice[src + 1];
            rgba[dst + 2] = slice[src];
            rgba[dst + 3] = slice[src + 3];
        }
    }

    let buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(width as u32, height as u32, rgba).ok_or_else(|| {
            WindowApiError::CaptureFailed("failed to construct RGBA buffer".into())
        })?;

    buffer
        .save(path)
        .map_err(|err| WindowApiError::CaptureFailed(err.to_string()))?;
    Ok(())
}

fn collect_windows() -> Result<Vec<WindowInfo>, WindowApiError> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let raw_list = unsafe { CGWindowListCopyWindowInfo(options, kCGNullWindowID) };
    if raw_list.is_null() {
        tracing::warn!("CGWindowListCopyWindowInfo returned null; returning empty list");
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    let count = unsafe { CFArrayGetCount(raw_list as CFArrayRef) };
    for index in 0..count {
        let dict_ref = unsafe {
            CFArrayGetValueAtIndex(raw_list as CFArrayRef, index) as CFDictionaryRef
        };
        if dict_ref.is_null() {
            continue;
        }
        let window = CFDictionaryWrapper { dict: dict_ref };

        let layer = window
            .number(unsafe { kCGWindowLayer as CFTypeRef })
            .unwrap_or(0);
        if layer != 0 {
            continue;
        }

        let identifier = window
            .number(unsafe { kCGWindowNumber as CFTypeRef })
            .map(|n| n.to_string())
            .unwrap_or_default();
        if identifier.is_empty() {
            continue;
        }

        let application = window
            .string(unsafe { kCGWindowOwnerName as CFTypeRef })
            .unwrap_or_else(|| "Unknown App".to_string());
        let title = window
            .string(unsafe { kCGWindowName as CFTypeRef })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| application.clone());

        results.push(WindowInfo {
            identifier,
            title,
            application,
            kind: WindowKind::Window,
        });
    }

    unsafe {
        CFRelease(raw_list as CFTypeRef);
    }

    Ok(results)
}

fn collect_displays() -> Result<Vec<WindowInfo>, WindowApiError> {
    let mut ids: [CGDirectDisplayID; MAX_DISPLAYS] = [0; MAX_DISPLAYS];
    let mut count: u32 = 0;

    let status =
        unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if status != 0 {
        return Err(WindowApiError::EnumerationFailed(format!(
            "CGGetActiveDisplayList failed with status {}",
            status
        )));
    }

    let mut displays = Vec::with_capacity(count as usize);
    for id in ids.iter().copied().take(count as usize) {
        displays.push(WindowInfo {
            identifier: format!("display:{}", id),
            title: format!("Display {}", id),
            application: "Screen".to_string(),
            kind: WindowKind::Display,
        });
    }

    Ok(displays)
}

struct CFDictionaryWrapper {
    dict: CFDictionaryRef,
}

impl CFDictionaryWrapper {
    fn get(&self, key: CFTypeRef) -> Option<CFTypeRef> {
        let value = unsafe { CFDictionaryGetValue(self.dict, key as *const c_void) as CFTypeRef };
        if value.is_null() {
            None
        } else {
            Some(value)
        }
    }

    fn string(&self, key: CFTypeRef) -> Option<String> {
        let value = self.get(key)?;
        if unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
            return None;
        }
        Some(unsafe { CFString::wrap_under_get_rule(value as _) }.to_string())
    }

    fn number(&self, key: CFTypeRef) -> Option<i64> {
        let value = self.get(key)?;
        if unsafe { CFGetTypeID(value) } != unsafe { CFNumberGetTypeID() } {
            return None;
        }
        unsafe { CFNumber::wrap_under_get_rule(value as _) }.to_i64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_window_coregraphics_rejects_zero_frames() {
        let result = stream_window_coregraphics("0", 0, Duration::from_millis(0), None);
        assert!(matches!(result, Err(WindowApiError::CaptureFailed(_))));
    }
}
