# Pong stack WebRTC / fallback diagnosis (2025‑11‑28, bc4c51ba‑2004‑4e6a‑9953‑cbf31a83f0f6)

## Scope

This doc analyzes the 2025‑11‑28 `pong-stack.sh start bc4c51ba-2004-4e6a-9953-cbf31a83f0f6` run with:

- LHS session: `675c8415-7523-40a4-a6fb-93727d729e55`
- RHS session: `54e4b84c-28af-4a10-9654-1f35ad8c9d61`
- Agent session: `0fa327c1-c8d0-4489-8ab4-322288c26021`

Goal: explain why manager↔host and browser↔host connections end up on HTTP fallback despite apparently healthy WebRTC, and identify concrete next‑run instrumentation to pinpoint the exact root cause.

Sources examined:

- Manager logs: `logs/beach-manager/beach-manager.log`
- Browser connector logs: `temp/pong.log`
- webrtc‑internals export: `temp/webrtc-internals.log` (older run; used only for baseline RTT context)
- Host logs from inside `beach-manager` container:
  - `/tmp/pong-stack/20251128-085811/beach-host-lhs.log`
  - `/tmp/pong-stack/20251128-085811/beach-host-rhs.log`
  - `/tmp/pong-stack/20251128-085811/beach-host-agent.log`
  - `/tmp/pong-stack/20251128-085811/player-lhs.log`
  - `/tmp/pong-stack/20251128-085811/player-rhs.log`
  - `/tmp/pong-stack/20251128-085811/agent.log`
- Mirrored host logs on the host: `temp/beach-host-*-20251128.log`
- `temp/docker-beach-manager-20251128.log` (docker logs mirror) where helpful

I explicitly searched all of the above for:

- Session IDs, peer_session IDs, and private beach ID
- ICE connection state transitions
- WebRTC negotiation failures
- Data channel open/close events
- Controller forwarder / delivery / ack logs
- Connector `connector.transport:update` and `connector.action:*` events

No potentially relevant log stream was left uninspected.

---

## Environment / config sanity

All three hosts (LHS, RHS, agent) were launched from inside `beach-manager` with consistent ICE/TURN hints:

- `BEACH_ICE_PUBLIC_IP=192.168.68.52`
- `BEACH_ICE_PUBLIC_HOST=192.168.68.52`
- `BEACH_ICE_PORT_START=62500`, `BEACH_ICE_PORT_END=62600`
- `BEACH_GATE_TURN_URLS=turn:192.168.68.52:3478`

Evidence (example, LHS host inside container: `/tmp/pong-stack/20251128-085811/beach-host-lhs.log`):

- Logs: `using NAT 1:1 hint for WebRTC ... nat_ip=192.168.68.52 nat_hint_source=EnvIp`
- Logs: `using ICE UDP port range from env ... port_start=62500 port_end=62600`
- Logs: `using TURN credentials from Beach Gate realm=turn.private-beach.test ttl_seconds=86400 server_count=1`

Manager env (from `logs/beach-manager/beach-manager.log: config` and container env) is consistent:

- `beach_road_url=http://beach-road:4132`
- `public_manager_url=http://127.0.0.1:8080`
- `BEACH_GATE_TURN_URLS=turn:192.168.68.52:3478`

This means the earlier diagnosis where TURN/ICE pointed at `127.0.0.1` inside the container network is no longer true for this run. All services are advertising the host LAN IP and the same UDP range.

Conclusion: this run is not suffering from the earlier TURN/ICE misconfiguration. Any remaining failures are higher up in the control/ack logic, not basic candidate reachability.

---

## Browser ↔ host paths

### LHS viewer (browser ↔ LHS host)

Host (inside `beach-manager`):

- 13:58:51.551Z  
  `offerer observed peer join ... role=Client label="private-beach-dashboard" ... session_id="675c8415-..."`  
  (`/tmp/pong-stack/20251128-085811/beach-host-lhs.log:978`)

- 13:58:52.150Z  
  `waiting for datachannel to open (15s timeout) ... label=private-beach-dashboard`  
  (`beach-host-lhs.log:1038`)

- 13:58:53.233Z  
  `primary data channel opened ... label=private-beach-dashboard`  
  `handshake data channel opened ... label="beach-secure-handshake"`  
  (`beach-host-lhs.log:1090–1091`)

- 13:58:53.234–.295Z  
  `registering data channel handler ... transport_id=5`  
  `data channel created ... label=beach peer=TransportId(6)`  
  `input listener started ... transport=WebRtc client_label="private-beach-dashboard" ...`  
  (`beach-host-lhs.log:1092–1142`)

There are no WebRTC negotiation failures or data channel errors for `private-beach-dashboard` in the LHS host log after this point. The session continues to send/receive frames.

Browser connector (`temp/pong.log`) for LHS session `675c8415-...`:

- All `connector.action:*` lines for this LHS session use `detail.status:"http_fallback"` and `detail.transport:"http_fallback"`; all `connector.transport:update` entries show `transport:"http_fallback"` with the error `"no action acknowledgements for >1.5s; treating as stalled"`.
- There are **no** entries with `transport:"webrtc"` or `transport:"Rtc"` for this session.

Interpretation:

- The **data plane** between the browser viewer and the LHS host is WebRTC and is healthy from the host’s perspective.
- The **connector/control plane** that the browser logs as `transport` is tracking the *controller* path (actions via manager), not the viewer data channel. That path is in HTTP fallback for LHS by the time the connector reports state.

### RHS viewer (browser ↔ RHS host)

Host (inside `beach-manager`):

- 13:59:01.610Z  
  `offerer: creating handshake data channel ...` (new viewer offer)

- 13:59:03.211Z  
  `primary data channel opened ... label=private-beach-dashboard`  
  `handshake data channel opened ... label="beach-secure-handshake"`  
  `registering data channel handler ... transport_id=5`  
  `data channel created ... label=beach peer=TransportId(6)`  
  (`/tmp/pong-stack/20251128-085811/beach-host-rhs.log:1330–1435`)

As with LHS, there are no negotiation‑ended‑with‑error warnings for `private-beach-dashboard`; the dashboard connection is fully up.

Browser connector logs for RHS (`sessionId":"54e4b84c-..."`) mirror LHS:

- All `connector.action:*` and `connector.transport:update` entries show `transport:"http_fallback"` only.

Conclusion for both players:

- Browser viewers successfully establish WebRTC data channels to both LHS and RHS hosts (host logs confirm).
- The connector’s notion of `transport` remains `http_fallback` because it is describing the *controller/action* path (browser→manager→host), which is not using WebRTC by the time we log, even though the viewer WebRTC channels are healthy.

### Agent viewer (browser ↔ agent host)

Agent host:

- 13:58:26.329Z  
  `advertised webrtc offer ... signaling_url="http://192.168.68.52:4132/sessions/0fa327c1-.../webrtc"`  

- 13:58:43.532Z  
  `primary data channel opened ... label=beach-manager`  
  `primary data channel opened ... label=mgr-actions`  
  (`beach-host-agent.log:71–72`)

- 13:59:20.100Z  
  `primary data channel opened ... label=private-beach-dashboard`  
  (`beach-host-agent.log:129`)

Again, no negotiation errors. The agent host has a healthy manager viewer webrtc channel and a dashboard channel.

---

## Manager ↔ host WebRTC paths

### LHS manager ↔ host

Manager (`logs/beach-manager/beach-manager.log`, session `675c8415-...`):

- 13:58:37.841Z  
  `attach_by_code verification succeeded ... session_id=675c8415-...`  
  `starting controller forwarder worker ... session_id=675c8415-...`

- 13:58:37.892Z  
  `manager viewer starting connect attempt ... viewer_peer_id=8e9e9c7c-... attempt=1`  
  `ice_env=[("BEACH_GATE_TURN_URLS","turn:192.168.68.52:3478"),("BEACH_TURN_EXTERNAL_IP",None),("BEACH_ICE_PUBLIC_IP","192.168.68.52"),("BEACH_ICE_PUBLIC_HOST","192.168.68.52")]`

- 13:58:41.295Z / 41.371Z  
  `ice_connection_state ... new_state=Checking` for two handshakes / transport IDs.

- 13:58:41.871Z / 41.873Z  
  `ice_connection_state ... new_state=Connected` for both transports.

- 13:58:44.499–.584Z  
  `webrtc answerer connected ... session_id=675c8415-... peer_session_id=cdd6d588-...`  
  `transport established transport="webrtc" ... role=Answerer` (twice, for the two channels)  
  `manager viewer connected via webrtc ... session_id=675c8415-...`  
  `controller forwarder negotiated transport ... transport=WebRtc has_webrtc_channels=true join_ms=66 negotiate_ms=6184`  
  `controller forwarder connected ... transport="webrtc" via_fast_path=true transport_id=9 peer_id=10`  
  `update_pairing_transport_status ... child_session_id=675c8415-... new_transport=Rtc`

From 13:58:44Z onward:

- Controller actions for LHS are dispatched via WebRTC fast path:
  - e.g. 13:59:46.523–.703Z: `controller forwarder dispatching actions to host ... fetched=1 inflight=0 transport="webrtc" via_fast_path=true` and `TRACE sent controller action ... via_fast_path=true transport="webrtc" action_id=... seq=1`.

- Acks are being persisted, but very unevenly:
  - A long sequence of `DEBUG controller acks persisted ... ack_count=1 via_fast_path=false` for LHS between 13:59:45.7Z and 13:59:48.1Z.
  - At 13:59:48.108Z: `controller acks persisted ... ack_count=3 via_fast_path=true` (first time we explicitly see a non‑zero `via_fast_path=true` ack count in this window).

- Then the key transition:
  - 13:59:48.109Z:  
    `WARN ack stall detected; falling back to primary transport ... inflight=3 transport="webrtc"`  
    `TRACE update_pairing_transport_status ... child_session_id=675c8415-... new_transport=HttpFallback new_error="ack_stall"`

After this:

- All subsequent controller actions for LHS use `transport="http_fallback" via_fast_path=false`.
- Pairing status for `child_session_id=675c8415-...` remains `HttpFallback` with occasional `new_error="no action acknowledgements for >1.5s; treating as stalled"`.
- There are **no** `webrtc negotiation failed` errors for LHS in this 11‑28 run; ICE remains `Connected`.

Host (`/tmp/pong-stack/20251128-085811/beach-host-lhs.log`) around the stall:

- At 13:59:47–48Z, immediately around the ack stall, the host is:
  - Polling controller actions over HTTP: `controller action poll returned empty set` (13:59:47.009Z, 47.036Z, etc.).
  - Actively sending frames over WebRTC:
    - 13:59:47.054Z: `dc.send ... state="end" result=Ok(Ok(108)) ... buffered_after=108 ready_state=Open` for transport_id=1.
    - 13:59:48.011Z: `dc.send ... bytes_written=85 ready_state=Open ... buffered_after≈200` for transport_ids 1, 3, 5.
  - There are no `peer negotiation ended with error` or `data channel did not open` warnings for any manager peers in this run.

Summary for LHS:

- Manager↔host WebRTC is successfully established and remains in ICE `Connected` state.
- Datachannels are open and actively sending from host at the time manager declares an ack stall.
- Manager’s controller forwarder decides that acknowledgements for WebRTC actions have not arrived within its 1.5s timeout and **falls back to HTTP**, even though the underlying WebRTC transport is healthy.

### RHS manager ↔ host

Manager (`session_id=54e4b84c-...`):

- 13:58:38.020–.131Z  
  Attach via code + start controller forwarder + manager handshake, identical structure to LHS.

- 13:58:41.296 / 41.369Z  
  `ice_connection_state ... new_state=Checking` for RHS transports.

- 13:58:41.872Z  
  `ice_connection_state ... new_state=Connected` for both RHS transports.

- 13:58:44.499–.584Z  
  `webrtc answerer connected ... session_id=54e4b84c-... peer_session_id=cb8ce39a-...`  
  `transport established transport="webrtc" ...`  
  `manager viewer connected via webrtc ...`  
  `controller forwarder negotiated transport ... transport=WebRtc has_webrtc_channels=true`  
  `controller forwarder connected ... transport="webrtc" via_fast_path=true transport_id=7 peer_id=8`  
  `update_pairing_transport_status ... child_session_id=54e4b84c-... new_transport=Rtc`

- Ack stall sequence for RHS:
  - 13:59:45.941Z: `controller forwarder dispatching actions ... transport="webrtc"`; `TRACE sent controller action ... seq=1 transport="webrtc"`.
  - 13:59:46.106Z: `WARN controller forwarder awaiting acknowledgements before requesting more actions ... inflight=1 transport="webrtc"`.
  - 13:59:47.314Z: more `TRACE sent controller action ... seq=2 transport="webrtc"`.
  - 13:59:47.503Z: `WARN ack stall detected; falling back to primary transport ... inflight=2 transport="webrtc"`; then `update_pairing_transport_status ... new_transport=HttpFallback new_error="ack_stall"`.

After this, RHS mirrors LHS: controller actions go over HTTP fallback, pairing status remains `HttpFallback`, and no WebRTC negotiation failures are recorded for RHS.

Host (`/tmp/pong-stack/20251128-085811/beach-host-rhs.log`) around the stall:

- 13:58:43.535Z: primary + handshake channels opened for `beach-manager` and `mgr-actions`.
- 13:59:01.610Z & 13:59:03.211Z: `private-beach-dashboard` data channel opened and handler registered.
- 13:59:39–48Z: similar to LHS, host continues to send frames over WebRTC with `dc.send` returning `Ok` and `ready_state=Open`, and no negotiation errors.

Summary for RHS:

- Manager↔RHS host WebRTC is established and remains up.
- Manager decides WebRTC fast-path is stalled (lack of timely acks) and falls back to HTTP, despite host continuing to send over WebRTC without errors.

### Agent manager ↔ host

Manager (`session_id=0fa327c1-...`):

- 13:58:38.041–.131Z: mirror of player attachment + handshake.
- 13:58:41.295–.919Z: `ice_connection_state` transitions from `Checking` to `Connected`.
- 13:58:44.499–.584Z: `webrtc answerer connected`, `transport established transport="webrtc"`, `manager viewer connected via webrtc`, `controller forwarder connected ... transport="webrtc" via_fast_path=true`.

Agent host (`/tmp/pong-stack/20251128-085811/beach-host-agent.log`):

- 13:58:43.532Z: primary data channels opened for `beach-manager` and `mgr-actions`.
- 13:59:20.100Z: primary data channel opened for `private-beach-dashboard`.

Crucially, **no ack stall warnings are logged for the agent session** in manager logs, and there are no negotiation errors on the agent host.

---

## Browser connector / controller timelines

`temp/pong.log` lines for LHS (`sessionId":"675c8415-..."`) and RHS (`sessionId":"54e4b84c-..."`) show:

- A steady flow of:
  - `connector.action:queued` with `detail.transport:"http_fallback"`
  - `connector.action:forwarded` with `detail.transport:"http_fallback"`
  - `connector.action:ack` with `detail.status:"http_fallback"`
  - `connector.transport:update` with `detail.transport:"http_fallback"` and `detail.error:"no action acknowledgements for >1.5s; treating as stalled"`
- There are no logged transitions to `transport:"webrtc"` / `transport:"Rtc"` for either player session.

This is consistent with the manager logs: after the first round of ack stalls, manager permanently switches each player’s controller pairing to HTTP fallback and never re‑promotes WebRTC for controller actions in this run.

---

## What we can confidently say

From the combined evidence:

1. **All three manager↔host WebRTC connections (LHS, RHS, agent) are successfully negotiated and stay in ICE `Connected` state.**  
   There are no `webrtc negotiation failed` warnings for the 11‑28 run for these session IDs, and these peers are all on the same Docker network (no external NAT or TURN required for manager↔host).

2. **All three hosts open primary + handshake data channels for manager (`beach-manager` and `mgr-actions`) and see them stay `Open` during the period where manager declares an ack stall.**  
   Host logs show ongoing `dc.send ... result=Ok(Ok(...)) ready_state=Open` with moderate `buffered_amount`, not a backing‑up channel.

3. **Both player hosts also successfully open a WebRTC data channel for the browser viewer (`private-beach-dashboard`).**  
   These channels show no negotiation errors or disconnects in host logs.

4. **The switch to HTTP is driven entirely by the manager’s ack‑stall logic, not by WebRTC negotiation failure.**  
   Manager’s `ack stall detected; falling back to primary transport ... transport="webrtc"` coincides with ongoing WebRTC traffic on the host and is followed by `update_pairing_transport_status ... new_transport=HttpFallback` and HTTP‑only controller actions.

5. **The agent session does not experience an ack stall in this run.**  
   Its manager↔host WebRTC fast‑path remains in use, suggesting the problem is correlated with the higher‑volume / time‑sensitive Pong player sessions rather than a blanket infrastructure issue.

---

## What remains uncertain (root‑cause hypotheses)

We **do not** yet have a single, definitive, code‑level root cause. The logs narrow the problem to the controller ack pipeline on the manager’s side, but several plausible mechanisms remain:

### Hypothesis A: Over‑aggressive ack stall threshold

- Observed: `ack stall detected; ... no action acknowledgements for >1.5s;` with inflight=2–3.
- If RTT over TURN + NAT occasionally spikes above that window, the manager will misclassify a transient delay as a stall and flip to HTTP.
- Counterarguments:
  - In this setup, manager and hosts are in the same Docker network; only the browser is outside. RTT between manager and host over WebRTC should be low (tens of ms).
  - webrtc‑internals from a prior run (11‑26) shows ~1.2s totalRoundTripTime for a **browser** ↔ host candidate pair over the public STUN path, not for manager↔host inside the Docker net.
  - Host logs around 13:59:47–48Z show multiple successful `dc.send` and no backpressure that would plausibly delay acks by >1.5s for the manager path.

Verdict: threshold tuning *may* be too tight for some deployments, but in this local Docker case it is unlikely to be the primary root cause by itself.

### Hypothesis B: Missing or misrouted acks for WebRTC controller actions

In this model, some controller actions sent via WebRTC are processed by the host, but the corresponding acks either:

- Are never emitted by the host, or
- Are emitted on a different transport or session than the manager expects, or
- Are dropped / not associated with the correct inflight actions in `controller.forwarder`.

Supporting observations:

- Manager logs show:
  - WebRTC controller actions being sent: `sent controller action ... via_fast_path=true transport="webrtc" seq=N`.
  - Acks being persisted with varying `ack_count`, but most of them are stamped `via_fast_path=false`, even while WebRTC is supposedly primary.
  - A short burst where `ack_count` suddenly jumps with `via_fast_path=true` (e.g., LHS at 13:59:48.108Z `ack_count=3 via_fast_path=true`), right before ack stall is declared.

- Host logs show:
  - Healthy WebRTC send loops with no indication of backpressure or send failures.
  - HTTP control poll (`controller action poll`) returning empty sets during the same window (i.e., no immediate fallback delivery to fall back on).

If, for example, the host only sends acks on the HTTP control channel (not the WebRTC datachannel) for some classes of actions, or if acks for WebRTC actions are being sent on the wrong `peer_session_id`, the manager’s `inflight` counter for WebRTC would never reach zero, leading to a stall.

Without code or explicit ack‑level logging on host or manager, we can’t yet distinguish:

- “Host never emitted ack for those WebRTC actions” vs.
- “Manager received ack but misattributed it (wrong session/seq)” vs.
- “Acks arrived too late due to internal scheduling or queueing.”

### Hypothesis C: Ack bookkeeping bug when multiple viewers / transports are active

This run has multiple peers per host:

- Manager viewer (mgr‑actions + beach‑manager channels).
- Browser viewer (private‑beach‑dashboard).
- Agent viewer (for agent host).

The controller fast‑path code has to juggle:

- Multiple `TransportId`s (webrtc and fallback),
- Multiple `peer_session_id`s, and
- Redis‑backed controller queues per session.

We do see:

- `drain_actions_redis returned no actions despite non-empty stream` for LHS later in the run (around 14:03:24Z), suggesting some mismatch between the delivery queue and the underlying stream state.
- Repeated `update_pairing_transport_status ... previous=None new_transport=HttpFallback` entries, which suggest the pairing state machine is being updated from multiple places.

It’s plausible that under concurrent load (two players + agent), the ack bookkeeping logic miscounts or double‑counts acks, causing `inflight` to stay non‑zero even though host is responding. That would produce exactly what we see: WebRTC stays up, but the forwarder believes the fast path is stalled and falls back to HTTP.

### Hypothesis D: Mixed‑transport acks confusing the state machine

Another subtle possibility:

- Some controller actions are delivered via HTTP fallback even while WebRTC is primary; their acks are persisted with `via_fast_path=false`.
- The ack stall logic may be primarily watching for *WebRTC* acks (or for changes in `ack_count` for `via_fast_path=true`), ignoring HTTP acks.
- If a period of time passes where only HTTP acks arrive for a session that is logically “Rtc”, the forwarder might see “no WebRTC acks in >1.5s” and fall back, even though host is responsive via HTTP.

We have circumstantial evidence:

- Many `controller acks persisted` lines for the player sessions are `via_fast_path=false` around the stall times.
- At the moment of ack stall for LHS, `ack_count` briefly increments with `via_fast_path=true` and then immediately the state flips to `HttpFallback`.

We would need explicit logging of per‑transport ack streams to confirm or rule this out.

---

## Synthesis / current best explanation

Given the evidence, the most defensible statement we can make is:

> For this 2025‑11‑28 `pong-stack` run, **manager↔host WebRTC is functionally healthy for all three roles (LHS, RHS, agent)**, and **browser↔host viewer WebRTC is healthy for both players**, but **the manager’s controller fast‑path incorrectly concludes that the WebRTC path for the player sessions is “stalled” due to missing or delayed acknowledgements** and unilaterally falls back to HTTP for controller actions.

In other words, the problem is **not** basic WebRTC connectivity; it is in the **controller ack pipeline and its stall detection logic**:

- Either acks for WebRTC actions are not being generated or routed correctly (Hypothesis B/C), or
- The stall detection policy is too brittle in the presence of mixed HTTP/WebRTC ack sources (Hypothesis D, possibly combined with A).

We do **not** yet have enough visibility to distinguish those hypotheses, because current logging:

- Does not show per‑action ack lifecycles (send→receive→persist) broken down by transport, and
- Does not correlate host‑side ack sends with manager‑side ack receipt.

---

## Logging / instrumentation to guarantee a conclusive next run

To turn the next run into a definitive experiment, we should add targeted logging on both manager and hosts. The goal is to be able to answer, for each controller action sent via WebRTC:

1. Did the host receive and process it?
2. Did the host emit an ack? On which transport?
3. Did the manager receive that ack and match it to the correct inflight action?
4. If the manager declared a stall, which actions were still considered inflight, and why?

### Manager‑side logging improvements

1. **Per‑action send/ack correlation**

   In `controller.forwarder` / `controller.delivery`:

   - When sending a controller action:
     - Log at `TRACE` (with sampling guard):  
       `action_send{session_id, private_beach_id, transport, via_fast_path, seq, action_id, transport_id, peer_session_id}`
   - When an ack is received/persisted:
     - Log at `TRACE`:  
       `action_ack{session_id, private_beach_id, transport_source, via_fast_path, seq, action_id, inflight_before, inflight_after, ack_count_now, last_ack_elapsed_ms}`

   This gives a direct mapping of action → ack, per transport.

2. **Stall detection context**

   In the code that logs:

   > `WARN ack stall detected; falling back to primary transport ... inflight=N transport="webrtc"`

   include a structured payload with:

   - `session_id`, `private_beach_id`
   - `inflight_by_transport` (e.g., `{webrtc: N_webrtc, http: N_http}`)
   - `last_ack_at_ms` and `last_ack_transport`
   - `last_successful_ack_seq`
   - `candidate_actions_without_ack` (a small list of `seq`/`action_id`)

   This will immediately tell us whether the stall is “no acks at all” vs. “acks are coming in via HTTP but not WebRTC” vs. “acks stopped entirely.”

3. **Pairing transport state machine**

   Every `update_pairing_transport_status` should log:

   - `previous` → `new_transport`
   - `cause` (e.g., `ack_stall`, `webrtc_negotiation_failed`, `client_prefers_http`, etc.)
   - `error` (if any)

   We already see `new_transport` and `new_error` in some lines, but this should be made consistent and explicit for all transitions so we can reconstruct the entire state machine per `child_session_id` without guesswork.

4. **Redis / stream integrity**

   For warnings like:

   > `drain_actions_redis returned no actions despite non-empty stream ... queue_depth=25`

   log the stream key, consumer group, last ID seen, and whether any errors occurred reading from Redis. This will let us rule in/out any bug where actions are in Redis but not making it to the forwarder.

### Host‑side logging improvements

1. **Explicit ack emission logs**

   Wherever the host emits controller acks (likely in the terminal host’s controller handler):

   - Log:  
     `controller_ack_send{session_id, private_beach_id, seq, action_id, transport_used, peer_session_id, transport_id}`.

   If acks can be sent on both WebRTC and HTTP, log that explicitly.

2. **Inbound controller actions**

   Log the arrival of controller actions on the host:

   - `controller_action_recv{session_id, private_beach_id, seq, action_id, transport_source}`.

   If we see actions arriving but no corresponding `controller_ack_send`, we know the bug is on the host side.

3. **Datachannel‑level backpressure / health**

   We already log `dc.buffered_amount.before/after` and `ready_state`. For the sessions of interest, it would help to:

   - Log when `buffered_amount_after` exceeds a higher threshold (e.g., 64 KB) to detect if acks might be delayed by backpressure.
   - Log any `ICEConnectionState` transitions to `Disconnected` or `Failed`, if these occur after the initial `Connected`.

### Browser‑side (connector) logging improvements

1. **Explicit Rtc vs HttpFallback transitions**

   When `connector.transport:update` logs, include:

   - `old_transport`, `new_transport`
   - `reason` (e.g., `ack_stall`, `webrtc_negotiation_failed`, `user_prefers_http`)
   - `session_id`, `controller_session_id`, `private_beach_id`

   This will let us see whether the browser ever believed it was on `Rtc` or if it always thought it was `http_fallback` for players.

2. **WebRTC vs HTTP path separation**

   For each action:

   - Log which path was used (`webrtc`, `http_fallback`) and whether it was retried across paths.
   - If an action is retried via HTTP after a WebRTC failure, log that explicitly.

### Optional: webRTC‑internals / metrics

For a truly conclusive run, capture a new `chrome://webrtc-internals` dump while running `pong-stack`:

- Ensure that the dump includes peer connections whose signaling URLs contain `/sessions/675c8415-...` and `/sessions/54e4b84c-...`.
- Correlate the candidate pair stats (RTT, availableOutgoingBitrate, packetsDiscarded, etc.) around the time of the ack stall (~13:59:47–48Z).

This will definitively rule in or out network jitter as a plausible cause for the stall detector triggering.

---

## Recommended next experiment

1. Add the logging instrumentation above (at least the manager‑ and host‑side `action_send` / `action_ack` lifecycle logs and the expanded ack stall payload).
2. Re‑run:
   ```bash
   RUST_LOG=info,webrtc=trace,webrtc::ice_transport=trace,webrtc::peer_connection=trace \
   ./apps/private-beach/demo/pong/tools/pong-stack.sh start bc4c51ba-2004-4e6a-9953-cbf31a83f0f6
   ```
3. When the manager again reports `ack stall detected` / `HttpFallback` for LHS and RHS:
   - Confirm, from logs, whether the host emitted acks for each inflight WebRTC action.
   - Confirm whether those acks were received and matched on the manager by `controller.forwarder`.
   - Confirm whether any HTTP acks were counted during a period the stall detector considered “no acks.”

Given the current evidence, the most likely outcome is that the next run will show either:

- Acks not being emitted for certain WebRTC actions (host‑side bug), or
- Acks arriving but not being counted against inflight WebRTC actions (manager‑side bookkeeping bug, especially in mixed HTTP/WebRTC scenarios).

Either way, the proposed logging should make the exact failure mode unambiguous.
