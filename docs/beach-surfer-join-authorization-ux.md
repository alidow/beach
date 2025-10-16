# Beach Surfer — Join Authorization Waiting UX

This complements server and CLI plans by detailing the Beach Surfer browser experience while a viewer waits for host approval.

## Context
- Host approval gates when the server begins sending sync frames (Hello/Grid). The WebRTC data channel may already be open, but the host has not authorized the viewer yet.
- Without guidance, the web UI risks showing an empty screen or appearing frozen.

## UX Goals
- Clear, reassuring state while waiting for approval.
- Smooth animation that communicates liveness (low CPU, accessible).
- Strict input gating so no keystrokes/mouse go to the host pre‑approval.
- Friendly outcomes: approved (brief transition), denied (clear message), or disconnected.
- Mobile‑friendly and resilient to slow networks.

## Implementation Status
- ✅ Overlay UX wired into `apps/beach-surfer/src/components/BeachTerminal.tsx` with CSS spinner, timed hints, and `beach:status:*` handling.
- ✅ Server sends `beach:status:approval_{pending,granted,denied}` so the browser can react immediately.
- ✅ Optional viewer label supported via the `label` query parameter (forwarded to the host prompt).
- ⬜ Optional telemetry and toast/CTA polish remain future work.

## States & Transitions
1) Connecting
   - As soon as an auto-connect attempt begins, show a compact overlay (“Connecting to host...”) with a CSS spinner.
   - The overlay is non-blocking (pointer-events disabled) so key handling logic remains unchanged.

2) Waiting for host approval
   - Trigger: RTC data channel open with no `Hello` yet.
   - Overlay switches to “Connected - waiting for host approval...” with the same spinner.
   - If the host hasn’t provided custom text, timed hints swap in after 10 s/30 s (“Still waiting... hang tight.” / “Still waiting... ask the host to approve.”).

3) Approved → Syncing
   - Trigger: `beach:status:approval_granted` or the first `Hello` frame.
   - Overlay shows “Approved - syncing...” for ~1.2 s, then fades away automatically.

4) Denied / Disconnected before approval
   - Trigger: `beach:status:approval_denied` or data channel closure prior to `Hello`.
   - Overlay shows either “Join request was declined by host” or “Disconnected before approval”, then hides after roughly 1.5 s.

## Signals & Detection
- Primary: textual data-channel messages with prefix `beach:status:` (pending/denied/granted) emitted by the host runtime.
- Fallback: if no status arrives within ~750 ms of the data channel opening, the client enters waiting mode with default copy.
- Approval is also inferred from the first `Hello` frame in case the host omits `approval_granted`.
- Heartbeats are not required for animation; the overlay relies on a local timer.

## Input Gating
- Keyboard and mouse forwarding already hinge on `subscriptionRef` in `BeachTerminal`; until `Hello` arrives, resize/input events short-circuit and stay local.
- No additional buffering is required—the client simply discards pre-handshake input.

## Component Design
- Location: `apps/beach-surfer/src/components/BeachTerminal.tsx`.
- `JoinStatusOverlay` renders within the wrapper (absolute positioning, pointer-events none) and receives the derived state/message.
- State machine:
  - `idle` → `connecting` when an auto-connect attempt starts.
  - `connecting` → `waiting` on data channel open (or immediately if already open).
  - `waiting` → `approved` on `approval_granted`/`Hello` → auto-hide after ~1.2 s.
  - `waiting` → `denied` on `approval_denied`; `waiting` → `disconnected` if the channel closes pre-handshake; both auto-hide after ~1.5 s.

## Animation & Performance
- The overlay uses a single Tailwind `animate-spin` border spinner (no JS tick loop).
- Timed hints rely on `setTimeout`, not animation frames, so idle CPU remains low.
- Styling sticks to opacity/transforms to stay GPU-friendly.

## Accessibility
- High‑contrast text; spinner accompanied by text (“Waiting for host approval…”).
- Respect reduced motion: `@media (prefers-reduced-motion)` → reduce animation to simple pulsing or static text.
- Screen readers: `aria-live="polite"` on status text; ensure focus remains within the page but not trapped.

## Mobile/Responsive
- Overlay scales to small viewports; avoid fixed pixel widths.
- Tap‑targets for CTAs ≥ 44px; ensure keyboard doesn’t occlude messages.

## Error & Timeout Handling
- Progressive hints swap into the overlay after 10 s and 30 s when no custom message is present.
- Denied/disconnected overlays automatically clear after a short delay; any follow-up join attempt starts fresh.

## Telemetry
- TODO: add timings such as `dc_open_to_hello_ms`, `waiting_duration_ms`, `denied_count` once instrumentation infrastructure lands.

## Implementation Steps (beach-surfer)
1) ✅ Overlay component + styles (`JoinStatusOverlay`) with accessibility-conscious copy.
2) ✅ Extended `BeachTerminal` state machine (`joinState`, timers, `handshakeReadyRef`).
3) ✅ Reused existing `subscriptionRef` gating for input; no additional buffering required.
4) ✅ Consumed `beach:status:*` control messages and added 750 ms fallback when absent.
5) ⬜ Playwright/Cypress coverage for pending → approved/denied/disconnected flows.
6) ⬜ Wire telemetry + documentation updates in the broader web plan.

## Server Coordination
- Host runtime already emits `beach:status:approval_pending`/`approval_denied`/`approval_granted` alongside heartbeats.
- Continuing to send heartbeats during authorization keeps the CLI stats alive; the browser overlay relies solely on local timers.

This plan keeps the browser experience clear and calm while preserving safety by not forwarding input until the host approves.
