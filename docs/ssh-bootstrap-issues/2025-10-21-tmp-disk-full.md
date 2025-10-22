# 2025-10-21 Bootstrap Fails Because /tmp Is Full

## Summary
- After fixing the hard-coded trace logging, SSH bootstrap still failed with `ssh connection closed before bootstrap handshake`.
- Inspecting `/tmp` on the host (`df -h /tmp`) showed the tmpfs was 100 % full (453 MiB used). All prior bootstrap runs left behind huge `beach-bootstrap-*.json` files (up to 404 MiB).
- With /tmp out of space, the remote `nohup … >"$temp_file"` redirection silently failed to write the JSON handshake, leaving a zero-byte file and tripping the CLI timeout.

## Observations (Oct 21, 2025)
- `ls -ltr /tmp/beach-bootstrap-*.json` listed many historical files, including `beach-bootstrap-240304.json` at 404 MB.
- `nohup printf hi >/tmp/test.txt` also produced a zero-byte file with `printf: write error: No space left on device`, confirming the disk-full condition.
- After deleting the stale bootstrap artifacts (`rm -f /tmp/beach-bootstrap-*.json`), `/tmp` usage dropped to 1 %.
- Re-running `beach ssh … --keep-ssh --headless` now prints a small (~12 KB) bootstrap file containing the JSON line followed by `INFO` logs; the CLI successfully receives the handshake and proceeds to headless validation.

## Mitigation
- Clean `/tmp` before launching a new host. The existing cleanup stanza already deletes zero-byte files; extend it (or add a periodic cron) to purge old `beach-bootstrap-*.json` blobs.
- Consider relocating bootstrap logs to a persistent directory with more space (e.g. `/var/log/beach`) or using a rolling file appender when `--keep-ssh` is enabled.
- Raise an explicit error if writing the temp file fails (e.g. check `$?` after `cat "$temp_file"` or write a short `healthcheck` entry to stderr) so disk-pressure is surfaced to the CLI instead of appearing as a generic handshake timeout.

## Validation
- After clearing `/tmp`, running:
  ```bash
  BEACH_LOG_LEVEL=trace RUST_LOG=trace BEACH_LOG_FILE=~/beach-debug/client.log \
  cargo run -p beach -- \
    ssh ec2-user@13.215.162.4 \
    --ssh-flag=-i --ssh-flag=~/.ssh/beach-test-singapore.pem \
    --ssh-flag=-o --ssh-flag=StrictHostKeyChecking=accept-new \
    --copy-binary \
    --copy-from "$(git rev-parse --show-toplevel)/target/x86_64-unknown-linux-gnu/release/beach" \
    --verify-binary-hash \
    --keep-ssh \
    --ssh-keep-host-running \
    --headless \
    --headless-timeout 60 \
    --session-server 'https://api.beach.sh'
  ```
  produced a valid handshake (session `09d00f02-f8ba-4fdd-980e-2e1b72e0ab61`). Headless validation still timed out awaiting the terminal snapshot—a separate transport issue under investigation—but the bootstrap stage succeeded once disk space was restored.

## Follow-Ups
- [ ] Extend the remote cleanup preamble to delete aged `/tmp/beach-bootstrap-*.json` files or relocate them to a persistent log directory with rotation.
- [ ] Have the CLI detect `No space left on device` in the bootstrap temp file and emit a targeted diagnostic.
- [ ] Add a check in the host code to refuse to start when the bootstrap output path cannot be created (fail fast with a helpful error).
