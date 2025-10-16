# Multi-Offerer Remediation Plan – 2025-10-02

authored by Codex

## Problem Statement
When a beach host is already running an offerer supervisor, additional session participants are still acquiring WebRTC offers and acting as offerers. When a browser viewer joins after a CLI client, Beach Road stores multiple `/webrtc/offer` blobs targeting the browser’s peer ID. The browser can pick the participant’s offer first, completing a handshake that the host’s negotiator never sees. The host eventually tears down its peer connection because the `__ready__` sentinel never arrives, which closes the browser’s data channel a fraction of a second after it opened.

This occurs because the client code still runs `OffererSupervisor::connect` for the participant role. The recent guard in `negotiate_transport` relies on `SessionHandle::role()`, but for joining clients the handle incorrectly reports `SessionRole::Host`, so offerer mode remains enabled everywhere.

## Goals
- Guarantee that only the true host process spins up the offerer supervisor.
- Ensure all non-host participants act purely as answerers and decline any `/webrtc/offer` requests.
- Preserve existing multi-viewer support once the host alone negotiates and fan-outs transports.

## Proposed Fix Plan

1. **Correct Session Role Attribution**
   - Audit `SessionManager::join` and any other code constructing `SessionHandle` instances to confirm we always mark joined participants as `SessionRole::Participant`.
   - Add logging/assertions when constructing a handle for visibility (`session_id`, `role`).
   - Update `negotiate_transport` to branch explicitly on the role: participants iterate only answerer offers, host evaluates offerer first. Add telemetry warning if a participant ever encounters a `role="offerer"` offer.

2. **Harden OffererSupervisor Entry Points**
   - Introduce an explicit `HostOfferer` guard (e.g., a newtype wrapper or function) so only call sites that already validated the host role can invoke `OffererSupervisor::connect`.
   - Consider feature gating the supervisor behind a command-line flag the CLI host sets; participants would not pass it.

3. **Reject Participant-Originated Offers**
   - In `OffererInner::handle_peer_join`, detect if the signaling client is running in participant mode. If so, refuse to enqueue negotiators regardless of incoming `RemotePeerEvent::Joined` notifications.
   - Optionally, emit a warning when we observe a participant-sourced offer in Beach Road to catch regressions.

4. **Integration Test Coverage**
   - Extend an integration test that launches a host + CLI viewer + browser (headless mocked) asserting only the host posts `/webrtc/offer` entries.
   - Add a regression test to ensure the participant role never adds entries to Beach Road’s offer store.

5. **Monitoring & Telemetry**
   - Log when we register a negotiator along with `session_role` to confirm only the host path emits those entries.
   - Add metrics for “offers posted per peer” tagged with role to catch future regressions quickly.

## Acceptance Criteria
- With the CLI host, CLI viewer, and browser viewer scenario, Beach Road records only one offer per browser peer (originating from the host).
- Browser data channel remains open beyond `__ready__` and the host logs “offerer transport established” for the browser peer.
- Participants never invoke `OffererSupervisor::connect`; relevant code paths panic or error if they do.
- Regression tests covering mixed-role negotiation pass.

