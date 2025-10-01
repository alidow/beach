# Multi-Client Viewport Strategy

_Last updated: 2025-09-30_

When we added WebRTC fan-out, every viewer began participating in the same PTY resize loop. The host accepts a `resize(cols, rows)` from whichever client sends it, adjusts the PTY, and broadcasts a `Grid` frame to all participants. Because that `Grid` frame currently includes the newly chosen `viewportRows`, every other client dutifully grows its own terminal, measures the larger viewport, and emits a *new* resize. The host honors it, rebroadcasts, and the loop repeats—Rust CLI, beach-web, everyone. The root problem is that we’re conflating the shared terminal buffer with each viewer’s local presentation geometry.

## Guiding principles

1. **One PTY size, many viewports.** The host must keep a single canonical PTY size (otherwise shells mis-render), but each viewer owns its *rendered* viewport height. PTY size is a shared resource; viewport height is client-local.
2. **Resize is a command, not a state broadcast.** When a client issues `resize(cols, rows)`, it is asking the host to reconfigure the PTY. The acknowledgement should not force other clients to inherit that same height.
3. **History snapshots stay global.** The `Grid` frame should continue to advertise `historyRows`, `baseRow`, and `cols` so everyone shares the same buffer and trimming behaviour. What changes is how viewers choose to render that buffer.

## High-level changes

- **Host (apps/beach-human):**
  - Continue honoring `ClientFrame::Resize` by resizing the PTY, updating the emulator, and trimming history.
  - Remove (or clearly mark as advisory) `viewport_rows` from the broadcast `HostFrame::Grid`. Optionally send the new viewport height only to the client that initiated the resize so it knows the PTY was updated.
  - Do *not* overwrite every other client’s viewport height when a resize arrives.

- **Viewers (Rust CLI + beach-web):**
  - Measure the local container/window (crossterm `Event::Resize` in Rust; `ResizeObserver` in React) and keep that as the actual viewport height.
  - Ignore the server’s `viewport_rows` unless they were the original requester. In practice, clamp to `min(localMeasured, serverSuggestion)` or just drop the field entirely.
  - Only send a new resize when the user changes the local window size, not because the host shipped a bigger `Grid`.

- **Optional coordination layer:** If we want stricter control, elect a “viewport owner” (e.g., the host/primary presenter). Only that owner’s resize commands touch the PTY. Everyone else stays read-only. This eliminates duelling resizes without disabling the feature entirely.

## Expected results

With these changes in place:

- A viewer resizing their window still resizes the PTY and updates the shared buffer.
- Other viewers immediately receive the updated `Grid` (so history stays in sync) but continue rendering with their own viewport height.
- The resize feedback loop disappears because the host no longer tells everyone else to grow.

## Detailed implementation plan

1. **Lock down the protocol contract**
   - Adopt option 1 from the design discussion: `HostFrame::Grid` will no longer broadcast `viewport_rows` to all clients.
   - Document the decision in protocol comments/release notes so future work doesn’t reintroduce the field by accident.

2. **Adjust protocol structs & encoding**
   - Update `apps/beach-human/src/protocol/mod.rs` and `apps/beach-human/src/protocol/wire.rs` to remove (or make optional) `viewport_rows` from the `Grid` variant.
   - Update encoding/decoding tests and fixtures; ensure decoders tolerate the missing field for backwards compatibility.

3. **Change host broadcast logic**
   - In `apps/beach-human/src/main.rs`, keep resizing the PTY/emulator when handling `ClientFrame::Resize`, but emit `HostFrame::Grid` without `viewport_rows`.
   - If the initiating client needs confirmation, optionally send a private acknowledgement (e.g., a targeted `Grid` frame or a new `resize_ack` message); otherwise rely on local measurement.

4. **Update the Rust CLI client**
   - Remove usage of `viewport_rows` in `apps/beach-human/src/client/terminal.rs` so the TUI relies solely on local `Event::Resize` measurements.
   - Ensure renderer helpers don’t try to force the viewport to match the broadcasted height.

5. **Update beach-web**
   - Drop remaining dependencies on server-supplied viewport heights in `apps/beach-web/src/components/BeachTerminal.tsx`; rely on `ResizeObserver` + internal clamps.
   - Simplify temporary suppression logic once the host stops broadcasting the field.

6. **Clean up tests & add regression coverage**
   - Update Rust + web unit/integration tests that assert on `viewport_rows`.
   - Add a regression scenario (e.g., two simulated clients alternating resize commands) to confirm the host no longer triggers an escalation.

7. **Telemetry & verification**
   - Log PTY resize events with peer IDs and clamped values while validating the fix in staging/local runs.
   - Manual QA: host + multiple viewers, resize independently, confirm the loop disappears, history/backfill still works.

8. **Compatibility / rollout**
   - If older clients might still send/expect `viewport_rows`, consider a protocol version bump or tolerant decoding.
   - Communicate the change in release notes/docs.

Once we decouple presentation from the PTY commands, multi-view sessions stop fighting each other and each viewer can render at whatever size fits their screen.

## Implementation progress _(2026-05-09)_

- Binary protocol changed: `HostFrame::Grid.viewport_rows` is now optional and only populated for the requesting client. All decoding paths tolerate the missing field (Rust + web).
- Host now omits `viewport_rows` when broadcasting to other participants while still acknowledging the initiating resize with a targeted frame.
- Rust CLI no longer trusts server-supplied heights; it leans on local terminal measurements and keeps independent viewport state. Empty-backfill retry logic was adjusted so trimmed histories don’t loop while genuine gaps still retry.
- Web client relies solely on the local `ResizeObserver` value and ignores the advisory field.
- Updated integration/unit tests cover the optional field, handshake behaviour, and history backfill retries so regressions are caught automatically.
