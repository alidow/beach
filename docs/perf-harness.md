# Beach-Human Performance Harness

This harness gives us a quick sanity check that the packed binary protocol is
shrinking payloads before we run interactive benchmarks.

## Quick payload comparison

```bash
# Run the harness (ignored by default to keep CI fast)
cargo test -p beach-human --test perf_harness -- --ignored --show-output
```

The test prints JSON vs. binary payload sizes for a representative handshake +
steady-state frame sequence. You should see the binary column at least 60% of
the JSON total. Re-run the command after protocol tweaks to make sure the
savings hold steady.

## Measuring with live binaries

1. Enable binary framing and perf counters:
   ```bash
   export BEACH_PROTO_BINARY=1
   export BEACH_HUMAN_PROFILE=1
   RUST_LOG=perf=debug,beach_human=debug cargo run -p beach-human -- --local-preview
   ```
2. Drive an editor workload (e.g. Vim) for ~30 seconds.
3. Gather stats: perf counters are emitted through tracing; redirect the log and
   inspect lines such as `sync_send_bytes`, `client_handle_frame`, and
   `pty_chunk_process`.
4. Repeat without `BEACH_PROTO_BINARY` set to compare byte/latency deltas.

For full comparisons vs. SSH+tmux, the existing `docs/dual-channel-implementation-plan.md`
benchmark checklist still applies—use this new harness to verify protocol-level
savings before capturing end-to-end timings.
