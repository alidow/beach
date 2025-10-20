# Beach Cabana – Phase 5 (Web Viewer Experience)

Status (2025-10-20)
- ✅ Cabana hosts now emit fragmented MP4 segments for `--codec h264` using the new Rust `Fmp4Writer`, so browsers can ingest live video without a transmux shim.
- ✅ Beach Surfer’s unified viewer gained media controls (pause/resume, fit-to-window cycling, stats toggle) plus a metrics overlay that reports resolution, FPS, bitrate, buffered depth, and total frames.
- ✅ PNG and H.264 transports surface actionable errors (unsupported codec, append failures) while continuing to show the Noise verification badge from Phase 4 before playback starts.
- ✅ Media stats feed into the control HUD, and playback state stays in sync with both the CLI/TUI flows and the desktop picker confirmation gate.

What landed
- Added `Fmp4Writer` inside `apps/beach-cabana/host/src/mp4.rs` and rewired the H.264 streaming path to broadcast init segments + `moof/mdat` fragments over the secure data channel.
- Extended `MediaCanvas` and `MediaVideo` to support pause/resume, fit modes (`contain`, `cover`, `actual`), metrics reporting, and error callbacks. Both components now translate transport metrics into viewer-facing stats.
- Introduced `ViewerControls`, a HUD overlay that provides controls and renders the metrics/error panel. `BeachViewer` orchestrates play state, fit mode, stats toggling, and hands off the metrics feed to the overlay.
- Updated documentation to mark Phase 5 complete and captured the new workflow for future operators.

How to run
- PNG stream (interactive controls): \
  `pnpm --filter beach-surfer dev` and join a Cabana PNG session; use the on-screen Play/Fit/Stats buttons to validate behaviour.
- H.264 stream end-to-end: \
  `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml --features webrtc -- webrtc-host-run --session-id demo --passcode secret --codec h264 --frames 300 --interval-ms 33` \
  then attach with beach-surfer; observe the stats overlay reporting MPG fragments, FPS, bitrate, and buffer depth.

Notes & gaps
- The stats HUD currently updates every ~500 ms with rolling averages. Longer-term we should expose historical charts or splice data into observability.
- Fit cycling covers the main viewer expectations; future work could add keyboard shortcuts, remember per-session preferences, and expose zoom/pan for actual-pixel mode.
- Fragmented MP4 currently targets H.264 Baseline/Constrained Baseline. HEVC/AV1 muxing will require additional encoder adapters.

Next steps toward Phase 6
- Thread the viewer metrics into shared telemetry so Cabana sessions emit frame rate, bitrate, and buffer health alongside terminal stats.
- Begin refactoring `beach-core` traits so GUI and terminal hosts can be selected via feature flags without forking CLI flows.
- Evaluate hooking the viewer controls into the desktop host shell for richer UX (pause/stop sharing, annotate overlays) once GUI integration wiring lands.
