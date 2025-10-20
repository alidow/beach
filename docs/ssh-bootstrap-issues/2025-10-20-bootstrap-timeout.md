# 2025-10-20 Bootstrap Handshake Timeout

## Summary
- Local bootstrap command fails with `ssh connection closed before bootstrap handshake`.
- Remote bootstrap log created as `/tmp/beach-bootstrap-218954.json` with size `0` bytes.
- This behavior has been recurring after apparently successful fixes.

## Remote Observations (Oct 20, 2025)
- `/tmp/beach-bootstrap-218954.json` remained empty while other historical bootstrap logs grew large (tens of MB), indicating the host process usually writes trace output once it is running.
- `ps -eo pid,state,etime,cmd | grep "./beach host"` shows many long-lived `./beach host` processes still running (oldest started Oct 18). All are sleeping and were launched with `--bootstrap-output=json` (some with `--bootstrap-survive-sighup`), suggesting prior bootstrap attempts never cleaned them up.
- The latest binary on the host is `/home/ec2-user/beach` (timestamp Oct 20 11:37), so the new build was copied successfully before the timeout.

## Hypotheses
- The host binary may take longer than the fixed 2 s window to reach the point where it writes bootstrap JSON (for example, waiting on the session server or other network I/O).
- Alternatively, the host might crash/panic before emitting any output, but no crash traces were logged and new host processes were not observed for this attempt, making a silent slow start more likely.
- Stale host processes and ever-growing `/tmp/beach-bootstrap-*.json` files could compound the timeout by increasing startup work or disk I/O.

## Next Actions
1. Manually run the remote bootstrap command without the 2 s timeout and watch whether/when JSON appears.
2. Capture the first logs from the fresh host start (consider forcing `RUST_LOG=error` until ready, or writing a minimal ready marker earlier in startup).
3. Clean up stale `./beach host` processes and trim historical `/tmp/beach-bootstrap-*.json` files to reduce noise while troubleshooting.
4. Evaluate increasing the bootstrap timeout or adding retries once we confirm the host simply needs more time.

## Resolution
- _Pending – update this section once we confirm the root cause and adopt a permanent fix._
