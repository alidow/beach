# WS-E Progress Log
- **Owner**: Codex
- **Last updated**: 2025-11-05T16:22:00Z
- **Current focus**: Session attach telemetry, credential persistence, and Clerk readiness

## Done
- Added manager-authenticated connect form to Application tile (session id + passcode attach)
- Wired tile lifecycle to shared viewer connection service and embedded `SessionTerminalPreviewClient`
- Surfaced connection status/latency badges and synchronized tile metadata with viewer state

## Next
- Instrument Application tile actions with WS-F taxonomy (`canvas.tile.connect.*`, `viewer.retry`) and emit tile/session metadata; land PR by 2025-11-06 to unblock telemetry dashboards.
- Define persisted session credential schema (session id, credential hash/passcode hint, harness type) with WS-D + backend and prototype serializer shared with layout persistence by 2025-11-08.
- Validate Clerk token availability across local/dev/staging: add smoke test exercising `useManagerToken` fallback + refresh and document sign-in prerequisites (pair with WS-A) by 2025-11-07.

## Blockers / Risks
- Rewrite still depends on Clerk template `NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE`; staging template activation pending (awaiting WS-B auth team ETA).
- Need confirmation that storing credential overrides client-side is acceptable until manager API exposes encrypted persistence (follow-up tracked with backend).

## Notes
- Tile metadata now reflects viewer status to drive header summaries and catalog imports.
