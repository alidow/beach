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
| 4 | 🔄 In progress | Host extraction | **Steps 1–4 complete:** host runtime now lives in `server::terminal::host::run`, `terminal::app` delegates, negotiation helpers are shared, and targeted host tests (`bootstrap_handshake_serializes_expected_fields`, `read_bootstrap_handshake_skips_noise_lines`, `webrtc_mock_session_flow`) are green. **Next:** finish the cleanup pass (docs/clippy/full test) before closing the phase. | `cargo check -p beach-human`, `cargo test -p beach-human bootstrap_handshake_serializes_expected_fields`, `cargo test -p beach-human read_bootstrap_handshake_skips_noise_lines`, `cargo test -p beach-human webrtc_mock_session_flow` |
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
   - 🔄 Partial: dead imports pruned and `cargo fmt` applied. Still need to update docs/comments if any stragglers appear and run `cargo clippy -p beach-human --all-targets -- -D warnings` plus a full `cargo test -p beach-human` before closing the phase.
