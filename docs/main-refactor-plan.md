# beach main.rs Refactor Roadmap

## Vision
- Shrink `apps/beach/src/main.rs` to a thin bootstrap that wires CLI parsing to the chosen state-sync machine.
- Align runtime code with the directory responsibilities (`cache/`, `client/`, `mcp/`, `model/`, `protocol/`, `server/`, `session/`, `sync/`, `telemetry/`, `transport/`), placing terminal-specific logic under `*/terminal/` so new state machines (GUI, etc.) can plug in beside it.
- Keep behaviour stable by landing changes in small, testable phases.

## Guiding Principles
- Move code first, refactor behaviour later. Let the compiler surface anything missed.
- Prefer additive copy/adapt/delete flows; avoid editing large blocks in place when splitting files.
- Maintain clear dependency flow: `main` → `terminal` bootstrap → specialised subsystems.
- Run deterministic checks after every phase before moving forward.

## Phased Plan & Test Gates

| Phase | Status | Goal | Key Work Items | Required Tests |
| --- | --- | --- | --- | --- |
| 0 | ✅ Done | Establish baseline | Run full suite once before touching code | `cargo test -p beach-human` |
| 1 | ✅ Done | Extract CLI & config helpers | Create `terminal::{cli,config}` modules, relocate Clap structs + logging glue, expose `parse()` helper; update `main.rs` to consume them | `cargo check -p beach-human` and `cargo run -p beach-human -- --help` |
| 2 | ✅ Done | Isolate bootstrap protocol | Move bootstrap handshake structs + helpers into `protocol/terminal/bootstrap.rs`; update callers | `cargo test -p beach-human bootstrap_handshake_serializes_expected_fields` and `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines` |
| 3 | ✅ Done | Introduce dispatcher | Add `terminal::app::run` coordinating `host/join/ssh`; slim `main.rs` to logging + delegation | `cargo check -p beach-human` and `cargo run -p beach-human -- --help` |
| 4 | ✅ Done | Host extraction | Host runtime now lives in `server::terminal::host::run`, `terminal::app` delegates, negotiation helpers are shared, and all targeted tests (including the full suite) plus `cargo clippy -p beach-human --all-targets -- -D warnings` pass. | `cargo check -p beach-human`, `cargo test -p beach-human bootstrap_handshake_serializes_expected_fields`, `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines`, `cargo test -p beach-human webrtc_mock_session_flow`, `cargo test -p beach-human`, `cargo clippy -p beach-human --all-targets -- -D warnings` |
| 5 | ✅ Done | Join extraction | Join workflow + MCP proxy bootstrap now live in `client::terminal::join`; negotiation helpers remain shared | `cargo test -p beach-human` and `cargo run -p beach-human -- join --help` |
| 6 | ✅ Done | SSH extraction | SSH bootstrap now lives in `transport::ssh::run`, terminal app delegates, helpers are consolidated | `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines` and `cargo run -p beach-human -- ssh --help` |
| 7 | ✅ Done | Transport negotiation | Negotiation, failover, and heartbeat publisher now live under `transport::terminal::negotiation` with callers updated | `cargo test -p beach-human heartbeat_publisher_emits_messages` and `cargo test -p beach-human handshake_refresh_stops_after_completion` |
| 8 | 🔄 In progress | Sync pipeline move | Shift timeline/backfill/update-forwarder + send helpers into `sync::terminal::server_pipeline` | `cargo test -p beach-human webrtc_mock_session_flow`, `cargo test -p beach-human history_backfill_contains_line_text`, `cargo test -p beach-human history_backfill_skips_default_rows` |
| 9 | ⏳ Todo | Runtime utilities & clean-up | Rehome spawn config helpers, viewport utilities, frame encoders; prune leftovers & update docs | `cargo fmt`, `cargo clippy -p beach-human --all-targets -- -D warnings`, `cargo test -p beach-human` |

## Notes & Risk Mitigation
- Compile after each move to catch missing imports/visibility (`cargo check -p beach-human`).
- Keep new modules private (`pub(crate)`) unless cross-crate reuse is required.
- If a phase uncovers hidden coupling, park TODOs in module doc comments instead of expanding scope.
- Update this plan whenever scope shifts; treat the table as the canonical checklist.

## Handy Commands
- `cargo test -p beach-human` — full regression for the CLI binary.
- `cargo test -p beach-human webrtc_mock_session_flow` — host/client handshake via in-process transports.
- `cargo test -p beach-human bootstrap_handshake_serializes_expected_fields` — validates bootstrap struct stability.
- `cargo test -p beach-human history_backfill_contains_line_text` — covers sync pipeline output.
- `cargo run -p beach-human -- --help` — fast smoke test that CLI still builds.

Following these phases keeps the work reviewable and verifiable, while steadily steering the codebase toward the intended module layout.

## Phase 4 – Host Extraction Checklist

1. **Session Utilities**
   - ✅ Done (via this session): `session/terminal/{tty.rs, authorization.rs}` now hold the raw-mode guard, host-input gate, and join authorization prompt. `session/mod.rs` re-exports the namespace, and `terminal::app` consumes the new helpers.
   - Tests: `cargo check -p beach-human` (passing).

2. **Host Runtime Module**
   - ✅ Done: `server/terminal/host.rs` now exposes `pub async fn run(base_url, HostArgs)` and owns the former host workflow (preview setup, acceptor, update forwarder, shared transport types, resize/input handlers, queue structs).
   - Tests: `cargo check -p beach-human`.

3. **Wire-Up**
   - ✅ Done: `server/terminal/mod.rs` exports `host`, and `terminal::app::run` delegates host commands with the heavy imports removed.
   - Tests: `cargo check -p beach-human`.

4. **Regression Tests**
   - ✅ Done: `cargo test -p beach-human bootstrap_handshake_serializes_expected_fields`, `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines`, and `cargo test -p beach-human webrtc_mock_session_flow`.
   - Smoke test SSH bootstrap (`cargo run -p beach-human -- ssh --help`) remains optional.

5. **Cleanup Pass**
   - ✅ Done: docs updated, lint backlog cleared, `cargo fmt`, `cargo test -p beach-human`, and `cargo clippy -p beach-human --all-targets -- -D warnings` are all green.

## Phase 5 – Join Extraction Checklist

1. **Module Scaffold**
   - ✅ Done: created `client::terminal::join` module and re-routed terminal CLI to call into it.

2. **Logic Lift-and-Shift**
   - ✅ Done: `client::terminal::join::run` now owns session discovery, transport negotiation, MCP proxy spawn, and client bootstrap without behavioural changes.

3. **App Slimming & Re-exports**
   - ✅ Done: `summarize_offers`, `kind_label`, passcode prompts, and `interpret_session_target` live under `client::terminal::join`; `terminal::app` is reduced to CLI dispatch plus SSH bootstrap.

4. **Tests & Follow-ups**
   - ✅ Done: `cargo test -p beach-human`, `cargo run -p beach-human -- join --help`, `cargo clippy -p beach-human --all-targets -- -D warnings`, and `cargo fmt` executed successfully.

## Phase 6 – SSH Extraction Checklist

1. **Module Lift**
   - ✅ Done: added `transport::ssh::run` housing the SSH bootstrap workflow, including remote command orchestration and handshake capture.

2. **App Delegation**
   - ✅ Done: `terminal::app::run` now defers the SSH command to the new transport module, leaving the CLI surface minimal.

3. **Helper Consolidation**
   - ✅ Done: stdout/stderr forwarding and bootstrap cleanup helpers moved alongside the SSH module with no remaining duplicates in `terminal::app`.

4. **Regression Checks**
   - ✅ Done: `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines` and `cargo run -p beach-human -- ssh --help` pass after the move.

## Phase 7 – Transport Negotiation Checklist

1. **Module Creation**
   - ✅ Done: added `transport::terminal::negotiation` exposing `negotiate_transport`, `Negotiated*` types, and consolidation helpers for transport selection.

2. **Failover Primitives**
   - ✅ Done: `SharedTransport` and `TransportSupervisor` relocated under the new module, with host runtime updated to reference them.

3. **Heartbeat Publisher**
   - ✅ Done: moved the heartbeat loop alongside negotiation, simplifying `terminal::host` by delegating publishing logic.

4. **Callers Updated & Tests**
   - ✅ Done: `client::terminal::join` and `server::terminal::host` now import from the transport module; `cargo test -p beach-human heartbeat_publisher_emits_messages` and `cargo test -p beach-human handshake_refresh_stops_after_completion` both pass.

## Phase 8 – Sync Pipeline Checklist

1. **Scope & Plan**
   - ✅ Done: inventoried `host_frame_label`, `send_*` chunkers, `collect_backfill_chunk`, `TimelineDeltaStream`, `TransmitterCache`, `ForwarderCommand`/`spawn_update_forwarder`, and handshake helpers inside `server::terminal::host` so we know exactly what moves into the shared module.

2. **Module Scaffold**
   - ✅ Done: `sync::terminal::server_pipeline` now owns the chunking helpers, timeline/backfill types, and shared negotiation plumbing (`spawn_update_forwarder`, `sync_config_to_wire`, `transmit_initial_snapshots`).

3. **Host Integration**
   - ✅ Done: `server::terminal::host` imports the shared helpers, retains only host-facing glue (accept loops, viewport wiring), and drops the duplicated pipeline definitions.

4. **Regression Tests**
   - ✅ Done: `cargo test -p beach-human webrtc_mock_session_flow`, `cargo test -p beach-human history_backfill_contains_line_text`, and `cargo test -p beach-human history_backfill_skips_default_rows` all pass with the new module layout.

5. **Follow-ups**
   - ⏳ Pending: add inline docs/ownership notes to the new module, double-check constant single-sourcing, and log any remaining host↔pipeline coupling for Phase 9 cleanup.
