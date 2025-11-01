## Private Beach Viewer Follow-Ups (Oct 2025)

### ✅ Completed
- **Keepalive + idle telemetry**
  - `run_viewer_worker` now sends a dedicated `__keepalive__` ping every 20 s and raises a warning if no host frames arrive for 45 s. This surfaced reconnect loops without relying on ad‑hoc log tailing.
- **Viewer style fidelity**
  - `ManagerViewerState::apply_update` persists `WireUpdate::Style`, so cached diffs retain the host’s style table.
  - Private Beach dashboard tiles render through the shared `BeachTerminal` component, restoring Surfer parity and eliminating the monochrome “terminal green” regression.
- **Client-side diagnostics**
- Controller-driven viewer service (`viewerConnectionService`) logs data-channel open/close events and signaling errors, making reconnect root-cause analysis possible from browser logs.
  - Cabana sessions short‑circuit to `CabanaPrivateBeachPlayer`, keeping media‑specific UX intact while terminals reuse Beach Surfer.
- **Reconnect metrics**
  - Prometheus counters `manager_viewer_keepalive_sent_total`, `manager_viewer_keepalive_failures_total`, `manager_viewer_idle_warnings_total`, and `manager_viewer_idle_recoveries_total` expose ping cadence, failures, idle intervals, and recoveries per session, unlocking Grafana/Honeycomb alerting.
- **Dashboard drawer parity**
  - Session drawers now poll `GET /sessions/:id/controller-events` with bearer auth (no SSE), reuse trimmed tokens, and render structured controller events alongside the terminal.
- **Credential hardening & legacy removal**
  - Manager now issues Gate-signed viewer tokens by default; passcode fallback is gone and Beach Road validates the signature before honoring joins.
  - `/sessions/:id/events/stream` was removed after the dashboard migrated fully to WebRTC + REST polling.
- **Dashboard polish**
  - Private Beach tiles surface transport security/latency badges and show reconnect messaging when negotiations restart; the expanded viewer keeps parity with Beach Surfer.
- **Viewer pipeline regression tests**
  - Added `manager_viewer_style_updates_survive_pipeline` to guard `WireUpdate::Style` handling.
  - TypeScript target bumped to ES2020 with module stubs so `npx tsc --noEmit` succeeds alongside the shared Surfer code.

### 🔄 In Progress / Follow-Ups
1. **TURN-only soak**
   - Manual recipe documented (`BEACH_WEBRTC_DISABLE_STUN=1`), but we still owe a sustained TURN-only load test to baseline quota impact and confirm keepalive cadence.
