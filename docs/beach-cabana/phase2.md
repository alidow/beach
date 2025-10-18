# Beach Cabana – Phase 2 Notes (Capture & Encoding Adapters)

Deliverables accomplished:

- Expanded `capture/` module with resize/downscale helpers and a `FrameProducer` trait consumed by both streaming and encoding flows.
- Implemented `GifVideoEncoder` as the initial software fallback plus `encode` CLI command (animated GIF output, downscaling via `--max-width`).
- Hooked CLI streaming/encoding commands into the capture abstraction, so future ScreenCaptureKit and hardware encoders can slot in without touching CLI surfaces.
- Feature-gated ScreenCaptureKit bridge remains planned via `cabana_sck` following the spike instructions (`docs/beach-cabana/screencapturekit-spike.md`).
- ScreenCaptureKit is now the default macOS capture backend with automatic CoreGraphics fallback and telemetry that logs blank-frame retries, frame dimensions, and delivery latency. The CLI `stream` command now emits per-run latency and byte metrics to help capture baselines. The `encode` command accepts a `--codec` flag and now supports VideoToolbox-backed H.264 output (`--codec h264`) alongside the GIF fallback.

Next steps:

1. Capture baseline metrics with the ScreenCaptureKit path (verify blank-frame telemetry and fallback logs under load).
2. Integrate VideoToolbox / hardware encoders behind the `VideoEncoder` trait (reuse GIF fallback as debug option).
3. Add automated capture/encode tests once `cabana_sck` is available in CI (macOS).

Test/metrics notes (2025-10-18):
- Added unit test for VideoToolbox H.264 encoder that writes a short Annex B stream from synthetic frames: `apps/beach-cabana/src/encoder/videotoolbox.rs` (macOS + `--features cabana_sck`).
- Added ignored smoke test for ScreenCaptureKit display streaming that captures a couple frames to a temp directory: `apps/beach-cabana/src/platform/macos/sck.rs` (run locally with GUI + screen-recording permission).
- The `stream` CLI now prints average/min/max frame latency and byte totals per run to aid baseline collection; SCK blank-frame events remain logged at `debug`.

How to run locally (macOS):
- `cargo test --manifest-path apps/beach-cabana/Cargo.toml --features cabana_sck`
- `cargo test --manifest-path apps/beach-cabana/Cargo.toml --features cabana_sck -- --ignored` (runs SCK smoke test)
- `RUST_LOG=info cargo run --manifest-path apps/beach-cabana/Cargo.toml -- stream --window-id display:<ID> --frames 60 --interval-ms 16`

## 2025-10-15 – ScreenCaptureKit Integration Status

- **Display capture ✅**: The new Rust bridge (with CGS/NSApplication bootstrapping) successfully captures full-display streams via `SCContentFilter::init_with_display_exclude_windows`. The CLI now receives continuous frames and writes PNGs when `--window-id display:N` is supplied.
- **Window capture ❌ (current blocker)**: Using `SCContentFilter::init_with_desktop_independent_window` returns only a single “blank” sample (`SCStreamFrameInfoStatus` missing, attachment array count = 1, total sample size = 0) and then the stream times out. The ScreenCaptureKit pipeline/callbacks are active, but pixel buffers are never delivered for window-only targets.
- **Hypothesis**: Desktop-independent windows appear to require additional configuration before frames arrive. Apple’s docs mention supplying explicit `sourceRect`/`destinationRect` or creating a display filter that includes the window.

### Follow-up (2025-10-16)
- Implemented the display+include-windows filter path and configured `sourceRect`/`destinationRect` for targeted windows. This unblocks window capture: `beach-cabana stream --window-id <CGWindowID>` now produces PNGs the same way display capture does.
- Retained light debug logging for blank frames so future agents can diagnose if ScreenCaptureKit ever regresses into `SCFrameStatusBlank` again.
