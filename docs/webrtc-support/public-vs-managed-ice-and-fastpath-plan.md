Public vs Managed WebRTC / ICE / Fast‑Path Plan
==============================================

Context
-------

We want `beach` to support two distinct usage modes:

- **Public mode** — the widely distributed, free binary:
  - Users do *not* log in.
  - The CLI must *only* talk to:
    - `beach-road` (session server / broker),
    - and whatever peers it connects to via WebRTC.
  - It must **never** call `beach-gate` or any other Beach service.
  - Public clients must **not** use TURN/coturn (we do not want to fund relay traffic for free users).
  - All ICE configuration in this mode must be local: env vars / config passed next to the binary.

- **Managed / logged‑in mode** — “Private Beach” / internal / paid scenarios:
  - Users log in via `beach-gate`.
  - The CLI is allowed to call `beach-gate` for auth and TURN credentials.
  - TURN/coturn is allowed and expected as a fallback.
  - We still want the same fast‑path and controller semantics so any app built on the harness “just works.”

In both modes we want:

- **Reliable fast‑path WebRTC** between:
  - `beach-manager` (often running in Docker), and
  - CLI hosts (local processes on the developer’s laptop), plus agents.
- **Robust controller queues**:
  - No perpetual Redis queue wedge (stuck pending entries).
  - Controller actions should flow from agent → manager → player hosts reliably.

This document lays out:

1. Goals and constraints.
2. Desired behavior in public vs managed mode.
3. Dev‑topology‑specific ICE configuration (manager in Docker vs all on host).
4. Concrete implementation steps in the Rust CLI (`beach`), manager (`beach-manager`), and docs/scripts.
5. Validation and smoke tests (including Pong).

The goal is that another Codex instance can implement everything end to end using only this doc plus the repo.


1. High‑Level Goals
-------------------

1. **Strict separation of modes**
   - Public mode:
     - No network calls to `beach-gate` or any other Beach service except `beach-road`.
     - No TURN usage (no allocation on `coturn`, no TURN credentials).
     - ICE configuration is purely local (env or config shipped with the binary).
   - Managed mode:
     - May talk to `beach-gate` for auth and TURN credentials.
     - May use both STUN and TURN, including the dev `coturn` instance.

2. **Deterministic, debuggable ICE behavior**
   - When fast‑path fails, logs should clearly state *why*:
     - No ICE servers configured.
     - Public mode: skipping TURN.
     - Failed to fetch TURN credentials (managed mode).
     - Only loopback/private candidates, no viable pairs, etc.
   - The same ICE configuration that passes `scripts/fastpath-smoke.sh` should make Pong (and other harness apps) work in dev.

3. **Robust fast‑path + controller delivery**
   - For any harness‑based session (Pong, Beach Surfer, etc.):
     - Manager↔host fast‑path (`mgr-actions` / `mgr-acks` channel) should be the primary controller delivery path whenever available.
     - Redis/HTTP should be a bounded fallback, not the steady‑state path.
     - Redis queues must not wedge with 500 pending items because acks never arrive or pending entries are never reclaimed.

4. **100% local dev reproducibility**
   - On a single laptop, we need a cookbook recipe that always works:
     - “Manager in Docker + hosts on Mac” topology.
     - “Everything on Mac” topology.
   - The ICE + fast‑path behavior should be identical across Pong and the dedicated smoke scripts.


2. Mode Semantics (Public vs Managed)
-------------------------------------

We introduce an explicit concept of **CLI mode** for `beach`:

- **Public mode** (default for widely distributed binary).
- **Managed mode** (when user has logged in and obtained a valid profile / tokens).

### 2.1 Mode detection and configuration

Implementation sketch in the CLI:

- Add a small helper in `apps/beach/src/auth/mod.rs` or a nearby module:
  - `fn is_public_mode() -> bool`:
    - Returns `true` if:
      - An env like `BEACH_PUBLIC_MODE=1` is set, *or*
      - No local profile / auth token is present (user never logged in).
    - Returns `false` when:
      - A valid profile/token for `beach-gate` exists (managed / logged‑in CLI).
  - This keeps behavior simple:
    - For development we can override with `BEACH_PUBLIC_MODE=0` if we want to force managed semantics even though a profile is missing.

- Mode will be consumed primarily by the WebRTC transport (`apps/beach/src/transport/webrtc/mod.rs`).

### 2.2 Public mode rules

When `is_public_mode() == true`:

1. **Network access constraints**
   - The CLI must **never**:
     - Call `beach-gate` for token exchange, entitlements, or TURN credentials.
     - Call any other Beach service except:
       - `beach-road` for:
         - Session creation / registration.
         - Any signaling APIs that are part of the public session protocol.
   - This should be enforced at the `auth` layer by:
     - Avoiding any `resolve_turn_credentials` calls.
     - Skipping authenticated endpoints entirely.

2. **TURN usage**
   - TURN is **disabled by design**:
     - No calls to `auth::resolve_turn_credentials`.
     - No parsing or usage of `BEACH_GATE_TURN_*` envs in the CLI.
   - Public users *can* still use STUN or TURN that *they* operate, but only via the generic `BEACH_ICE_SERVERS` override:
     - E.g. `export BEACH_ICE_SERVERS='[{"urls":["stun:stun.l.google.com:19302"]}]'`.
     - The CLI treats `BEACH_ICE_SERVERS` as opaque, local configuration; it does not contact Beach services.

3. **ICE server selection**
   - In `apps/beach/src/transport/webrtc/mod.rs::load_turn_ice_servers`:
     - First call `local_ice_servers_from_env()` (existing).
       - If this returns `Some(servers)` → use it and **do not** call Gate.
     - If `is_public_mode()` and `local_ice_servers_from_env()` returned `None`:
       - **Return `Ok(None)`** without any attempt to contact Gate.
       - Emit a single, high‑signal log message:
         - Target: `beach::transport::webrtc`
         - Message: `public mode: no ICE servers configured; using host/prflx candidates only; connectivity may be limited`.
     - This ensures public mode never talks to Gate, and never uses Beach‑hosted TURN.

4. **Candidate behavior**
   - With `Ok(None)` from `load_turn_ice_servers`, the WebRTC stack will use:
     - Host, prflx, and possibly srflx candidates, depending on OS.
   - This is acceptable in public mode; it might limit connectivity but avoids Beach‑funded TURN.

### 2.3 Managed mode rules

When `is_public_mode() == false` (user has logged in):

1. **Network access**
   - CLI may:
     - Call `beach-gate` for:
       - Device auth / token exchange.
       - Entitlements.
       - TURN credentials via `/turn/credentials`.
   - All existing managed/Private Beach flows remain allowed.

2. **TURN usage**
   - `load_turn_ice_servers()` behavior:
     - If `BEACH_ICE_SERVERS` is set (local override) → use it, skip Gate.
     - Else, call `auth::resolve_turn_credentials(None)`:
       - On success:
         - Use returned TURN servers as existing code does.
       - On `TurnNotEntitled` errors:
         - Propagate the error so caller can decide (e.g. bail or continue with no ICE servers).
       - On other errors:
         - Log a warning, return `Ok(None)` to allow fallback to host candidates.
   - This is already implemented; we just need to guard it behind `!is_public_mode()`.

3. **ICE server precedence**
   - Precedence order in managed mode:
     1. `BEACH_ICE_SERVERS` (explicit local override).
     2. TURN credentials from `beach-gate`.
     3. Fallback: none, i.e. host/prflx candidates (last resort).


3. Dev Topologies and ICE Configuration
---------------------------------------

We care about two dev topologies:

1. **Topology A: Manager in Docker; hosts on the laptop**
   - `beach-manager`, `beach-road`, `beach-gate`, `coturn` all run in Docker via `docker-compose.yml`.
   - CLI hosts and agents (Pong players, etc.) run on the host OS.

2. **Topology B: Everything on host**
   - `beach-manager`, `beach-road`, `beach-gate`, `coturn`, and CLI hosts all run directly on the laptop.

The ICE plan must work for both.

### 3.1 Manager configuration (common pieces)

The manager’s fast‑path WebRTC code lives in `apps/beach-manager/src/fastpath.rs`:

- `FastPathSession::new`:
  - Uses an explicit UDP port range from:
    - `BEACH_ICE_PORT_START`
    - `BEACH_ICE_PORT_END`
    - Via `EphemeralUDP` and `setting.set_udp_network(UDPNetwork::Ephemeral(ephemeral))`.
  - Optionally sets a NAT 1:1 IP via:
    - `BEACH_ICE_PUBLIC_IP` (preferred),
    - Or resolving `BEACH_ICE_PUBLIC_HOST` (default `host.docker.internal`).
  - Logs `public_ip_hint` and `host_hint` under target `fast_path.ice`.

In Docker (`docker-compose.yml`):

- `beach-manager` service:
  - Publishes UDP range: `62000-62100:62000-62100/udp`.
  - Sets env:
    - `BEACH_ICE_PUBLIC_IP: ${BEACH_ICE_PUBLIC_IP:-}`
    - `BEACH_ICE_PUBLIC_HOST: ${BEACH_ICE_PUBLIC_HOST:-host.docker.internal}`
    - `BEACH_ICE_PORT_START: ${BEACH_ICE_PORT_START:-62000}`
    - `BEACH_ICE_PORT_END: ${BEACH_ICE_PORT_END:-62100}`
  - Uses `scripts/docker/beach-manager-entry.sh` to auto‑resolve `BEACH_ICE_PUBLIC_IP` if unset.

On the host (`.env.local` in repo root):

- We currently default:
  - `BEACH_ICE_PUBLIC_IP=127.0.0.1`
  - `BEACH_ICE_PORT_START=62000`
  - `BEACH_ICE_PORT_END=62100`
  - This works with Docker’s `host-gateway` mapping when CLI hosts run on the same machine.

### 3.2 Topology A: Manager in Docker; hosts on laptop

For **reliable dev** in both public and managed mode:

1. **Manager side (Docker)**
   - Keep `docker-compose.yml` as is (UDP range + `extra_hosts: host.docker.internal:host-gateway`).
   - Ensure `.env.local` in repo root sets:
     - `BEACH_ICE_PUBLIC_IP=127.0.0.1`
     - `BEACH_ICE_PORT_START=62000`
     - `BEACH_ICE_PORT_END=62100`
   - This guarantees `FastPathSession::new` advertises `127.0.0.1:62xxx` candidates, which the host can reach via the published UDP ports.

2. **Host CLI side**
   - For both public and managed dev testing, we want a **canonical default**:
     - `export BEACH_ICE_SERVERS='[{"urls":["stun:host.docker.internal:3478","stun:127.0.0.1:3478"]}]'`
   - This matches `scripts/fastpath-smoke.sh` “local” mode and uses the dev `coturn` container *as STUN only*.
   - In **public mode**, this is fully allowed:
     - The STUN configuration is purely local; the CLI never calls Gate.
   - In **managed mode**, we can:
     - Leave `BEACH_ICE_SERVERS` unset to exercise the full TURN path via Gate, *or*
     - Keep it set to ensure consistent STUN behavior for debugging.

3. **Behavior summary**
   - Public mode dev:
     - CLI uses `BEACH_ICE_SERVERS` → STUN only → no Gate.
     - Manager advertises `127.0.0.1:62xxx` via NAT 1:1.
   - Managed mode dev:
     - Preferred path: unset `BEACH_ICE_SERVERS`, fetch TURN via Gate, use `coturn`.
     - For local debugging: set `BEACH_ICE_SERVERS` to the same STUN list as above.

### 3.3 Topology B: Everything on host

When `beach-manager`, `beach-road`, `beach-gate`, `coturn`, and the CLI all run on the host:

1. **Manager**
   - Run with:
     - `BEACH_ICE_PUBLIC_IP=127.0.0.1`
     - `BEACH_ICE_PORT_START`/`END` same as Docker defaults (optional but keeps logs consistent).

2. **CLI hosts**
   - For public mode:
     - Option 1: no `BEACH_ICE_SERVERS` → host/prflx candidates only.
     - Option 2 (recommended for reliability): same `BEACH_ICE_SERVERS` override as above, pointing to:
       - `stun:127.0.0.1:3478`.
   - For managed mode:
     - Either rely on Gate for TURN, or reuse the same `BEACH_ICE_SERVERS` override.


4. Changes in the CLI (`beach`)
-------------------------------

The key code lives in:

- `apps/beach/src/auth/mod.rs` (or nearby modules):
  - Mode detection and auth/profile handling.
- `apps/beach/src/transport/webrtc/mod.rs`:
  - ICE server resolution: `local_ice_servers_from_env`, `load_turn_ice_servers`.
  - TURN credential usage via `auth::resolve_turn_credentials`.

### 4.1 Implement mode detection

1. Add a helper (`auth` layer):

```rust
pub fn is_public_mode() -> bool {
    if std::env::var("BEACH_PUBLIC_MODE").ok().as_deref() == Some("0") {
        return false;
    }
    if std::env::var("BEACH_PUBLIC_MODE").ok().as_deref() == Some("1") {
        return true;
    }
    // Fallback: treat “no profile / no login” as public.
    !has_valid_profile()
}
```

- `has_valid_profile()` should be whatever existing logic we use to decide whether the CLI can talk to Gate.
- This ensures:
  - Devs can explicitly toggle with `BEACH_PUBLIC_MODE`.
  - End users default to public semantics until they log in.

2. Expose `is_public_mode()` so `transport::webrtc` can call it.

### 4.2 Gate calls and TURN in public mode

In `apps/beach/src/transport/webrtc/mod.rs`:

1. Update `load_turn_ice_servers()`:

   - Pseudocode:

   ```rust
   async fn load_turn_ice_servers() -> Result<Option<Vec<RTCIceServer>>, AuthError> {
       if let Some(servers) = local_ice_servers_from_env()? {
           return Ok(Some(servers));
       }

       if auth::is_public_mode() {
           tracing::info!(
               target: "beach::transport::webrtc",
               "public mode: no BEACH_ICE_SERVERS configured; skipping TURN; using host/prflx candidates only"
           );
           return Ok(None);
       }

       // existing resolve_turn_credentials logic for managed mode...
   }
   ```

   - This guarantees:
     - In public mode, the CLI never calls Gate for TURN.
     - TURN is only ever fetched when not in public mode.

2. Add a one‑time log at session creation:
   - When we first construct a WebRTC peer connection for a session, log:
     - Mode (`public` vs `managed`).
     - Whether we are using:
       - `BEACH_ICE_SERVERS` override,
       - Gate‑provided TURN,
       - or host/prflx only.

### 4.3 Optional: hard guardrails

To be extra safe:

- If `auth::is_public_mode()` is true:
  - Any attempt to call Gate (e.g. a function that uses `BEACH_GATE_URL`) should:
    - Either refuse at compile‑time (not wired in), or
    - Return an error (`PublicModeForbidden`) that is logged once and then ignored.

This ensures future changes don’t accidentally reintroduce Gate calls in public mode.


5. Manager and Fast‑Path Behavior
---------------------------------

Most manager fast‑path and controller behavior already exists. We need to ensure it plays nicely with the ICE plan and doesn’t wedge Redis.

Key files:

- `apps/beach-manager/src/fastpath.rs`
- `apps/beach-manager/src/state.rs`

### 5.1 Fast‑path ICE logging

In `FastPathSession::new` (already partially done):

- Ensure we log:
  - `public_ip_hint` (chosen NAT 1:1 IP).
  - `BEACH_ICE_PORT_START`/`END` (if set).
  - Any failures to resolve `BEACH_ICE_PUBLIC_HOST`.
- When ICE connection state transitions to `failed`:
  - Log the last few candidate meta entries we saw (using existing `parse_candidate_meta` helpers).
  - This makes it easy to see when the candidate set is unusable.

This is mostly logging; behavior is already correct with respect to envs.

### 5.2 Controller queues and fast‑path

In `apps/beach-manager/src/state.rs`:

- `queue_actions` and `send_actions_over_fast_path` decide whether to:
  - Use fast‑path (mgr‑actions/mgr‑acks).
  - Enqueue into Redis for HTTP delivery.

We want:

1. **Sessions with an active harness fast‑path session**:
   - `queue_actions` should:
     - Attempt `send_actions_over_fast_path`.
     - On success → **do not** enqueue into Redis.
     - On `SessionMissing` / `ChannelMissing` → log at debug level, then fall back to Redis.

2. **Redis recovery for stuck queues**:
   - In `drain_actions_redis`:
     - Today we see logs like:
       - `drain_actions_redis returned no actions despite non-empty stream ... queue_depth=500; fast_path_ready=false`.
     - Implement a recovery path:
       - When `XLEN > 0` but `XREADGROUP ... '>'` returns nil repeatedly:
         - Use `XPENDING` + `XCLAIM` (or `XREADGROUP` from `"0"`) to reclaim a bounded batch of pending entries.
         - Deliver them and ack via `ack_actions_redis`.
       - Put a hard cap per poller iteration (e.g. 100 entries) to avoid starving other work.
   - This ensures a previous failed run doesn’t leave streams stuck forever.

3. **HTTP polling gating**
   - Hosts should not indefinitely pause HTTP controller polling when fast‑path is failing.
   - In the host/harness (see `crates/beach-buggy` and CLI host code):
     - Only pause HTTP when:
       - Fast‑path is active *and* queue depth is zero.
     - If fast‑path fails:
       - Resume HTTP polling promptly so actions can still be drained and acked.

Most of this is already in place; we just need to ensure the logic is aligned with the new ICE behavior and mode semantics.


6. Docs and Script Updates
--------------------------

To make this plan usable by developers and future tools, we need to update documentation and helper scripts.

### 6.1 Pong helper (`docs/helpful-commands/pong.txt`)

Update Pong commands so they:

- Explicitly set `BEACH_ICE_SERVERS` for hosts and agent during local dev:

```bash
export BEACH_ICE_SERVERS='[{"urls":["stun:host.docker.internal:3478","stun:127.0.0.1:3478"]}]'
```

- Do *not* export any `BEACH_GATE_*` envs or tokens.
- Optionally set `BEACH_PUBLIC_MODE=1` when you want to simulate a public user:
  - This ensures the CLI behaves as if it has no access to Gate/TURN.

### 6.2 fastpath-smoke.sh enhancements

In `scripts/fastpath-smoke.sh`:

- Ensure the `local` mode already:
  - Sets `BEACH_ICE_SERVERS` (it does).
  - Verifies `using ICE servers from BEACH_ICE_SERVERS` appears in the host log.

- Add a new mode `public`:
  - Launch the host with:
    - `BEACH_PUBLIC_MODE=1`.
    - The same `BEACH_ICE_SERVERS` override as `local`.
  - Assert:
    - `fast path controller channel ready` shows up.
    - No log lines indicating TURN credentials fetched from Gate.

- Optionally add a `managed` mode:
  - Unset `BEACH_ICE_SERVERS`.
  - Ensure:
    - `using TURN credentials from Beach Gate` appears in logs.
    - Fast‑path comes up.

This gives us three pinned configurations that we can test with CI or ad‑hoc.

### 6.3 New ICE dev doc

Add a short, user‑facing doc (e.g. `docs/webrtc-support/dev-ice-setup.md`) describing:

- How to run the stack in Docker.
- Environment variables to set for manager (`.env.local`) and hosts (`BEACH_ICE_SERVERS`, `BEACH_PUBLIC_MODE`).
- How to run:
  - `scripts/fastpath-smoke.sh` in `local`, `public`, and `managed` modes.
  - The Pong showcase end to end.

(This current file is the design/spec; the dev doc can be a condensed “how‑to.”)


7. Implementation Checklist
---------------------------

This section is a step‑by‑step list another Codex instance can follow.

### 7.1 CLI (`beach`)

1. In `apps/beach/src/auth/mod.rs` (or similar):
   - Implement `fn is_public_mode() -> bool` as described in 4.1.
   - Implement `fn has_valid_profile() -> bool` if it doesn’t already exist, or reuse existing logic for “logged in vs not.”

2. In `apps/beach/src/transport/webrtc/mod.rs`:
   - Import and use `auth::is_public_mode()`.
   - Update `load_turn_ice_servers()`:
     - Use `local_ice_servers_from_env()` as first priority.
     - If `is_public_mode()` and no local override:
       - Log once and return `Ok(None)` without calling Gate.
     - Otherwise, call `auth::resolve_turn_credentials` as today.
   - Add a log at peer connection setup indicating:
     - Mode: `public` vs `managed`.
     - ICE config source: `BEACH_ICE_SERVERS`, `TURN`, or `host/prflx only`.

3. (Optional) Add a central guard in `auth` or `client`:
   - If `is_public_mode()` is true, any function that would call Gate returns `PublicModeForbidden` instead.

### 7.2 Manager (`beach-manager`)

1. `apps/beach-manager/src/fastpath.rs`:
   - Ensure `FastPathSession::new` logs:
     - `public_ip_hint`.
     - `BEACH_ICE_PORT_START`/`END` (if present).
   - On ICE state `failed`, log candidate meta derived from `parse_candidate_meta`.

2. `apps/beach-manager/src/state.rs`:
   - In `queue_actions`:
     - Confirm that:
       - Successful fast‑path delivery prevents Redis enqueue.
       - `SessionMissing`/`ChannelMissing` are logged at debug and fall back to Redis.
   - In `drain_actions_redis`:
     - Implement recovery for stuck pending entries (bounded `XPENDING`/`XCLAIM` pass).
   - In ack and metrics functions:
     - Keep queue depth metrics accurate so devs can see when Redis is being used as the steady‑state path vs fallback.

### 7.3 Scripts and docs

1. `docs/helpful-commands/pong.txt`:
   - Add `BEACH_ICE_SERVERS` export near the Pong commands.
   - Mention `BEACH_PUBLIC_MODE=1` for public‑mode simulations.

2. `scripts/fastpath-smoke.sh`:
   - Add `public` (and optionally `managed`) modes as described.
   - Add checks for absence/presence of Gate/TURN logs.

3. New short doc:
   - `docs/webrtc-support/dev-ice-setup.md`:
     - Summarize environment variables and commands to:
       - Start the Docker stack.
       - Run `fastpath-smoke.sh` in multiple modes.
       - Run Pong end to end.


8. Validation Strategy
----------------------

After implementing the above:

1. **Public mode validation**
   - Run `scripts/fastpath-smoke.sh` in `public` mode:
     - `BEACH_PUBLIC_MODE=1`.
     - `BEACH_ICE_SERVERS` set as in 3.2.
   - Verify:
     - `fast path controller channel ready` appears for the test session.
     - No logs in host indicating TURN credentials were fetched from Gate.
     - No HTTP requests from the CLI to Gate in logs or via `docker logs beach-gate`.

2. **Managed mode validation**
   - Log in to Gate (existing flow).
   - Run `scripts/fastpath-smoke.sh` in `gate`/`managed` mode:
     - `BEACH_PUBLIC_MODE` unset.
     - `BEACH_ICE_SERVERS` unset.
   - Verify:
     - Fast‑path channel becomes ready.
     - Host logs contain `using TURN credentials from Beach Gate`.

3. **Pong showcase**
   - Start Docker stack with manager, road, gate, coturn.
   - Use the updated Pong commands (`docs/helpful-commands/pong.txt`):
     - Ensure `BEACH_ICE_SERVERS` exported.
     - Optionally set `BEACH_PUBLIC_MODE=1` to simulate public usage.
   - Attach player sessions and agent in the Private Beach UI.
   - Verify:
     - Pong paddles move.
     - Manager logs show fast‑path controller channel established.
     - Redis controller queues do not grow unbounded; no repeated “drain_actions_redis returned no actions despite non-empty stream” warnings.

4. **Regression checks**
   - Ensure existing flows (e.g. Beach Surfer, terminal sessions) still work in both:
     - Public mode with no TURN (host + STUN only).
     - Managed mode with full TURN.

Once these validations pass, we will have:

- A clear separation between public and managed WebRTC behavior.
- Deterministic, debuggable ICE configuration for laptop dev.
- A solid foundation for future fast‑path/harness work without re‑introducing Gate dependencies into public mode.

