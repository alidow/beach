# beach main.rs Refactor Roadmap

## Objectives
- Reduce `apps/beach/src/main.rs` to a thin bootstrap that wires CLI input to the chosen state-sync implementation.
- Enforce the established directory responsibilities (cache, client, mcp, model, protocol, server, session, sync, telemetry, transport) with terminal-specific code inside `*/terminal/`.
- Preserve behaviour for terminal sessions while setting scaffolding for future state-sync machines (e.g. GUI) and keeping deterministic test coverage after each increment.

## Guiding Principles
- Prefer moving code without rewriting behaviour; defer optimisations until functionality is modularised.
- Favour additive refactors (copy + adapt + delete) with exhaustive compiler errors as a guide.
- Keep terminal-specific logic under `*/terminal/` submodules; place reusable/generic code in the parent module.
- After each increment run focussed tests (`cargo test -p beach --lib`, targeted integration tests, or existing binaries) before proceeding.

## Incremental Plan

### 0. Baseline Safety Net
- **Action**: Capture current behaviour by running `cargo test -p beach` and the existing integration subset `cargo test -p beach --test webrtc_mock_session_flow` (if isolated).
- **Artifact**: Save test logs and note any flaky cases.
- **Rationale**: Establish baseline before moving code.

### 1. Extract CLI & Logging Glue (`terminal::cli`)
- **Action**:
  - Create `apps/beach/src/client/terminal/cli.rs` (or `apps/beach/src/terminal/cli.rs` if you prefer a new top-level `terminal/` module) and move `Cli`, `LoggingArgs`, `Command`, `HostArgs`, `JoinArgs`, `SshArgs`, `BootstrapOutput`, and `cursor_sync_enabled`.
  - Export a `pub fn parse() -> BeachCli` returning the structured arguments and a `pub fn configure_logging()` helper.
  - Adjust `lib.rs` (or introduce `apps/beach/src/terminal/mod.rs`) to re-export new CLI module.
  - Keep bootstrap-handshake-related enums temporarily in place (later step).
- **Tests**:
  - `cargo test -p beach --bin beach -- --help` (ensure CLI builds; the command prints help and exits).
  - `cargo test -p beach tests::bootstrap_handshake_serializes_expected_fields` (the unit test ensures CLI struct still available).

### 2. Isolate Bootstrap Handshake Types (`protocol::terminal::bootstrap`)
- **Action**:
  - Move `BootstrapHandshake` struct, `emit_bootstrap_handshake`, `read_bootstrap_handshake`, `remote_bootstrap_args`, `render_remote_command`, `shell_quote`, `scp_destination`, `resolve_local_binary_path`, and `copy_binary_to_remote` into `apps/beach/src/protocol/terminal/bootstrap.rs`.
  - Provide `pub` APIs consumed by both CLI (for host) and SSH command.
  - Keep CLI modules calling into this new file.
- **Tests**:
  - `cargo test -p beach bootstrap_handshake_serializes_expected_fields`.
  - `cargo test -p beach read_bootstrap_handshake_skips_noise_lines`.
  - `cargo test -p beach shell_quote_handles_spaces_and_quotes`.

### 3. Split Command Dispatch Layer (`terminal::app`)
- **Action**:
  - Introduce `apps/beach/src/terminal/app.rs` containing `pub async fn run(cli: BeachCli) -> Result<(), CliError>` that matches on `Command` and calls out to specialised modules (`host`, `join`, `ssh`).
  - Update `main.rs` to: configure logging, parse CLI, call `terminal::app::run(cli).await`. `main.rs` should now only hold `tokio::main` and error printing.
- **Tests**:
  - `cargo test -p beach` (ensures binary links with new module).
  - Manual smoke: `cargo run -p beach -- --help` (ensures CLI wiring still works).

### 4. Extract Host Workflow (`server::terminal::host`)
- **Action**:
  - Copy `handle_host` and all host-only helpers (input gate, local preview, webrtc accept loop, viewer accept loop, update forwarder spawn) into `apps/beach/src/server/terminal/host.rs`.
  - Move `JoinAuthorizer`, `JoinAuthorizationMetadata`, `run_authorization_prompt`, `PromptCleanup`, `HostInputGate`, and `RawModeGuard` into `apps/beach/src/session/terminal/authorization.rs` (or similar).
  - Inject dependencies (terminal sync builder, transport registration) via parameters where possible; minimise direct `use` of other modules by adding `pub` constructors.
  - Keep terminal-specific sync/backfill logic local until later step to avoid cascading churn.
- **Tests**:
  - `cargo test -p beach host::webrtc_mock_session_flow` (integration ensures host handshake still works).
  - `cargo test -p beach handshake_refresh_stops_after_completion`.
  - Manual: `cargo run -p beach -- host --help` to confirm CLI help path still functions.

### 5. Extract Join Workflow (`client::terminal::join`)
- **Action**:
  - Move `handle_join` and the MCP proxy spawn block into `apps/beach/src/client/terminal/join.rs`.
  - Ensure `TerminalClient` remains imported from existing module; provide public `JoinContext` struct to share config data with host (if needed later for GUI).
  - Keep negotiation function shared for now; it will move next.
- **Tests**:
  - `cargo test -p beach` (covers client handshake tests).
  - Manual: run `cargo run -p beach -- join --help`.

### 6. Extract SSH Bootstrap (`transport::ssh`)
- **Action**:
  - Move `handle_ssh` and helper logic into `apps/beach/src/transport/ssh/bootstrap.rs` (or `client/terminal/ssh.rs` if preferred).
  - Retain dependency on bootstrap protocol module from Step 2.
  - Provide `pub async fn run(args: SshArgs, base_url: &str) -> Result<(), CliError>` used by dispatcher.
- **Tests**:
  - `cargo test -p beach read_bootstrap_handshake_skips_noise_lines` (should still pass because it now lives in protocol module).
  - Manual: `cargo run -p beach -- ssh --help`.

### 7. Centralise Transport Negotiation (`transport::terminal::negotiation`)
- **Action**:
  - Create `apps/beach/src/transport/terminal/negotiation.rs` and move `negotiate_transport`, `NegotiatedTransport`, `NegotiatedSingle`, `TransportSupervisor`, and `SharedTransport`.
  - Expose a clean API that both host and join modules call. Provide typed result enumerations within the module.
  - Move `HeartbeatPublisher` beside negotiation or into `sync` module if better aligned.
- **Tests**:
  - `cargo test -p beach handshake_refresh_stops_after_completion` (relies on negotiation).
  - `cargo test -p beach heartbeat_publisher_emits_messages`.

### 8. Move Terminal Sync Pipeline (`sync::terminal::server_pipeline`)
- **Action**:
  - Extract `TimelineDeltaStream`, `TransmitterCache`, `PreparedUpdateBatch`, `spawn_update_forwarder`, `collect_backfill_chunk`, `Backfill*` structs, and `send_*` helpers into `apps/beach/src/sync/terminal/server_pipeline.rs`.
  - Export a struct like `TerminalHostPipeline` responsible for owning the channels and update spawn logic, returning handles to the host module.
  - Ensure telemetry hooks move with the code.
- **Tests**:
  - `cargo test -p beach webrtc_mock_session_flow` (exercise sync pipeline).
  - `cargo test -p beach history_backfill_contains_line_text` & `history_backfill_skips_default_rows`.

### 9. Curate Miscellaneous Utilities
- **Action**:
  - Place `display_cmd`, `build_spawn_config`, `detect_terminal_size`, and viewport command handling in `server/terminal/runtime.rs`.
  - Consolidate repeated `send_host_frame` helpers into `protocol::terminal::frames` for reuse.
  - Ensure host module depends on these via small APIs, not large glob imports.
- **Tests**:
  - `cargo test -p beach` (ensures all unit tests still pass).
  - Manual: `cargo run -p beach -- host --local-preview` within dev environment (optional smoke test).

### 10. Prune Residuals & Document
- **Action**:
  - Remove unused imports from `main.rs` (should now be lean), leaving only `use terminal::app::run`.
  - Update module manifests (`mod` statements) and ensure `lib.rs`/`main.rs` compile without referencing removed symbols.
  - Add module-level docs summarising the split and update `docs/` or README to point to new architecture.
- **Tests**:
  - `cargo fmt`, `cargo clippy -p beach --all-targets -- -D warnings`.
  - `cargo test -p beach` (full regression).

## Risk Mitigation & Notes
- Prioritise compiler-guided refactors: move code, fix imports, run tests.
- If new modules require visibility changes (`pub(crate)` etc.), prefer narrow scopes to avoid exposing internals publicly.
- Each extraction step should keep function signatures stable; only change call sites once new module is in place and confirmed by tests.
- For future state-sync machines, factor new modules under `apps/beach/src/<category>/mod.rs` to reuse the same API surface.

## Suggested Test Commands Cheat Sheet
- `cargo test -p beach` – full unit + integration suite for the CLI binary.
- `cargo test -p beach webrtc_mock_session_flow` – exercises host/client handshake via in-process transport pair.
- `cargo test -p beach bootstrap_handshake_serializes_expected_fields` – ensures bootstrap structs remain stable.
- `cargo test -p beach history_backfill_contains_line_text` – validates sync pipeline after extraction.
- `cargo run -p beach -- --help` – quick smoke for CLI wiring.

Following this roadmap keeps the system operational after each incremental change while progressively aligning the codebase with the intended directory structure and paving the way for additional state-sync machines.
