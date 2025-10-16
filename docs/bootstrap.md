# SSH Bootstrap Guide

This document explains how to start a `beach` session through SSH, mirroring the familiar `mosh` workflow: establish a short-lived SSH control channel, spin up a beach host on the remote machine, then hand off to WebRTC for the active session.

## 1. Requirements
- The `beach` binary must exist on the remote machine and be executable by the SSH user.
- SSH key-based auth is recommended; the local command forces `BatchMode=yes` unless `--no-batch` is provided.
- The remote node needs outbound connectivity to the session broker that both sides share (defaults to `http://127.0.0.1:8080`).

## 2. Fast Start
```bash
# Launch remote host then attach locally
beach ssh user@example.com

# Run a specific program instead of the login shell
beach ssh user@example.com -- -- htop

# Deploy a freshly built binary before launching the host
beach ssh user@example.com --copy-binary --remote-path /tmp/beach -- -- ./service.sh

# Keep the SSH control channel alive for log tailing (logs emitted at --log-level=info)
beach --log-level info ssh user@example.com --keep-ssh

# Override the remote beach binary path and session server
beach --session-server https://beach.example.com ssh \
  user@example.com --remote-path /opt/beach/bin/beach
```

Under the hood the CLI executes the following on the remote machine:
```
exec <remote-path> host --bootstrap-output=json --session-server <url> --wait [-- ...]
```
By default the SSH transport is terminated as soon as the handshake JSON is received; the host ignores `SIGHUP` so the PTY survives once the control channel disappears. Use `--keep-ssh` to leave the channel up, and stream remote stdout/stderr into the local log sink (set `--log-level info` or `--log-file` to observe it).

## 3. Binary Deployment Options
- `--copy-binary` uploads the local executable via `scp` before launching the host. Override the source path with `--copy-from <path>` and the destination binary name with `--remote-path`.
- `--scp-binary` points to an alternative `scp` implementation (defaults to `scp`). All `--ssh-flag` values are forwarded to the transfer.
- When `--copy-binary` is omitted the command assumes the remote `--remote-path` already resolves to an executable.

## 4. Handshake Envelope
The host emits a single JSON object when `--bootstrap-output=json` is active. The current schema (version `1`) looks like:
```json
{
  "schema": 1,
  "session_id": "6aa0c0a1-24f2-4ff7-9c55-1a8c913d7c3c",
  "join_code": "123456",
  "session_server": "http://127.0.0.1:8080",
  "active_transport": "WebRTC",
  "transports": ["webrtc", "websocket"],
  "preferred_transport": "webrtc",
  "host_binary": "beach",
  "host_version": "0.1.0",
  "timestamp": 1700000000,
  "command": ["/bin/zsh"],
  "wait_for_peer": true
}
```
Fields marked optional in the CLI implementation (e.g. `preferred_transport`) are omitted when not present. Future schema bumps will increment `schema` and remain backward compatible.

## 5. SSH Flags and Overrides
- `--ssh-flag FLAG`: repeat to pass arbitrary options (e.g. `-J jumpbox`, `-p 2222`).
- `--no-batch`: disables the default `BatchMode=yes` so SSH may prompt for passwords/OTP.
- `--request-tty`: request a PTY (`ssh -tt`) if you want to watch remote logs before hand-off.
- `--handshake-timeout SECONDS`: abort if the JSON envelope is not received in time (default `30`).
- `--keep-ssh`: leave the control channel open and stream remote stdout/stderr into the log sink (recommended with `--log-level info` or `--log-file`).

## 6. Managed Shells & Profiles
- Non-login shells: remote `.bashrc`/`.zshrc` customisations that echo banners will be ignored by the bootstrap parser as long as they output valid JSON eventually. Keep noisy profile output minimal to avoid timeouts.
- Login shells that switch directories or spawn subshells should leave `exec beach …` as the final command so the handshake completes predictably.
- For managed environments (e.g. corporate jump boxes) prefer `--copy-binary` + `--remote-path /tmp/beach` to avoid permission issues in system directories.

## 7. Troubleshooting
- **SSH exits immediately & client aborts**: ensure the remote binary is on `$PATH` and executable; check `ssh --ssh-flag "-vvv"` for remote errors.
- **Timeout waiting for handshake**: confirm the remote host can talk to the session broker; use `--handshake-timeout` to lengthen and check broker logs.
- **Session server mismatch**: the host prints the `session_server` it actually used; the local client always honours that field.
- **Leave SSH open**: run with `--request-tty` and omit `--no-batch` if you need to keep the channel alive for log tailing; the host will continue running regardless thanks to `SIGHUP` ignore.

## 8. Future Enhancements
- Verify remote binary integrity (hash/size) after transfer when `--copy-binary` is used.
- Parallelise multi-host bootstrap (`beach ssh --fanout`) for fleet rollouts.
- Allow directing `--keep-ssh` output to a specified log file without adjusting global log level.
