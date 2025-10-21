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
  - Prometheus counters `manager_viewer_keepalive_sent_total`, `manager_viewer_keepalive_failures_total`, `manager_viewer_idle_warnings_total`, and `manager_viewer_idle_recoveries_total` expose ping cadence, failures, idle intervals, and recoveries per session, unlocking Grafana/Honeycomb alerting.
- **Dashboard drawer parity**
  - Session drawers now poll `GET /sessions/:id/controller-events` with bearer auth (no SSE), reuse trimmed tokens, and render structured controller events alongside the terminal.

### 🔄 In Progress / Follow-Ups
1. **UX polish**
   - Surface transport health (secure/plaintext badge, latency) in the tile chrome.
   - Add a reconnection banner when the viewer falls back to reconnect loops.
2. **Credential hardening & legacy removal**
   - Land Gate/Beach Road support for signed viewer tokens, update the manager API to prefer them, and remove the fallback once hosts understand the new credential.
   - Migrate any remaining consumers off `/sessions/:id/events/stream`, then delete the SSE endpoint.
3. **Hardening**
   - Add an integration test that asserts `WireUpdate::Style` survives the viewer pipeline.
   - Simulate TURN-only environments to ensure the keepalive cadence doesn’t trigger quota alarms.
