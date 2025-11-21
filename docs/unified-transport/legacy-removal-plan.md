# Legacy Fast-Path Decommission Plan (Milestone 5)

This breaks down the remaining work to finish unified transport rollout and
remove the legacy `/fastpath` stack while retaining a safe fallback during
validation.

## Scope
- Host/harness: prefer unified transport (`UnifiedBuggyTransport`) for actions,
  acks, state, and health; fall back to HTTP/legacy fast-path when extensions
  are unsupported or fail.
- Manager: rely on extension routing for controller traffic; remove legacy
  fast-path peers/endpoints once unified is stable.
- Observability: add metrics/logs for extension routing and fallback decisions.
- Testing: run smoke tests in unified and legacy modes before removing the
  fallback.

## Work items

### A) Host/harness switch-over
- In `apps/beach/src/server/terminal/host.rs`:
  - After `NegotiatedSingle` is returned, construct
    `UnifiedBuggyTransport::new(transport.clone())` when the manager advertises
    extension support; pass it to the harness.
  - Keep the existing HTTP + fast-path code as fallback when extensions are
    absent or the unified bridge errors.
  - Ensure state/health publishers and idle snapshots prefer extension sends
    (fallback to HTTP).
- Add temporary env/toggle to force legacy vs unified for A/B and remove it
  after validation.

### B) Manager/controller/viewer scoping
- Controllers: subscribe to `fastpath` via `transport.subscribe_extensions` and
  route ack/state/health through existing handlers (already partially done).
- Viewers: do *not* subscribe to `fastpath`; ignore unknown namespaces.
- Add metrics/logs:
  - `transport.extension.received{namespace,kind,role}`
  - `transport.extension.dropped{namespace,reason}`
  - `transport.extension.fallback{reason}` when falling back to HTTP/legacy.

### C) Legacy removal
- Host: delete fast-path peer creation (`mgr-actions`, `mgr-acks`, `mgr-state`)
  and `/fastpath/...` REST plumbing once unified passes smoke.
- Manager: remove `FastPathSession` and controller forwarder upgrade probes;
  clean up metrics/docs referencing fast-path channels.

### D) Testing
- Run with unified *on*:
  - `scripts/pong-fastpath-smoke.sh --skip-stack`
  - `scripts/fastpath-smoke.sh`
- Run with unified *off* (legacy fallback) to ensure compatibility.
- Record artifact paths and update `docs/unified-transport/plan.md` milestone
  status after each run.

## Execution order (suggested)
1) Implement host-side switch-over with fallback + metrics.
2) Confirm controller ack/state/health flow via extensions in dev (manual logs).
3) Add viewer scoping + metrics.
4) Run smoke tests (on/off).
5) Remove legacy peers/endpoints and toggles; rerun smoke; update docs.
