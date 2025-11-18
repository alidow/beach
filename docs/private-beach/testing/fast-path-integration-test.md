# Fast-Path Integration Test Spec

Runs the full Pong stack headlessly (no browser) to catch regressions where controller fast-path channels churn or fall back to HTTP. The harness lives entirely inside Docker so Codex or CI can run it without external tools.

## Goals

- Prove each controller session (agent tile → lhs/rhs players) establishes exactly one `mgr-actions` channel and stays on fast path.
- Detect manager-side timeouts / fallbacks (`timed out waiting for controller data channel`, repeated `poller started (fallback)` in agent log).
- Surface `/sessions/.../webrtc/offer?... 404` storms without needing Chrome DevTools.

## Environment

1. `docker compose up beach-manager beach-road beach-gate coturn redis postgres` (reuse existing compose file). Mount a writable volume for `/tmp/pong-stack` and `logs/beach-manager` so the host harness can read logs.
2. Export deterministic env before launching the test harness:
   - `BEACH_TEST_MODE=1`
   - `BEACH_ICE_PUBLIC_IP` / `BEACH_ICE_PUBLIC_HOST` (same as local dev; derive via `hostname -I | awk '{print $1}'`).
   - `CLERK_TEST_TOKEN` (optional; pong-stack already synthesizes mock Clerk codes).
3. Run `./apps/private-beach/demo/pong/tools/pong-stack.sh start <fixture-id>` inside the beach-manager container. Fixture ID can be any UUID; the script prints the lhs/rhs/agent session IDs deterministically.

## Harness Flow

1. **Spin up stack** – issue the compose + pong-stack commands from a controlling script (Python or Rust) using `docker compose exec beach-manager ...`. Capture stdout to learn the session IDs/passcodes.
2. **Stabilize** – sleep ~20 s to allow the controller to acquire leases and push a handful of actions.
3. **Collect artifacts** – copy the following back to the host (via `docker compose cp`):
   - `/tmp/pong-stack/beach-host-{lhs,rhs,agent}.log`
   - `/tmp/pong-stack/agent.log`
   - `/tmp/pong-stack/player-{lhs,rhs}.log` (optional sanity check)
   - `logs/beach-manager/beach-manager.log`
4. **Analyze** – run regex-based assertions (see below). Emit offending log snippets if any check fails.
5. **Teardown** – `pong-stack.sh stop` followed by `docker compose down` (or leave infra running if other tests depend on it).

## Assertions

1. **Single fast-path channel per session**
   - Host logs must contain exactly one `fast path controller channel ready` entry per session with `client_label="mgr-actions"` and no repeated entries unless the handler actually disconnects. Fail if any `fast path controller channel ready ... client_label="pb-controller"` occurs after a `mgr-actions` entry (indicates fallback).

2. **No manager timeouts**
   - `logs/beach-manager/beach-manager.log` must not contain `timed out waiting for controller data channel`. Allow a short grace period before the first success (e.g., ignore entries within the first 5 s of stack launch) but fail otherwise.
   - Likewise, `controller forwarder already running; skipping spawn` is informational; only the timeout log line is a hard failure.

3. **Stable agent transport**
   - In `/tmp/pong-stack/agent.log`, count `poller started (fallback)` and `fast-path restored` lines per session. Fail if more than one fallback occurs after the initial attach.
   - Also scan for `readiness blocked for rhs/lhs: transport fast_path_unavailable`; persistent spam indicates the controller never reached fast path.

4. **No `/webrtc/offer 404` storms**
   - Parse `logs/beach-manager/beach-manager.log` for `GET /sessions/.../webrtc/offer ... 404` (Beach Road logs these) or directly run the harness’s `scripts/cdp-read-console.js` against `private-beach` if we hook it up later. For the initial test, grepping manager logs for `"/webrtc/offer" 404` is sufficient.

5. **Action throughput sanity check**
   - Ensure `agent.log` reports at least `N` `ACTION [sent]` events (e.g., `N=5`) for each player to prove inputs flowed over the chosen transport.

## Implementation Notes

- Write the harness under `tests/integration/fast_path.rs` (Rust) or `scripts/fast-path-integration.py`. Use `Command::new("docker")` to orchestrate compose/pong-stack.
- Add a `make fastpath-integration` target that sets env vars, launches the harness, and prints a concise pass/fail summary.
- Expose the harness via CI by adding a GitHub Actions step (behind a label like `FASTPATH_INTEGRATION=1`) so we can opt-in without slowing every push.
- Keep log parsing simple: use `regex` crate or Python’s `re` to count matches and surface the offending lines in the failure output.

## Future Enhancements

- Instead of sleeping a fixed time, poll the Beach Manager API (`/private-beaches/<pb-id>/controller-pairings`) to confirm the controller pipeline is active before collecting logs.
- Record additional metrics (latency histograms, fast-path ack counts) to chart regressions over time.
- Extend the harness to simulate multiple private beaches concurrently to stress the fast-path registry.

With this spec in place, Codex (or CI) can replicate the complex multi-host scenario headlessly, making it far easier to diagnose regressions without manual browser steps.
