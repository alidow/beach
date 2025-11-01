# Private Beach Terminal View-Only Refactor

## Background

- Host PTYs are being resized to `120×40` whenever a passive viewer connects. Evidence: `/tmp/beach-host.log:2937085` records `WireClientFrame::Resize` as the second inbound frame on the transport, even though the tile UI never triggered a resize.
- The Private Beach tile preview runs `BeachTerminal` with `autoResizeHostOnViewportChange={false}`, yet the handshake still emits a resize frame. Separate instances (Beach Surfer, dev tools, etc.) also default to auto-resize and will keep issuing `resize` frames after handshake.
- Because every viewer reuses the same `BeachTerminal` component, disabling a single path (e.g., ResizeObserver) is insufficient; the component continues to expose helpers (`sendHostResize`, `requestHostResize`, handshake kickers) that can fire even when the caller intends to be “view-only.”

## Goals

- Ensure passive viewers **never** resize the host PTY unless a human explicitly requests it.
- Provide a well-defined “view-only” contract so higher-level surfaces (Private Beach tiles, Surfer previews, Cabana dashboards) can opt in without auditing internal helpers.
- Maintain explicit resize affordances (“Match PTY size”, locked tiles) that require user intent.

## Non-Goals

- Redesigning tile layout or zoom behavior.
- Changing the pong demo or host-side PTY implementation.
- Implementing client-side authorization for resize. (Future work could add ACLs, but is out of scope.)

## Proposed Solution

### 1. Introduce a View-Only Mode for `BeachTerminal`

- New prop: `viewOnly?: boolean` (default `false`). When `true`:
  - Do **not** expose `sendHostResize` / `requestHostResize` in the viewport state payload.
  - Skip wiring the “Match PTY size” UI callback.
  - Force `autoResizeHostOnViewportChange` to `false` internally, regardless of caller input.
  - Guard handshake helpers: no implicit `resize` frames during connect.
  - Ensure any internal state that previously toggled `suppressNextResizeRef` is bypassed; the flag should be irrelevant in view-only mode.

### 2. Update Call Sites

- Private Beach `SessionTerminalPreviewClient`:
  - Pass `viewOnly` to both driver (transport-enabled) and clone instances when the tile is unlocked or otherwise passive.
  - Only disable view-only when the UI explicitly requests host control (e.g., user clicks “Lock & Resize”, “Match PTY size”, or future actions).
- Beach Surfer:
  - Identify contexts that are purely observational (e.g., passive dashboards, read-only inspectors) and opt them into `viewOnly`.
  - Keep full interactive clients (where the user can type) in non-view-only mode.

### 3. Logging & Attribution

- Tag outbound resize frames with a client label/peer ID before the data channel send.
- On the host, extend the resize log to include the `client_label` and whether the request came from a view-only client (should always be `false` after the change).
- Provide a one-click toggle (env/config) for verbose resize logging to aid future debugging.

### 4. Validation / QA Plan

1. Close all viewers, start a host session, attach a PB tile in view-only mode:
   - Expect **no** `processed resize request` in `/tmp/beach-host.log`.
2. Toggle tile “Lock” (or “Match PTY size” once implemented):
   - Expect a single resize frame with the PB tile’s client label.
3. Repeat with Beach Surfer view-only preview:
   - Expect no resizes.
4. Repeat with Surfer interactive session (non view-only):
   - Expect resizes only when the user explicitly triggers “Match PTY size” or a similar control.

### 5. Migration Considerations

- Audit existing consumers of `BeachTerminal` in `apps/beach-surfer`, `apps/private-beach`, `apps/beach-cabana`, and any shared packages. Decide which contexts require interactivity vs. view-only.
- Provide a codemod or lint rule to highlight call sites that forget to specify `viewOnly`.
- Communicate the new behavior to feature teams so they can update dashboards that relied on implicit resizing.

### 6. Risks & Mitigations

- **Risk:** Hidden consumers rely on implicit resizing.  
  **Mitigation:** Add runtime warnings in development when a resize is emitted from a component that did not explicitly opt out of view-only.
- **Risk:** Interactive sessions accidentally marked `viewOnly`, breaking UX.  
  **Mitigation:** Unit tests for `BeachTerminal` to ensure keyboard input + resize still work when `viewOnly=false`.
- **Risk:** Regression in tile sizing heuristics (host metadata no longer lines up).  
  **Mitigation:** Extend existing E2E tile tests to assert PTY size before and after lock/snap actions.

### 7. Deliverables

- Updated `BeachTerminal` component with `viewOnly` mode.
- Updated Private Beach tile preview + Beach Surfer usage.
- Enhanced logging for resize attribution.
- Automated tests covering both passive and interactive scenarios.

## Open Questions

- Should view-only mode also disable other host-affecting actions (e.g., keyboard input, custom host commands)? For now we assume yes; any view-only consumer should already avoid attaching `transport`.
- Do we want a host-side config to **reject** resize frames from clients flagged as view-only? (Could be a follow-up enhancement once client labels are enforced.)
- How should the UI communicate when host resizing is unavailable? (E.g., disable “Match PTY size” button with tooltip.)

