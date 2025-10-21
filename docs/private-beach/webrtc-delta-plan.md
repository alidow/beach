## Private Beach Viewer Follow-Ups (Oct 2025)

### ✅ Completed
- **Keepalive + idle telemetry**
  - `run_viewer_worker` now sends a dedicated `__keepalive__` ping every 20 s and raises a warning if no host frames arrive for 45 s. This surfaced reconnect loops without relying on ad‑hoc log tailing.
- **Viewer style fidelity**
  - `ManagerViewerState::apply_update` persists `WireUpdate::Style`, so cached diffs retain the host’s style table.
  - Private Beach dashboard tiles render through the shared `BeachTerminal` component, restoring Surfer parity and eliminating the monochrome “terminal green” regression.
- **Client-side diagnostics**
  - `useSessionTerminal` logs data-channel open/close events and signaling errors, making reconnect root-cause analysis possible from browser logs.
  - Cabana sessions short‑circuit to `CabanaPrivateBeachPlayer`, keeping media‑specific UX intact while terminals reuse Beach Surfer.
- **Reconnect metrics**
  - Prometheus counters `manager_viewer_keepalive_failures_total` and `manager_viewer_idle_warnings_total` expose failed pings and idle intervals per session, unlocking Grafana/Honeycomb alerting.
- **Dashboard drawer parity**
  - Session drawers now poll `GET /sessions/:id/controller-events` with bearer auth (no SSE), reuse trimmed tokens, and render structured controller events alongside the terminal.

### 🔄 In Progress / Follow-Ups
1. **UX polish**
   - Surface transport health (secure/plaintext badge, latency) in the tile chrome.
   - Add a reconnection banner when the viewer falls back to reconnect loops.
2. **Hardening**
   - Add an integration test that asserts `WireUpdate::Style` survives the viewer pipeline.
   - Simulate TURN-only environments to ensure the keepalive cadence doesn’t trigger quota alarms.
