# Beach Cabana – Phase 2 Notes (Capture & Encoding Adapters)

Deliverables accomplished:

- Expanded `capture/` module with resize/downscale helpers and a `FrameProducer` trait consumed by both streaming and encoding flows.
- Implemented `GifVideoEncoder` as the initial software fallback plus `encode` CLI command (animated GIF output, downscaling via `--max-width`).
- Hooked CLI streaming/encoding commands into the capture abstraction, so future ScreenCaptureKit and hardware encoders can slot in without touching CLI surfaces.
- Feature-gated ScreenCaptureKit bridge remains planned via `cabana_sck` following the spike instructions (`docs/beach-cabana/screencapturekit-spike.md`).

Next steps:

1. Replace CoreGraphics fallback with ScreenCaptureKit streaming on macOS when the bridge is ready.
2. Integrate VideoToolbox / hardware encoders behind the `VideoEncoder` trait (reuse GIF fallback as debug option).
3. Add automated capture/encode tests once `cabana_sck` is available in CI (macOS). 

## 2025-10-15 – ScreenCaptureKit Integration Status

- **Display capture ✅**: The new Rust bridge (with CGS/NSApplication bootstrapping) successfully captures full-display streams via `SCContentFilter::init_with_display_exclude_windows`. The CLI now receives continuous frames and writes PNGs when `--window-id display:N` is supplied.
- **Window capture ❌ (current blocker)**: Using `SCContentFilter::init_with_desktop_independent_window` returns only a single “blank” sample (`SCStreamFrameInfoStatus` missing, attachment array count = 1, total sample size = 0) and then the stream times out. The ScreenCaptureKit pipeline/callbacks are active, but pixel buffers are never delivered for window-only targets.
- **Hypothesis**: Desktop-independent windows appear to require additional configuration before frames arrive. Apple’s docs mention supplying explicit `sourceRect`/`destinationRect` or creating a display filter that includes the window.

### Next steps to unblock window capture
1. When building the window configuration, set `SCStreamConfiguration`’s `sourceRect` / `destinationRect` to the window’s frame (and confirm ScreenCaptureKit accepts those calls at runtime).
2. If frames remain blank, try switching to `SCContentFilter::init_with_display_include_windows` (display-based filter + include list) and verify whether the target window renders correctly.
3. Once frames arrive, remove the temporary frame timeout warning suppression and ensure `next_frame` still handles the initial blank samples gracefully.
