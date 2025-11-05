# Prompt: Investigate Host Auto-Approval Failure

## Context
- Private Beach rewrite tile connecting to host session `f7ef78bd-23e5-45c5-ae2d-5d8d2b1ff38f`.
- Host CLI launched with default dev settings (`cargo run`), expected to auto-approve viewers.
- Viewer tile shows blank terminal; log reveals it remains in `waiting` state for host approval despite auto-approve expectation.
- Relevant logs: `temp/private-beach-rewrite-2.log` around lines ~520-620 show join-state stuck on “Connected - waiting for host approval…” and `join-state-apply` cycling hints.
- Viewer snapshot stays `rows:0`, `viewportHeight:0`; no terminal frames consumed.
- Auto-approval previously worked but has regressed intermittently.

## Task
Deep dive into why auto-approval isn’t firing and the viewer remains blocked:
- Confirm current auto-approval configuration on host (look at `JoinAuthorizer` setup, flags/env vars, CLI args).
- Trace the control path in `apps/beach/src/server/terminal/host.rs` + `authorization.rs` to see under what conditions `authorizer.should_emit_pending_hint()` / `authorize()` returns false.
- Identify whether rewrite flow is missing a parameter (metadata, label) that the authorizer requires.
- Check for regressions in manager/connection service ensuring approval signals (`beach:status:approval_granted`) reach the viewer.
- Deliver actionable fix or clear reproduction notes.

## Repro Steps
1. `cargo run` from repo root (dev profile).
2. Open `http://localhost:3003/beaches/<id>` and add Private Beach rewrite tile.
3. Attach session using provided share URL (`127.0.0.1:4132/...`, passcode `7T8IB5`).
4. Observe tile stuck on waiting overlay / blank content; host terminal still sitting in interactive approval prompt even though auto-approve expected.

## Artifacts
- `temp/private-beach-rewrite-2.log` (latest run capturing waiting state).
- Host log snippet (session start & prompt output included above in CLI context).

## Goal
Determine root cause of missing auto-approval and propose fix or mitigation so rewrite tiles auto-approve like legacy flow.
