# Private Beach Terminal Preview – Color Initialization Follow-Up

## Issue Overview
- **Symptom:** Private Beach dashboard tiles render BeachTerminal previews without their initial styled colors. Tiles appear monochrome until the live WebRTC viewer reconnects and streams fresh frames.
- **Root Cause:** The dashboard relies on cached `StateDiff` payloads (`terminal_full`) emitted by Beach Manager. Prior to the latest changes those payloads only included plain text lines and cursor metadata; the per-cell style table was not materialised, so client hydrators had no way to restore colors before a live transport subscribed.
-
- **Impact:** Tiles stay visually “cold” whenever the full viewer disconnects/reconnects, after page refreshes, or while the cached Manager diff is the only data source (e.g., when testing fallback flows without WebRTC).

## Completed Work
1. **Manager snapshot enhancements** (`apps/beach-manager/src/state.rs`):
   - `capture_terminal_frame_simple` now exports `styled_lines` alongside `lines`. Each cell bundles its character plus a `style` payload containing the resolved foreground/background colors and attribute bits.
   - A `styles` array mirrors the terminal style table so downstream consumers can reconcile ids with rgba values.
   - Added regression coverage (`state::tests::manager_viewer_style_updates_survive_pipeline`) to ensure emitted payloads include both the enriched style metadata and the legacy plain-text lines.
2. **Protocol surface update** (`crates/beach-buggy/src/lib.rs`):
   - Extended `TerminalFrame` with `styled_lines`, `styles`, `rows`, and `cols`.
   - Introduced `StyledCell` and `CellStylePayload` structs so harnesses, Manager, and testing utilities share the enriched payload contract.

## Remaining Scope to Finish the Fix
1. **Dashboard hydrator update**
   - Teach `apps/private-beach` client code (TileCanvas / SessionTerminalPreviewClient path) to detect `styled_lines` + `styles` in cached payloads and hydrate the `TerminalGridStore` before any live transport attaches.
   - Add unit coverage for the hydrator branch to confirm colors round-trip from `StateDiff` without a live viewer.
2. **Fallback compatibility guard**
   - Maintain backwards compatibility: gracefully handle legacy payloads that still omit `styled_lines` by falling back to plain-text hydration.
3. **Sandbox verification**
   - Use the Private Beach sandbox (`/apps/private-beach/src/pages/dev/private-beach-sandbox.tsx`) to reproduce a styled tile end-to-end.
   - Add a sandbox test step to this validation plan with the provided session metadata (contains blue borders):
     ```json
     {
       "schema": 2,
       "session_id": "856b9a1b-bf02-4c8d-95a8-5e3483b8ecfc",
       "join_code": "RV9TA7",
       "session_server": "http://localhost:4132/",
       "active_transport": "WebRTC",
       "transports": ["webrtc", "websocket"],
       "preferred_transport": "webrtc",
       "webrtc_offer_role": "offerer",
       "host_binary": "beach",
       "host_version": "0.1.0-20251027174235",
       "timestamp": 1761848284,
       "command": [
         "/usr/bin/env",
         "python3",
         "/Users/arellidow/development/beach/apps/private-beach/demo/pong/player/main.py",
         "--mode",
         "lhs"
       ],
       "wait_for_peer": true,
       "mcp_enabled": false
     }
     ```
   - Launch the sandbox with these parameters, ensure the tile renders the blue-bordered preview immediately after page load (without waiting for live frames), and capture console logs.
4. **Manual verification checklist**
   - Clear Manager cache / restart the worker to force reliance on `last_state`.
   - Reload the dashboard in production-like mode and confirm colors appear instantly.
   - Re-run `cargo test -p beach-manager` and the relevant `apps/private-beach` test suite once the hydrator changes land.

## Hand-Off Notes
- The enriched payload surfaced by Manager already ships; only the Private Beach client needs to consume it.
- Look for TODOs or comments referencing “hydrate from styled_lines” once the client work begins.
- Keep the existing telemetry (`[terminal][trace]` logs) – they will confirm when previews fall back to cached styled data vs. live viewer frames.

