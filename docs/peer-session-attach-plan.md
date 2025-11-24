Peer Session Attach Refactor (2-Step WebRTC Offer Guard)

Goal
- Eliminate transient 404s on `/sessions/:peer_session_id/webrtc/offer` by requiring a created/attached `peer_session_id` before any offer is accepted. Browser/manager cannot post an offer until attach is complete; the endpoint never 404s for “unknown session” in steady state.

Scope/Constraints
- Greenfield: no feature flags or staged rollout needed; replace legacy behavior.
- Terminology: `peer_session_id` = per-terminal attachment used for signaling/routing to the host; distinct from the public host’s session id.
- Components: Beach Road signaling REST/WS, manager/browser clients, host pairing logic.

Plan (bite-sized steps to implement)

API contract
- [x] Define the attach request/response shape (fields: host_session_id, passcode?, role, peer_id?; response: peer_session_id, host_endpoint).
- [x] Update offer/answer spec to require peer_session_id; replace 404 with 409/Retry-After if polled pre-attach.
- [x] Document the contract (auth headers, response codes).

Beach Road
- [x] Add `POST /peer-sessions/attach` that validates auth, looks up host_session_id+passcode, mints peer_session_id, persists mapping, returns it.
- [x] Add storage helpers: create/read peer_session mapping; key WS routing by peer_session_id.
- [x] Update `/peer-sessions/:peer_session_id/webrtc/offer|answer` to require existing peer_session; return 409/Retry-After if not visible yet (no 404).
- [x] Update tests for storage/handlers to cover attach, offer/answer happy path, and pre-attach polling.

Manager/browser clients
- [x] Manager side: transport auto-attaches legacy `/sessions` URLs with `{role}` metadata; peer_session_id logged/telemetry in Rust.
- [x] Browser side (beach-surfer): attach first with `{role, peer_id}`, persist peer_session_id, switch to `/peer-sessions/:id/webrtc/*`, add retry/backoff. (rewrite-2 still lacks signaling code.)
- [x] Send offers/answers only with peer_session_id; add retry/backoff on 409/Retry-After (browser path wired in beach-surfer).
- [ ] Update telemetry for attach/offer attempts and retries (browser path pending).
- [ ] Wire explicit metadata: include `{role, peer_id}` in attach for both manager and browser.

Host side
- [ ] Route/control traffic keyed by peer_session_id; ensure metrics/health use that ID.
- [ ] Surface peer_session_id in host telemetry/logs for controller/sync frames.

Telemetry
- [ ] Counters: attach attempts/success/fail; offer/answer attempts/success/fail by peer_session_id; retry counts.
- [ ] Logs: attach IDs, offer acceptance, 409/Retry-After events.

Tests
- [x] Unit/integration (in-process stub): attach → offer/answer → data round-trip; no 404 path (webrtc_signaling_end_to_end updated).
- [x] Regression: offer before attach → 409/Retry-After then success after attach (webrtc_signaling_404_then_recovers).
- [ ] E2E: browser+manager use attach, single-channel WebRTC without fallback.
- [ ] Host telemetry/regression: host routes frames using peer_session_id and reports it.

History / current blockers
- Rust side: attach-first transport, auto-attach for legacy URLs, 404/409 retry, peer_session telemetry/logging in manager are done. Beach Road is attach-first and now reuses any existing host→peer_session mapping so both peers share the same peer_session_id (prevents split offer/409 loops).
- Browser path is in `apps/beach-surfer/src/transport/signaling.ts` and `webrtc.ts` (connectAnswerer, etc.); now attach-first. Rewrite-2 lacks signaling code.
- Host regression pending: need a controller-capable transport stub (IPC rejects namespaced frames); can use WebRTC test pair with send_namespaced once wired.

Implementation notes for Codex
- Touch points: Beach Road signaling REST/WS handlers, manager/browser connect flow, host handshake consumption.
- Remove any implicit session creation inside offer handlers; attach must be explicit and first.
- Keep it greenfield: delete legacy 404 path and unused fast-path logic.***
