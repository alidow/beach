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

| Phase | Goal | Key Work Items | Required Tests |
| --- | --- | --- | --- |
| 0 | Establish baseline | Run full suite once before touching code | `cargo test -p beach-human` |
| 1 | Extract CLI & config helpers | Create `terminal::{cli,config}` modules, relocate Clap structs + logging glue, expose `parse()` helper; update `main.rs` to consume them | `cargo check -p beach-human` and `cargo run -p beach-human -- --help` |
| 2 | Isolate bootstrap protocol | Move bootstrap handshake structs + helpers into `protocol/terminal/bootstrap.rs`; update callers | `cargo test -p beach-human bootstrap_handshake_serializes_expected_fields` and `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines` |
| 3 | Introduce dispatcher | Add `terminal::app::run` coordinating `host/join/ssh`; slim `main.rs` to logging + delegation | `cargo check -p beach-human` and `cargo run -p beach-human -- --help` |
| 4 | Host extraction | Move host runtime + session UX (`JoinAuthorizer`, raw-mode guard, input gate) into `server::terminal::host` and `session::terminal::authorization` | `cargo test -p beach-human webrtc_mock_session_flow` and `cargo test -p beach-human handshake_refresh_stops_after_completion` |
| 5 | Join extraction | Move join workflow + MCP proxy bootstrap into `client::terminal::join`; keep negotiation shared | `cargo test -p beach-human` and `cargo run -p beach-human -- join --help` |
| 6 | SSH extraction | Relocate SSH bootstrap into `transport::ssh`; consolidate bootstrap helpers | `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines` and `cargo run -p beach-human -- ssh --help` |
| 7 | Transport negotiation | Create `transport::terminal::negotiation` housing negotiation + failover + heartbeat publisher | `cargo test -p beach-human heartbeat_publisher_emits_messages` and `cargo test -p beach-human handshake_refresh_stops_after_completion` |
| 8 | Sync pipeline move | Shift timeline/backfill/update-forwarder + send helpers into `sync::terminal::server_pipeline` | `cargo test -p beach-human webrtc_mock_session_flow`, `cargo test -p beach-human history_backfill_contains_line_text`, `cargo test -p beach-human history_backfill_skips_default_rows` |
| 9 | Runtime utilities & clean-up | Rehome spawn config helpers, viewport utilities, frame encoders; prune leftovers & update docs | `cargo fmt`, `cargo clippy -p beach-human --all-targets -- -D warnings`, `cargo test -p beach-human` |

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
