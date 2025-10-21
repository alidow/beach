# 2025-10-21 Bootstrap Fails With Trace Logging Enabled

## Summary
- Running `cargo run -p beach -- ssh …` with `BEACH_LOG_LEVEL=trace` and `BEACH_LOG_FILE=~/beach-debug/client.log` locally forwards those env vars to the remote host.
- The remote `./beach host --bootstrap-output=json …` inherits `BEACH_LOG_LEVEL=trace`, writes megabytes of trace logs to stdout, and only prints the JSON bootstrap envelope after the log flood.
- The CLI only sleeps ~2 s then `cat`s the remote temp file; because the file starts as empty and the SSH stdout stream contains trace logs instead of the JSON line, the CLI exits with `bootstrap handshake failed: ssh connection closed before bootstrap handshake`.
- Manual inspection (`ls -lh /tmp/beach-bootstrap-*.json`) shows the latest file ballooned to hundreds of MB and the JSON line is buried in trace output, confirming the host ran successfully but the bootstrap contract (single JSON on stdout) was violated by verbose logging.

## Observations (Oct 21, 2025)
- Remote host still has `./beach` 0.1.0-20251021200441 deployed (`ssh ec2-user@13.215.162.4 "./beach --version"`).
- Launching the host directly without trace logging produces the expected JSON blob immediately:
  ```bash
  ssh ec2-user@13.215.162.4 \
    'RUST_LOG=info BEACH_LOG_LEVEL=info ./beach host --bootstrap-output=json --bootstrap-survive-sighup --session-server https://api.beach.sh'
  ```
- Reproducing the failure by exporting the same trace env vars locally (`BEACH_LOG_LEVEL=trace BEACH_LOG_FILE=… cargo run …`) creates `/tmp/beach-bootstrap-<pid>.json` files ≥400 MB with a single JSON line at the bottom and leaves the CLI stuck at “bootstrap handshake failed”.
- Killing the host and rerunning with a sanitized environment immediately restores expected behavior, proving the binary is healthy.

## Root Cause
- The CLI forwards all local env vars, so enabling trace-level logging locally inadvertently enables trace logs on the remote host.
- The bootstrap protocol assumes the host writes exactly one JSON line to stdout; high-volume trace logs violate that expectation, so the CLI sees noisy stdout, stops waiting, and reports a handshake failure even though the host stayed alive.

## Proposed Fix
1. Strip high-verbosity log env vars (`RUST_LOG`, `BEACH_LOG_LEVEL`, `BEACH_LOG_FILE`) from the environment we forward during bootstrap unless the user explicitly opts in (e.g. `--forward-env`).
2. Alternatively (short-term), document that remote logging should be redirected via `BEACH_LOG_FILE=/var/log/beach-host.log` when debugging, so stdout remains reserved for the bootstrap JSON.
3. Update the CLI bootstrapper to tolerate leading log lines by:
   - Streaming stdout until it sees a valid JSON blob, ignoring non-JSON prefixes.
   - Surfacing a clearer diagnostic when stdout contains extra data (“remote host emitted logs before bootstrap JSON; check logging env vars”).

## Immediate Mitigation
- Unset the trace logging env vars before rerunning bootstrap:
  ```bash
  BEACH_LOG_LEVEL=info \
  BEACH_LOG_FILE= \
  cargo run -p beach -- ssh ec2-user@13.215.162.4 … --copy-binary
  ```
- If trace logs are needed, set `BEACH_LOG_FILE` to a remote path via `--remote-env BEACH_LOG_FILE=/var/log/beach-trace.log` so stdout stays clean.

## Follow-Ups
- [ ] Patch the CLI to scrub noisy logging env vars during bootstrap.
- [ ] Add a regression test that ensures the bootstrap stdout is a single JSON line even when verbose logging is requested via `--remote-env`.
- [ ] Consider writing remote logs to stderr to avoid corrupting stdout in future debugging sessions.
