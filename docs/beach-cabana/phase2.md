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
