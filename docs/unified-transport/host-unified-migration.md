# Host/Harness Unified Transport Migration Plan

This doc captures the remaining work to finish Milestones 4–5 on the host side:
move Beach Buggy onto the negotiated transport, scope extension subscriptions,
and decommission legacy fast-path peers/endpoints. It is written as a short,
executable checklist so the next pass can implement without re-discovery.

## Goals
- Expose the negotiated transport (WebRTC/WebSocket/IPC) to Beach Buggy and use
  `UnifiedBuggyTransport` by default for actions/acks/state/health.
- Keep HTTP + legacy fast-path as fallback during rollout; prefer unified when
  available.
- Scope extension subscriptions so viewers/controllers only consume allowed
  namespaces; add tracing/metrics for extension routing.
- Remove the extra `/fastpath` REST endpoints and data channels once unified
  paths are stable; update smoke tests.

## Work items

### 1) Host: attach Beach Buggy to unified transport
- In `apps/beach/src/server/terminal/host.rs`, when controller negotiation
  yields a `NegotiatedSingle { transport, metadata, … }`, construct
  `UnifiedBuggyTransport::new(transport.clone())` and pass it into the harness
  instead of `HttpTransport` when possible.
- Keep existing HTTP/legacy fast-path code as fallback:
  - If unified construction fails or transport is missing, fall back to existing
    HTTP + fast-path setup.
  - Gate on controller capability: if manager does not support extensions,
    stay on legacy.
- Ensure state/health publishers and idle snapshot workers prefer the unified
  bridge (send state/health over extensions) and only use HTTP when unified is
  unavailable.

### 2) Controller action ingress/egress
- Actions: route host → manager via `UnifiedBuggyTransport::ack_actions` and
  map manager → host via `UnifiedBuggyTransport::receive_actions` instead of
  HTTP poll when unified is enabled.
- Acks/state/health: ensure the host consumes `fastpath` extension frames via
  `subscribe_extensions("fastpath")` so the Bridge receives them without a
  data-channel.

### 3) Viewer/namespace scoping
- Add an extension registry on the host side (or reuse manager’s ACL model) so
  viewers only subscribe to whitelisted namespaces. For now:
  - Controllers: subscribe to `fastpath`.
  - Viewers: subscribe to none (ignore extensions).
- Add tracing + counters:
  - `transport.extension.received{namespace,kind,role}`
  - `transport.extension.dropped{namespace,reason}`

### 4) Legacy removal (Milestone 5)
- Delete host-side fast-path peer creation (`mgr-actions`, `mgr-acks`,
  `mgr-state`) and `/fastpath/...` REST endpoints once unified/ext path is
  stable.
- Remove `FastPathSession` plumbing in manager and the controller forwarder
  upgrade probes.
- Clean up metrics and docs referencing fast-path channels.

### 5) Testing/rollout
- Add a toggle/env (temporary) to force legacy or unified for A/B during
  rollout, then remove once stable.
- Run deferred smoke tests with unified on/off:
  - `scripts/pong-fastpath-smoke.sh --skip-stack`
  - `scripts/fastpath-smoke.sh`
- Verify regression risk: controller actions arrive, acks persist, state diffs
  flow, health publishes reach manager.

## Prompt / next steps to execute
1. Wire `UnifiedBuggyTransport` into `apps/beach/src/server/terminal/host.rs`
   at transport negotiation time, falling back to HTTP/legacy fast-path when
   unified is unavailable.
2. Update state/health publishers to prefer unified extension sends, with HTTP
   fallback.
3. Add extension subscription scoping + metrics on host/client, then remove the
   fast-path peer/endpoint plumbing once unified passes smoke.
4. Run the smoke scripts in unified and legacy modes and update
   `docs/unified-transport/plan.md` with test results before removing the toggle.
