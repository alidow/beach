# Pong Automation Agent

`apps/private-beach/demo/pong/agent/main.py` hosts a curses-based “Claude code” style console that emulates the Private Beach manager agent. It ingests terminal state diffs, visualises them, and issues MCP `terminal_write` commands to steer the paddle TUIs in `../player`.

## Features

- Accepts newline-delimited JSON frames that match the `StateDiff` payload emitted by the Beach Buggy harness (`{"session_id": "...", "sequence": 42, "payload": {"type": "terminal_full", "lines": [...], "cursor": {...}}}`).
- Automatically drives each mapped session: aligns paddles with the tracked ball, hands the ball off between paddles, and updates a running score.
- Emits MCP-style actions (`terminal_write` with newline-terminated byte payloads) to a log file and/or a downstream TCP sink.
- Provides a Claude-code inspired UI: rolling log on the left, per-session telemetry on the right, and a prompt box for manual commands (`pause`, `resume`, `serve`, `m <session> <delta>`, `quit`).

## Quick Start

1. Launch two paddle TUIs (see `../player/README.md`). If you use beach-manager pairing, note the actual session IDs; for local experiments you can invent IDs (`lhs-demo`, `rhs-demo`).
2. Start the agent (via the harness launcher or directly):

   ```bash
   python3 main.py \
     --mcp-base-url https://manager.private-beach.test/api \
     --mcp-token $PB_MANAGER_TOKEN \
     --private-beach-id <beach-id> \
     --session-tag $PONG_SESSION_TAG \
     --action-log ./actions.jsonl
   ```

   - `--private-beach-id` enables automatic discovery and controller pairing.
   - `--session-tag` (defaults to `$PONG_SESSION_TAG`) narrows discovery when multiple agents run in the same beach.
   - `--session` may still be provided to override role mappings manually (`lhs:session-id`).
   - `--actions-target` (optional) forwards every MCP action to a TCP sink as JSON lines.

3. The agent subscribes to each paddle’s SSE state stream, renews controller leases, and drives the paddles autonomously. A running score appears in the header; the log pane records spawn/scoring events and every `terminal_write` command (`actions.jsonl` captures the raw payloads).

## State Stream Reference

The agent consumes Beach Manager’s SSE endpoint (`/sessions/:id/state/stream`). Each `data:` frame contains a full `StateDiff` payload:

```json
{
  "sequence": 42,
  "emitted_at": "2025-07-04T18:21:13.123Z",
  "payload": {
    "type": "terminal_full",
    "lines": [
      "|                                                                          |",
      "|   #                                                                      |",
      "...",
      "Commands: m <delta> | b <y> <dx> <dy> | quit"
    ],
    "cursor": null
  }
}
```

- Only `payload.type == "terminal_full"` is processed today.
- Rows should represent the visible terminal after trimming trailing spaces (matching the current harness behaviour).
- Sequences are monotonic per session; older frames are ignored.

## Prompt Commands

- `pause` / `resume` – toggle the autopilot loop.
- `serve [session|side]` – immediately spawn a ball for the given session (defaults to random choice).
- `m <session|side> <delta>` – manually move a paddle by delta rows.
- `token <session|side> <value>` – assign or override controller tokens for `queue_action`. Use `token default <value>` (or `token * <value>`) to set a fallback token; omit the value to clear it.
- `actions` – print the count of recorded MCP actions.
- `quit` – exit the TUI.

## Integration Notes

- MCP actions use newline-terminated byte payloads (`m 1.5\n`). Ensure downstream transports honour raw bytes—no additional quoting.
- The built-in action forwarder is intentionally simple. Swap it out for the real `private_beach.queue_action` bridge when wiring into the harness.
- Autopilot physics are intentionally conservative: velocity is inferred from consecutive frames; the paddle aims slightly ahead based on vertical velocity. Tweak `--max-step`, `--min-threshold`, `--serve-interval`, `--serve-dx`, and `--serve-dy` to alter behaviour.
- Credentials can be supplied through CLI flags (`--mcp-base-url`, `--mcp-token`, `--default-controller-token`, `--session-token session=token`) or environment variables (`PB_MCP_BASE_URL`, `PB_MCP_TOKEN`, `PB_MANAGER_TOKEN`, `PB_BUGGY_TOKEN`, `PB_CONTROLLER_TOKEN`). Interactive prompts are shown only when standard input is a TTY.

## Testing Helpers

- `sample-diff.jsonl` (alongside this README) remains useful for offline experiments—pipe it through the harness launcher to replay deterministic frames.
- Run with `--action-log` to inspect emitted commands without wiring a transport.
- Pair the agent with `tools/smoke_test.py` to ensure the Private Beach has the expected paddle/agent assignments before recording demos.
