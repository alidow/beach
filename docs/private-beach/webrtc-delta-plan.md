## Private Beach Viewer Follow-Ups (Oct 2025)

### 1. Intermittent viewer reconnects
**Symptom**: Dashboard tiles periodically log a new WebRTC handshake (every few minutes).

**Likely causes**
- Manager’s viewer worker restarts (panic, Redis failure, diff persist error).
- ICE connection idles out (no keepalive on the data channel).

**Proposed fixes**
- Tail `beach-manager` logs around reconnects to identify errors.
- Add a keepalive in `run_viewer_worker` (e.g., send `__ready__` on a `tokio::interval`).
- Enhance client logging (e.g., in `useSessionTerminal`) to capture RTC close events and reasons.

### 2. Terminal styling missing
**Symptom**: Tiles render single-color “terminal green”.

**Root cause**
- `ManagerViewerState::apply_update` ignores `WireUpdate::Style`, so the diff payload never includes the color/style table.
- Result: JSON payload lacks data the React viewer needs to render styles.

**Options**
- Re-apply style handling in `ManagerViewerState`.
- **Preferred**: Reuse Beach Surfer’s React terminal components (they already process style tables). This aligns with the longer-term plan for dashboard parity.

### 3. Path forward
1. Investigate `beach-manager` logs (possibly add structured logging) to find reconnect cause.
2. Implement a viewer keepalive (or reuse the existing heartbeat publisher).
3. Swap dashboard tiles to reuse the shared Surfer terminal component (colors, styling, reconnect behavior already tested there).
