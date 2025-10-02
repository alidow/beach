# SSH Bootstrap Implementation Plan

This plan defines the work required to let `beach-human` establish a WebRTC session by first hopping through SSH, mirroring the bootstrap flow that `mosh` uses. Each phase ends with concrete validation so we can land the work incrementally.

## Phase 0 – Goals & Constraints
- Preserve the existing `beach host`/`beach join` workflows; the SSH bootstrap is an additive UX.
- Ensure the remote host session keeps running after the temporary SSH control channel drops.
- Avoid bespoke daemons on the remote host: the SSH flow should only require the `beach-human` binary.
- Keep stdout noise predictable so callers can automate around it (scripts, other CLIs).

## Phase 1 – Host Handshake Envelope
**Implementation**
- Add a `--bootstrap-output <mode>` flag to the host CLI (`default`, `json`, future-extensible).
- When `json` is selected, write exactly one JSON object to stdout with:
  - `session_id`, `join_code`, `session_server`, selected `transport`, timestamp, and host command metadata.
  - Optional warning field for non-fatal issues (e.g. no WebRTC offer yet).
- Hide the existing banner/pretty logging while JSON mode is active; direct other logs to stderr only when necessary.
- Auto-disable local preview and stdin mirroring while in bootstrap mode (non-TTY safe defaults).

**Validation**
- Unit-test the JSON serializer to ensure forward compatibility and no stray whitespace.
- Smoke test `beach host --bootstrap-output=json` locally and confirm the PTY stays attached after SSH exits.

## Phase 2 – Host Runtime Hardening For SSH
**Implementation**
- Treat bootstrap mode as non-interactive: skip raw mode, terminal resize monitor, and stdin forwarder.
- Ignore `SIGHUP` so the PTY survives when the SSH session closes.
- Add a `--wait` default in bootstrap mode; exit with an informative error if no transport appears within a timeout (configurable env/flag).
- Ensure the host process terminates cleanly on `SIGINT`/`SIGTERM` even when detached.

**Validation**
- Integration test (tokio) that simulates bootstrap mode, drops stdin/stdout early, and verifies runtime completion plus transport handshake.

## Phase 3 – Local Bootstrap Command
**Implementation**
- Introduce a new CLI subcommand: `beach ssh <target> [flags] [-- passthrough]`.
- Spawn `ssh` via `tokio::process::Command` with options:
  - Use `-o BatchMode=yes` by default; allow `--ssh-flag` repeats for overrides.
  - Provide optional `--remote-path` pointing to the remote `beach` binary (default `beach`).
  - Forward extra args after `--` as the remote host command (mirrors existing host CLI).
- Capture stdout/stderr; parse until the handshake JSON appears, then close stdin and let ssh exit.
- Reuse `handle_join` with parsed handshake data to launch the local client automatically.

**Validation**
- Unit-test the handshake parser with mixed stdout/stderr streams (JSON embedded amid banners, partial lines, etc.).
- Add an integration test using a fake `ssh` shim binary that emits canned output.

## Phase 4 – Error Handling & User Feedback
**Implementation**
- Define friendly error messages for:
  - SSH binary missing / non-zero exit.
  - Handshake timeout or malformed JSON.
  - Remote host exiting before handshake.
  - Join failures (bad passcode, transport negotiation failure).
- Surface structured diagnostics behind `--log-level` and emit actionable hints (`--ssh-flag="-v"`, `--remote-path`).

**Validation**
- Extend tests to cover each error path.
- Manual verification with intentionally broken scenarios.

## Phase 5 – Documentation & Follow-ups
**Implementation**
- Add `docs/bootstrap.md` describing setup, handshake schema, and troubleshooting. ✅
- Update existing plans (`apps/beach-human/plan.md`, `README` if present) to reference the new feature. ✅
- Outline future enhancements (scp sync, persistent ssh mode, agentless bootstrap). ➡️ tracked under "Future Enhancements".

**Validation**
- Peer review docs for clarity.
- Ensure new docs render cleanly (no Markdown lint regressions).

With this phased approach we can commit Phase 1–4 in a single PR if preferred, but each phase is independently testable so we can stage the work as needed.
