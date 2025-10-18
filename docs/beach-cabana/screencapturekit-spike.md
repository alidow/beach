# Beach Cabana – ScreenCaptureKit Streaming Spike

Goal: prototype a macOS-only high-FPS capture path that replaces the current Core Graphics snapshots with ScreenCaptureKit’s incremental frame delivery, while keeping Cabana standalone and closed source.

## Requirements
- Capture windows or displays selected via existing identifiers.
- Produce BGRA frames (or CVPixelBuffer handles) that Cabana can hand to future encoder modules.
- Maintain zero-trust guarantees: no additional secrets leave the host process.
- Keep integration self-contained (no changes to open-source Beach crates yet).
- Feature gated: enable with `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml --features cabana_sck -- stream ...` so OSS builds remain unaffected.

## Integration Approaches

### 1. Swift/Obj‑C Shim + FFI (Recommended)
- Author a small Swift package (`CabanaCaptureKit`) exposing C-callable functions to:
  - Enumerate `SCShareableContent`.
  - Start an `SCStream` for a window/display with desired resolution/FPS.
  - Provide callbacks delivering `CMSampleBuffer` pointers.
- Build as a static/dynamic library included in the proprietary workspace.
- Bind from Rust via `unsafe extern "C"` functions (using `cbindgen` + module map).
- Pros: minimal Objective-C runtime work in Rust; future-proof with Swift.
- Cons: tooling complexity (Swift Package Manager build step).

### 2. Pure Rust + `objc`/`block` crates
- Use `objc` crate to message ScreenCaptureKit APIs directly.
- Marshal blocks via `block` crate and translate `CMSampleBuffer` to `CVPixelBuffer`.
- Pros: single toolchain (Rust only).
- Cons: tedious memory management; harder to keep up with Apple API changes.

## Proposed Plan
1. **Scaffold capture runtime**
   - Add `apps/beach-cabana/host/src/capture/mod.rs` with trait `FrameProducer` and a macOS implementation behind `#[cfg(target_os = "macos")]`. (Module scaffold in place; trait still TODO.)
   - For initial spike, include feature flag `cabana_sck` to compile the ScreenCaptureKit bridge.
2. **Swift shim (if chosen)**
   - Create `apple/CabanaCaptureKit/Package.swift`.
   - Functions:
     ```c
     typedef void (*cabana_frame_callback)(const uint8_t* bytes, size_t len, size_t width, size_t height, void* ctx);
     int cabana_start_capture(const char* target_id, uint32_t fps, cabana_frame_callback cb, void* ctx);
     void cabana_stop_capture(int handle);
     ```
   - Expose build script (`build.rs`) compiling the Swift package into `.a`/`.dylib` and link in Cabana.
3. **Rust adapter**
   - Implement `ScreenCaptureKitProducer` that:
     - Resolves target (window/display) using existing enumeration metadata.
     - Bridges callback to async channel delivering `Frame { timestamp, buffer }`.
     - Provides iterator/stream API for CLI `stream`.
4. **CLI integration**
   - Update `stream` command to detect `cabana_sck`; when enabled, consume live frames asynchronously and write PNGs / metrics.
   - Fallback to CoreGraphics polling when flag absent.
5. **Telemetry**
   - Log FPS, frame drops, latency to inform encoder tuning.
6. **Testing**
   - Manual validation: capture Terminal, complex window, full desktop.
   - Add integration test (macOS only) behind `#[cfg_attr(not(target_os = "macos"), ignore)]` that ensures at least N frames arrive within timeout (requires CI gating).

## Open Questions
- How to package Swift artifacts for distribution (XCFramework vs raw dylib)?
- Do we need entitlements/custom Info.plist for ScreenCaptureKit?
- How to expose audio capture (SCStream supports audio—likely later phase)?

## Deliverables for Spike
- Prototype CLI command (`beach-cabana stream --sck`) that retrieves ~60 frames over 5 seconds.
- Notes on build pipeline adjustments for Swift integration.
- Updated docs summarizing limitations and next tasks.
