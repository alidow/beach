# 2025-10-21 Bootstrap Fails With Trace Logging Enabled

## Summary
- `beach ssh … --keep-ssh --ssh-keep-host-running` hard-codes `env RUST_LOG=trace BEACH_LOG_LEVEL=trace …` when launching the remote host (`remote_bootstrap_args` in `apps/beach/src/protocol/terminal/bootstrap.rs`).
- Because of that baked-in trace log level, every launch produces megabytes of trace output on stdout before the JSON envelope, so the CLI gives up with `bootstrap handshake failed: ssh connection closed before bootstrap handshake`.
- Manual inspection (`ls -lh /tmp/beach-bootstrap-*.json`) shows the newest files balloon to hundreds of MB with a single JSON line at the end; the remote process stays alive (`pgrep "./beach host"` lists multiple survivors).

## Observations (Oct 21, 2025)
- Remote host still has `./beach` 0.1.0-20251021200441 deployed (`ssh ec2-user@13.215.162.4 "./beach --version"`).
- Launching the host directly without trace logging produces the expected JSON blob immediately:
  ```bash
  ssh ec2-user@13.215.162.4 \
    'RUST_LOG=info BEACH_LOG_LEVEL=info ./beach host --bootstrap-output=json --bootstrap-survive-sighup --session-server https://api.beach.sh'
  ```
- Reproducing the failure via the CLI always emits `env … BEACH_LOG_LEVEL=trace` on the remote regardless of local settings (confirmed by inspecting the SSH command in debug output). Even setting `BEACH_LOG_LEVEL=info` locally still results in trace on the remote because of the hard-coded constant.
- Killing the host and rerunning it manually with `BEACH_LOG_LEVEL=info` immediately restores expected behavior, proving the binary is healthy.

## Root Cause
- The bootstrap client forces trace-level logging remotely when `--keep-ssh` is used (a regression introduced when we wanted extra diagnostics for long-lived hosts).
- The bootstrap protocol assumes the host writes exactly one JSON line to stdout; high-volume trace logs violate that expectation, so the CLI sees noisy stdout, stops waiting, and reports a handshake failure even though the host stayed alive.

## Proposed Fix
1. Stop hard-coding `BEACH_LOG_LEVEL=trace` inside `remote_bootstrap_args`; respect a user-supplied override or fall back to `info`.
2. Provide a first-class `--remote-env` or `--remote-log-level` flag so engineers can turn trace on deliberately when needed.
3. Update the CLI bootstrapper to tolerate leading log lines by:
   - Streaming stdout until it sees a valid JSON blob, ignoring non-JSON prefixes.
   - Surfacing a clearer diagnostic when stdout contains extra data (“remote host emitted logs before bootstrap JSON; check logging env vars”).

## Immediate Mitigation
- Avoid `--keep-ssh` until the CLI stops forcing trace, or override the remote launch by explicitly requesting info level through the command vector (temporary hack):
  ```bash
  cargo run -p beach -- \
    ssh ec2-user@host \
    --keep-ssh \
    --ssh-keep-host-running \
    -- \
    env BEACH_LOG_LEVEL=info BEACH_LOG_FILE=/tmp/beach-host.log
  ```
- Alternately, run without `--keep-ssh` during bootstrap; the remote command then omits the `env …` wrapper and sticks to the binary defaults.

## Follow-Ups
- [ ] Patch the CLI to respect a configurable remote log level (default info) and stop hard-coding trace.
- [ ] Add a regression test that ensures the bootstrap stdout is a single JSON line even when remote trace logging is enabled.
- [ ] Consider writing remote logs to stderr to avoid corrupting stdout in future debugging sessions.
