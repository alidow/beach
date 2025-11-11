# Private Beach Data Model (Postgres)

## Design Goals
- Capture durable relationships between identities, private beaches, sessions, and automation without storing high-volume transient telemetry.
- Keep tables normalized, single-purpose, and named in singular form for clarity (`private_beach`, `private_beach_membership`, etc).
- Delegate authentication to Beach Gate while tracking authorization state and invitations locally.
- Store only metadata for state caches and files; shared key/value payloads remain in Redis for the lifetime of the private beach.

## Entity Overview
- **organization** – top-level customer representing a company or team.
- **organization_membership** – connects accounts to organizations with roles for billing/administration.
- **account** – human users or service agents authenticated through Beach Gate.
- **private_beach** – collaborative surface grouping sessions, layouts, and policies.
- **private_beach_membership** – role + status for an account inside a private beach.
- **beach_group** – optional collection within a private beach for bulk authorization.
- **group_membership** – connect accounts to groups with a role.
- **share_link** – signed, revocable invitation grants scoped access.
- **session** – logical attachment of a Beach/ Cabana session into a private beach.
- **session_tag** – lightweight metadata labels for sessions (e.g., “manager”, “scoreboard”).
- **automation_assignment** – declares which agent account manages which sessions.
- **controller_event** – audit trail of controller lease changes.
- **file_record** – metadata for files stored in S3/Blob store scoped to a private beach.

## Enumerations
- `account_type`: `human`, `agent`, `service`.
- `account_status`: `active`, `disabled`.
- `membership_role`: `owner`, `admin`, `contributor`, `viewer`.
- `membership_status`: `active`, `invited`, `suspended`, `revoked`.
- `group_role`: `admin`, `member`.
- `organization_role`: `owner`, `admin`, `billing`, `member`.
- `harness_type`: `terminal_shim`, `cabana_adapter`, `remote_widget`, `service_proxy`, `custom`.
- `session_kind`: `terminal` (PTY/TUI), `cabana_gui` (Beach Cabana stream), `manager_console` (agent control shell), `widget` (aux UI module), `spectator_feed` (read-only dashboard tile), `service_daemon` (background helper).
- `automation_role`: `observer` (read-only), `controller` (issues actions), `coordinator` (manages other controllers).
- `controller_event_type`: `acquired`, `renewed`, `released`, `preempted`, `revoked`.

## Tables

### organization
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `name` | `text` NOT NULL |
| `slug` | `citext` UNIQUE NOT NULL |
| `billing_email` | `citext` NULL |
| `metadata` | `jsonb` | Billing plan, address, etc |
| `created_at` | `timestamptz` |
| `updated_at` | `timestamptz` |
| `archived_at` | `timestamptz` NULL |

### organization_membership
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `organization_id` | `uuid` FK → `organization` |
| `account_id` | `uuid` FK → `account` |
| `role` | `organization_role` |
| `created_by_account_id` | `uuid` FK → `account` NULL |
| `created_at` | `timestamptz` |
| `updated_at` | `timestamptz` |
| Unique | `(organization_id, account_id)` |

### account
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `type` | `account_type` | Human vs agent vs service |
| `status` | `account_status` | Defaults to `active` |
| `beach_gate_subject` | `text` UNIQUE NOT NULL | Stable identifier from Beach Gate |
| `display_name` | `text` | User supplied display |
| `email` | `citext` UNIQUE NULL | Optional for agents |
| `avatar_url` | `text` | Optional |
| `metadata` | `jsonb` | Freeform attributes (time zone, org) |
| `default_organization_id` | `uuid` FK → `organization` NULL |
| `created_at` | `timestamptz` | `default now()` |
| `updated_at` | `timestamptz` | Updated via trigger |

### private_beach
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `organization_id` | `uuid` FK → `organization` |
| `owner_account_id` | `uuid` FK → `account(id)` |
| `name` | `text` NOT NULL |
| `slug` | `citext` UNIQUE NOT NULL | Human-friendly identifier |
| `description` | `text` |
| `default_role` | `membership_role` | Applied to new share-link guests |
| `layout_preset` | `jsonb` | Initial dashboard layout |
| `settings` | `jsonb` | Flags (shared storage enabled, etc) |
| `created_at` | `timestamptz` |
| `updated_at` | `timestamptz` |
| `archived_at` | `timestamptz` NULL |

### private_beach_membership
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `private_beach_id` | `uuid` FK → `private_beach` |
| `account_id` | `uuid` FK → `account` |
| `role` | `membership_role` | Enforced by Beach Gate tokens |
| `status` | `membership_status` | Defaults to `active` |
| `invited_by_account_id` | `uuid` FK → `account` NULL |
| `invitation_token_hash` | `text` NULL | For pending invites |
| `invited_at` | `timestamptz` NULL |
| `activated_at` | `timestamptz` NULL |
| `suspended_at` | `timestamptz` NULL |
| `notes` | `text` |
| `created_at` | `timestamptz` |
| `updated_at` | `timestamptz` |
| Unique | `(private_beach_id, account_id)` |

### beach_group
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `private_beach_id` | `uuid` FK → `private_beach` |
| `name` | `text` NOT NULL |
| `description` | `text` |
| `created_by_account_id` | `uuid` FK → `account` |
| `created_at` | `timestamptz` |
| `updated_at` | `timestamptz` |
| Unique | `(private_beach_id, lower(name))` |

### group_membership
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `beach_group_id` | `uuid` FK → `beach_group` |
| `account_id` | `uuid` FK → `account` |
| `role` | `group_role` | Controls invite/manage rights |
| `created_at` | `timestamptz` |
| Unique | `(beach_group_id, account_id)` |

### share_link
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `private_beach_id` | `uuid` FK → `private_beach` |
| `created_by_account_id` | `uuid` FK → `account` |
| `label` | `text` | Friendly name |
| `token_hash` | `text` UNIQUE NOT NULL | Hash of signed link token |
| `granted_role` | `membership_role` | Usually `viewer` / `contributor` |
| `max_uses` | `integer` NULL | Null = unlimited |
| `use_count` | `integer` DEFAULT 0 |
| `expires_at` | `timestamptz` NULL |
| `revoked_at` | `timestamptz` NULL |
| `created_at` | `timestamptz` |

### session
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `private_beach_id` | `uuid` FK → `private_beach` |
| `origin_session_id` | `uuid` NOT NULL | ID from open-source Beach core |
| `harness_id` | `uuid` NULL | Present when harness registered |
| `harness_type` | `harness_type` NULL | Identifies sidecar flavor (lightweight PTY shim, Cabana adapter, etc) |
| `kind` | `session_kind` |
| `title` | `text` |
| `display_order` | `integer` | For layout defaults |
| `location_hint` | `text` | e.g., `us-east-1` |
| `capabilities` | `jsonb` | Snapshot of harness-declared capabilities |
| `metadata` | `jsonb` | Additional descriptors (tags, env) |
| `created_by_account_id` | `uuid` FK → `account` NULL |
| `created_at` | `timestamptz` |
| `last_seen_at` | `timestamptz` |
| `ended_at` | `timestamptz` NULL |
| Unique | `(private_beach_id, origin_session_id)` |

### session_tag
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `session_id` | `uuid` FK → `session` |
| `tag` | `text` NOT NULL |
| `created_at` | `timestamptz` |
| Unique | `(session_id, lower(tag))` |

### automation_assignment
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `private_beach_id` | `uuid` FK → `private_beach` |
| `controller_account_id` | `uuid` FK → `account` | Agent or human |
| `role` | `automation_role` |
| `session_id` | `uuid` FK → `session` NULL | Null = scope to entire private beach |
| `config` | `jsonb` | Parameters (e.g., rate limits) |
| `created_by_account_id` | `uuid` FK → `account` |
| `created_at` | `timestamptz` |
| `updated_at` | `timestamptz` |
| Unique | `(controller_account_id, session_id)` with partial for NULL session using unique index with `session_id IS NOT NULL` |

### controller_event
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `session_id` | `uuid` FK → `session` |
| `event_type` | `controller_event_type` |
| `controller_account_id` | `uuid` FK → `account` |
| `issued_by_account_id` | `uuid` FK → `account` NULL | Manager/admin who triggered change |
| `controller_token_id` | `uuid` | FK → `controller_lease(id)` |
| `payload` | `jsonb` | Additional context (reason, expiry) |
| `occurred_at` | `timestamptz` DEFAULT now() |
| Notes | | Emitted only on controller lease transitions (acquire/renew/release/preempt/revoke) |

### controller_lease
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()`; doubles as `controller_token` |
| `session_id` | `uuid` FK → `session` |
| `controller_account_id` | `uuid` FK → `account` NULL | Null for anonymous harness bootstrap |
| `issued_by_account_id` | `uuid` FK → `account` NULL | Actor (human/admin/agent) that granted the lease |
| `reason` | `text` | Optional annotation shown in audit/UI |
| `issued_at` | `timestamptz` DEFAULT now() |
| `expires_at` | `timestamptz` | Lease expiry; manager enforces renewals before this timestamp |
| `revoked_at` | `timestamptz` NULL | Populated when lease is forcefully ended |
| Unique | — | Multiple concurrent leases per session; uniqueness enforced by `id` |
| Notes | | Token string issued to controllers equals `id`; concurrent leases are differentiated by account/reason metadata and validated independently. RLS restricts reads to the same private beach and scoped roles. |

### session_runtime
| Column | Type | Notes |
| --- | --- | --- |
| `session_id` | `uuid` PK FK → `session` | 1:1 extension table; cascades on delete |
| `state_cache_url` | `text` | Redis/WebRTC location provided to harness |
| `transport_hints` | `jsonb` | Broker/WebRTC hinting returned in registration |
| `last_health` | `jsonb` | Latest health heartbeat payload (optional) |
| `last_health_at` | `timestamptz` | Timestamp of last heartbeat |
| Notes | | Supplements Redis cache so restarts can rebuild baseline metadata; terminal state snapshots now live exclusively in Redis |

### file_record
| Column | Type | Notes |
| --- | --- | --- |
| `id` | `uuid` PK | `default uuid_generate_v7()` |
| `private_beach_id` | `uuid` FK → `private_beach` |
| `path` | `text` | Virtual path in namespace |
| `version` | `integer` DEFAULT 1 |
| `storage_key` | `text` NOT NULL | Object storage handle |
| `size_bytes` | `bigint` |
| `content_type` | `text` |
| `checksum` | `text` |
| `uploaded_by_account_id` | `uuid` FK → `account` NULL |
| `uploaded_by_session_id` | `uuid` FK → `session` NULL |
| `uploaded_at` | `timestamptz` |
| `deleted_at` | `timestamptz` NULL |
| Unique | `(private_beach_id, path, version)` |
| Notes | | Either `uploaded_by_account_id` or `uploaded_by_session_id` must be present |

## Relationships Summary
- `organization` 1↔N `private_beach`; `organization` ↔ `account` via `organization_membership`.
- `account` 1↔N `private_beach` (ownership), N↔N via `private_beach_membership`.
- `private_beach` 1↔N `beach_group`; `beach_group` ↔ `account` via `group_membership`.
- `private_beach` 1↔N `session`; each session can carry multiple `session_tag`s and `automation_assignment`s.
- `private_beach_membership` grants direct roles; `group_membership` extends indirect access (Beach Gate issues tokens reflecting both).
- `share_link` produces temporary `private_beach_membership` rows once redeemed; tokens are validated against `share_link.token_hash`.
- `controller_event` provides immutable audit trail tied to session + controller.
- `file_record` scoped to a private beach, referencing either accounts or sessions for provenance; key-value data lives entirely in Redis for the duration of the private beach.

## Implementation Notes
- Prefer UUIDv7 (time-ordered) identifiers for all primary keys to preserve monotonic insert order while avoiding guessable sequences (enable via `uuid_generate_v7()` or ULID helper where needed).
- Use Postgres native enums for role/status fields; migrations must include safe alteration procedures.
- All FK cascades should default to `ON DELETE RESTRICT` except `private_beach_membership` (cascade on account removal) and `session_tag` (cascade on session deletion).
- Materialized views (future) can summarize membership counts, automation coverage, or audit stats.
- Sensitive columns (`token_hash`, `share_link.token_hash`) should use salted hashes with `pgcrypto`; file checksums may remain unhashed as they are non-secret integrity markers.
- Add row-level security policies so that API services enforce Beach Gate claims: e.g., `private_beach_membership.role >= viewer` can `SELECT` session metadata for that private beach.
- `controller_event` entries are emitted on lease lifecycle transitions (acquire/renew/release/preempt), not per render diff, keeping write volume modest while preserving an audit trail.
- Persisted controller leases (`controller_lease`) drive token validation even if Redis loses state; invalidate on revoke/expiry via cron or LISTEN/NOTIFY hooks.
- `session_runtime` holds diagnostic metadata so the manager can resume without waiting for Redis warm-up; treat it as advisory (Redis remains source of truth for real-time diffs).
