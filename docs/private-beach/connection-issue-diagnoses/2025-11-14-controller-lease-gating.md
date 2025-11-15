# Controller Lease Expiration Drops Controller Actions (Nov 14, 2025)

## Context
- Showcase run with two Python paddle hosts plus the mock agent (session `47a4b064-eca7-4661-b72e-0fe2edcb145d`). Launch steps followed the snippets in `docs/helpful-commands/pong.txt`.
- Logs collected during the run:
  - Dashboard console: `temp/pong.log`.
  - Host CLI: `~/beach-debug/beach-host-agent.log`.
  - Manager tail inside Docker: `temp/manager-tail.log`.
  - WebRTC inspection: `temp/webrtc_internals_dump`.

## Evidence
1. **Hosts do auto-attach quickly once the manager handshake arrives.**  
   - The agent host registers at `17:14:37Z`, immediately starts its controller action consumer, then receives the auto-attach hint via the manager handshake around `17:15:29Z`. Two seconds later it logs `auto-attached via handshake` and `manager attach confirmed; controller action consumer starting` (`~/beach-debug/beach-host-agent.log:138805448-138805505`).  
   - Before the handshake lands the health reporter emits a few `session not found` 404s, which matches the new host-side trace logs we added in `apps/beach/src/server/terminal/host.rs` to measure attach latency.

2. **All three peer connections reach `connectionstatechange: "connected"` and announce the required data channels.**  
   - For PC `13-10` (LHS), the WebRTC internals dump shows the ICE state advancing to `"connected"` at `1763140503084.29` and the connection state following at `1763140504283.266`, followed by `datachannel` entries for both `beach` and `beach-secure-handshake` (`temp/webrtc_internals_dump:60-140`).  
   - PCs `13-11` (RHS) and `13-12` (agent) follow the same pattern later in the file (`temp/webrtc_internals_dump:170-320`, `temp/webrtc_internals_dump:330-420`). This confirms the fast_path transport itself was live during the stall.

3. **Dashboard traces show controller handshakes completing and leases being issued with 120 s TTLs.**  
   - For the RHS paddle, `beachConnectionLogger` records multiple `handshake:success` events between `17:15:17Z` and `17:15:48Z`, each with a `leaseExpiresAtMs` approximately two minutes in the future (`temp/pong.log:998-1929`).  
   - When the agent tile requests a controller lease we log the TTL explicitly: `controller-lease {phase:"request", ttlMs:120000}` and `expiresAtMs:1763140671469` (`temp/pong.log:1937-1944`). No follow-up lease renewals were logged before the manager started dropping commands.

4. **Despite the healthy transport, Beach Manager rejects every controller action because the lease has expired.**  
   - The new manager-side trace hooks (`apps/beach-manager/src/state.rs`) show repeated `target="controller.actions.drop"` warnings every ~15 s for session `1788671f-…` with `reason="missing_lease"`, `controller_token=6af4cb8f`, and `strict_gating=true`. These are visible in `temp/manager-tail.log:379`, `temp/manager-tail.log:392`, and `temp/manager-tail.log:399`.  
   - Each drop also records `attach_age_secs` in the 125–161 s range, which lines up with the 120 s lease TTL quoted above. Fast-path readiness is logged as `false` even though WebRTC is up because strict gating refuses to mark the transport ready without a fresh lease.

5. **Queue depth wobbles from the dashboard side because the UI keeps sending actions with the stale token.**  
   - Once the lease expires (~17:17:50Z) the dashboard still dispatches commands using the previous controller token (the prefix in the manager log remains `6af4cb8f`). There are no `controller-lease {phase:"success"}` entries after 17:15:48Z in `temp/pong.log`, so the client never rotates credentials even as it notices the manager dropping actions.

## Root Cause
Controller strict gating is doing its job: after ~120 s the manager invalidates the existing controller lease and mints a new one, but the dashboard never re-runs `issueControllerHandshake`/lease renewal for the affected tiles. With an expired lease the manager treats every action as unauthenticated (`lease_id="none"`), so it drops the payload before it reaches the host even though the host is attached and the fast-path data channels are healthy. This also explains the “wobble” seen in the UI: the tile oscillates between “connecting” and “connected” states as the viewer tears down and recreates transports trying to deliver actions that are immediately rejected.

## Mitigations / Next Steps
1. **Refresh controller leases proactively.** When the manager emits a new lease (or when 75% of the TTL has elapsed), re-run the handshake/lease flow so the dashboard always has a valid token before strict gating kicks in. The existing `controller-lease` traces in `temp/pong.log` make it easy to verify this once implemented.
2. **Handle `missing_lease` drops in the dashboard.** On a `412`/`missing_lease` response from `controller.actions`, immediately request a new lease instead of retrying with the stale token.
3. **Use the new trace hooks to monitor attach latency.** The throttled logs we added in `apps/beach/src/server/terminal/host.rs` and `apps/beach-manager/src/state.rs` now record (a) how long a host waits for attach approval, and (b) how long the manager holds a handshake before issuing a lease. Keep them enabled while iterating so we can tell whether future stalls are due to handshake delays or lease churn.
4. **Optional sanity test:** temporarily disable strict gating (`CONTROLLER_STRICT_GATING=false`) in a local run. If actions start flowing immediately, it confirms the only blocker was lease expiration.
