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
| 4 | 🔄 In progress | Host extraction | Move host runtime + session UX (`JoinAuthorizer`, raw-mode guard, input gate) into `server::terminal::host` and `session::terminal::{authorization, tty}`; expose `host::run` for reuse | `cargo test -p beach-human webrtc_mock_session_flow` and `cargo test -p beach-human handshake_refresh_stops_after_completion` |
| 5 | ⏳ Todo | Join extraction | Move join workflow + MCP proxy bootstrap into `client::terminal::join`; keep negotiation shared | `cargo test -p beach-human` and `cargo run -p beach-human -- join --help` |
| 6 | ⏳ Todo | SSH extraction | Relocate SSH bootstrap into `transport::ssh`; consolidate bootstrap helpers | `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines` and `cargo run -p beach-human -- ssh --help` |
| 7 | ⏳ Todo | Transport negotiation | Create `transport::terminal::negotiation` housing negotiation + failover + heartbeat publisher | `cargo test -p beach-human heartbeat_publisher_emits_messages` and `cargo test -p beach-human handshake_refresh_stops_after_completion` |
| 8 | ⏳ Todo | Sync pipeline move | Shift timeline/backfill/update-forwarder + send helpers into `sync::terminal::server_pipeline` | `cargo test -p beach-human webrtc_mock_session_flow`, `cargo test -p beach-human history_backfill_contains_line_text`, `cargo test -p beach-human history_backfill_skips_default_rows` |
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
   - Add `session/terminal/{tty.rs, authorization.rs}` with the raw-mode guard, input gate, and join-authorization prompt.
   - Re-export the new modules from `session/mod.rs` and update call sites to use them.
   - Tests: `cargo check -p beach-human`.

2. **Host Runtime Module**
   - Create `server/terminal/host.rs` exposing `pub async fn run(base_url, HostArgs)`.
   - Move `handle_host` and supporting helpers (preview setup, acceptor, update forwarder, shared transport types, resize/input handlers, queue structs) from `terminal::app` into the new module.
   - Keep telemetry/trace hooks unchanged while relocating imports.
   - Tests: `cargo check -p beach-human`.

3. **Wire-Up**
   - Update `server/terminal/mod.rs` to export `host`.
   - Change `terminal::app::run` to delegate host requests to `server::terminal::host::run` and drop the duplicated logic/imports.
   - Ensure `main.rs` still compiles with the slimmer `app` module.
   - Tests: `cargo check -p beach-human`, `cargo run -p beach-human -- --help`.

4. **Regression Tests**
   - Run targeted host flows: `cargo test -p beach-human bootstrap_handshake_serializes_expected_fields`, `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines`, and `cargo test -p beach-human webrtc_mock_session_flow`.
   - Smoke test SSH bootstrap: `cargo run -p beach-human -- ssh --help` (optional but recommended).

5. **Cleanup Pass**
   - Remove dead imports from `terminal::app.rs` and friends.
   - Ensure module-level docs/comments point at the new locations.
   - Final check: `cargo fmt`, `cargo clippy -p beach-human --all-targets -- -D warnings`, `cargo test -p beach-human`.
