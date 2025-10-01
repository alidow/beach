# Beach Web Cursor Desync Implementation Notes

## Summary

Authoritative cursor frames are being introduced so the Beach web client no
longer relies on heuristic inference. Host and client now exchange explicit
`cursor` data (position, visibility, blink, and sequence number) alongside the
grid updates. Predictive typing is still supported, but speculative cursor moves
collapse as soon as the host confirms or rejects the prediction.

## Work Completed

### Protocol / Wire Format
- Bumped `PROTOCOL_VERSION` to `2` and defined `FEATURE_CURSOR_SYNC` negotiation
  bit (Rust & TypeScript).
- Added `CursorFrame` structure and optional cursor payloads to `snapshot`,
  `delta`, and `history_backfill` frames, plus a standalone `cursor` frame.
- Updated Rust (`apps/beach-human/src/protocol`) and TS
  (`apps/beach-web/src/protocol`) encoders/decoders with round-trip tests.

### Host Runtime (`apps/beach-human`)
- `TransmitterCache` coalesces cursor updates and tracks last emitted cursor.
- `spawn_update_forwarder` piggybacks cursor data on snapshot/delta/backfill
  batches and emits dedicated cursor frames when necessary.
- `AlacrittyEmulator` surfaces cursor state (row/col, visibility, blink) after
  each processed chunk.
- `TerminalClient` handshake stores `FEATURE_CURSOR_SYNC`, applies cursor frames
  authoritatively, tracks predicted cursor positions, and reconciles predictions
  when host seq overtakes them.

### Web Client (`apps/beach-web`)
- `TerminalGridCache` now owns:
  - Cursor feature flag, authoritative state, predicted cursor state, visibility
    and blink flags, and latest cursor seq.
  - Updated `applyUpdates` to accept `{ authoritative, origin, cursor }` options;
    cursor frames mark the cache dirty even when grid cells are unchanged.
  - Predictions maintain a separate predicted cursor that is cleared on ack or
    when host seq supersedes it.
- `TerminalGridStore` exposes `setCursorSupport` and `applyCursorFrame` helpers;
  snapshots now include cursor metadata for renderers.
- `BeachTerminal` handles hello features, forwards cursor frames to the store,
  and renders predicted cursor underline distinct from the confirmed cursor.
- Added unit tests covering cache/store cursor frames, predicted cursor
  behaviour, and renderer output (`cache.test.ts`, `gridStore.test.ts`,
  `BeachTerminal.lines.test.ts`, `wire.test.ts`).

## Remaining Work

1. **Runtime validation**
   - Manually validate both CLI preview (`BEACH_CURSOR_SYNC=1 cargo run ...`) and
     web client to ensure cursor remains aligned through prompt redraws,
     carriage returns, and Ctrl-U.
   - Verify downgrade behaviour with legacy clients: the host should still send
     heuristic data when the feature flag is disabled and the web client should
     degrade gracefully.

2. **Docs / rollout**
   - Document the feature flag (`BEACH_CURSOR_SYNC`) and minimum protocol
     version requirement.
   - Once validation passes, remove fallback heuristics and update release
     notes.

## Hand-off Checklist
- [x] Search for lingering `features: 0` duplicates or missing features fields
  across tests/fixtures and fix them.
- [x] Confirm all history backfill/snapshot fixtures include `cursor: None` (or
  actual cursor frames) where expected.
- [x] Re-run full test suites (Rust + TS) and capture failures.
- [ ] Perform manual regression in both CLI and web environments with and
  without the feature flag.
- [ ] Update documentation / rollout guide once validated.

With these cleanups and validations, the cursor desync effort can move into the
final integration/testing phase.
