# Publish Token Integration Plan

Public Beach Buggy sessions currently receive only a **controller token** via the
manager handshake. That lease token lets the host queue controller actions, but
it does *not* authorize publishing terminal state or health events back to the
Private Beach manager. Because the heartbeat/idle snapshot worker has no bearer,
public hosts stop emitting `/sessions/:id/state` and `/sessions/:id/health`
updates, causing the manager’s stale-session watchdog to tear them down and the
agent’s controller POSTs to time out.

This document outlines how to introduce a **publish token** that is scoped for
state/health uploads, expose it to Beach Buggy automatically, and ensure
`apps/private-beach-rewrite-2` (the only supported dashboard) benefits from it.
The legacy `apps/private-beach` code is deprecated and should not be updated.

## Goals

1. Every host (public or private) receives a short-lived bearer token that
   authorizes `/sessions/:id/state` and `/sessions/:id/health`.
2. Beach Buggy consumes this token automatically and keeps its idle snapshot /
   heartbeat worker running without requiring manual env vars.
3. The Private Beach rewrite-2 frontend does not need configuration changes;
   agents and hosts continue to interact through the manager as they do today.
4. Only the Beach Buggy harness ever sees the publish token. The rewrite-2 UI
   and user-facing agents keep operating with controller tokens and Clerk
   tokens respectively.

## High-Level Architecture

```
Beach Manager (controller handshake)
  ├─ controller_token (existing)
  └─ publish_token (new) ─┐
                           │
Beach Buggy host           │
  ├─ controller API (leases, /actions)
  └─ idle snapshot worker (uses publish token for /state + /health)

Private Beach Rewrite-2
  ├─ Clerk JWT → gate exchange → manager token (unchanged)
  └─ Controller pairings → controller tokens (unchanged)
```

## Implementation Plan

### 1. Manager: Issue Publish Token on Attach/Handshake

* `AppState` now mints a JWT with scopes `pb:sessions.write pb:harness.publish`
  whenever a session attaches (register, attach-by-code, attach-owned) or a
  controller lease is acquired/renewed.
* The token is surfaced via a `idlePublishToken` transport hint alongside the
  existing controller auto-attach metadata. The hint mirrors into the
  `idle_snapshot.publish_token` payload for backwards compatibility.
* Controller handshakes (`/sessions/:id/control/handshake`) include the hint so
  Beach Buggy can consume it immediately; the control-plane also refreshes the
  hint whenever it auto-dispatches a manager handshake.
* Tokens are short-lived (default 30 minutes) and are reissued whenever the
  manager hands out a new lease, so cached credentials expire naturally.

### 2. Beach Buggy Harness: Consume the Hint

* `parse_idle_publish_bearer()` now prefers the new `idlePublishToken` hint but
  falls back to the legacy `idle_snapshot.publish_token` field or the
  `PB_MANAGER_TOKEN` env for older hosts.
* `IdleSnapshotController` sets the bearer as soon as the hint arrives (either
  from the register response or from future manager handshakes) so the idle
  snapshot worker no longer logs “token unavailable”.
* All logging continues to omit the raw JWT; we only note whether publish
  credentials were delivered.

### 3. Manager Renewal & Revocation Logic

* Publish hints are stored per session and rewritten any time the manager renews
  or reissues a controller lease. Hosts therefore receive a fresh token during
  each handshake.
* When the last controller lease disappears (e.g., session detaches), the
  manager strips the `idlePublishToken` hint so stale hosts cannot keep
  publishing.

### 4. Private Beach Rewrite-2 (Dashboard)

* No direct changes required. The rewrite-2 app already orchestrates controller
  pairings via Clerk manager tokens and does not need access to publish tokens.
* Documentation should reflect that only rewrite-2 is supported and hosts obtain
  publish credentials via the handshake.

### 5. Tooling & Validation

* Update `docs/helpful-commands/pong.txt` to mention that no manual manager
  token is required once publish tokens are enabled.
* Add integration tests (or CLI smoke tests) that run `beach host` without any
  env credentials, attach it to a private beach, and verify the manager logs
  continuous `signal_health` / `push_state` events.
* For observability, emit metrics in the manager whenever a publish token is
  issued/renewed, so we can confirm public sessions are receiving them.

## Security Considerations

* Publish tokens should be scoped narrowly (only state/health endpoints) and
  tied to a single session ID.
* Tokens must be short-lived; Beach Buggy will automatically refresh by
  reattaching or requesting a new handshake.
* Only the Beach Buggy harness needs the token. Agents, browsers, and other
  clients should remain unaware of it.

## Rollout Steps

1. Implement the manager changes behind a feature flag (e.g.,
   `PB_PUBLISH_TOKEN_HANDSHAKE=1`).
2. Update Beach Buggy to consume the hint while retaining fallbacks.
3. Deploy manager + CLI builds to staging; run the Pong demo without any
   `PB_MANAGER_TOKEN` env vars and confirm sessions stay alive.
4. Enable the feature flag in production, monitor controller/publish token
   metrics, and remove the flag once stable.

With this flow in place, public Beach Buggy sessions will keep their controller
and publish channels alive automatically, resolving the current timeout loop.
