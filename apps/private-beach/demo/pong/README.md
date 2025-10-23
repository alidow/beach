# Private Beach Pong Demo

This prototype contains two Python TUIs that exercise the Private Beach orchestration flow:

- `player/` – Paddle renderer that reacts to byte-sequence commands (`m`, `b`, `quit`).
- `agent/` – Manager-style controller that consumes terminal state diffs, tracks score, drives both paddles, and issues MCP-style actions.

## Harness Launcher

Each Tcl/TUI process should be wrapped by the Beach harness so diffs stream to the Private Beach manager. The helper `tools/launch_session.py` bootstraps a session, attaches it to your Private Beach, and keeps the underlying command interactive:

```bash
cd apps/private-beach/demo/pong
python3 tools/launch_session.py \
  --manager-url https://manager.private-beach.test/api \
  --private-beach-id <beach-id> \
  --auth-token $PB_MANAGER_TOKEN \
  --role lhs \
  -- python3 player/main.py --mode lhs

# In another terminal for the right paddle
python3 tools/launch_session.py \
  --manager-url https://manager.private-beach.test/api \
  --private-beach-id <beach-id> \
  --auth-token $PB_MANAGER_TOKEN \
  --role rhs \
  -- python3 player/main.py --mode rhs

# And once more for the agent
python3 tools/launch_session.py \
  --manager-url https://manager.private-beach.test/api \
  --private-beach-id <beach-id> \
  --auth-token $PB_MANAGER_TOKEN \
  --role agent \
  -- python3 agent/main.py --mcp-base-url https://manager.private-beach.test/api
```

The launcher captures the bootstrap handshake, auto-attaches the session, stores metadata (`pong_role`, `pong_tag`), and forwards stdin/stdout so you retain a fully interactive TUI. Metadata tags allow the agent to auto-discover and pair with paddle sessions.

## Player TUI

When invoked manually (outside the harness), type newline-terminated ASCII commands directly into each terminal (or pipe them from the agent):

- `m <delta>` – Move paddle vertically by `<delta>` rows. Positive values move the paddle up; negative values move it down.
- `b <y> <dx> <dy>` – Spawn a ball at the edge opposite the paddle. `<y>` is the starting vertical position; `<dx>`/`<dy>` are velocity components. The horizontal velocity is automatically adjusted so the ball travels toward the paddle.
- `quit` / `exit` – Close the TUI.

The current input buffer, last status message, and command reference appear at the bottom of the screen. The ball bounces off the top/bottom walls and the paddle; it despawns once it leaves the arena.

## Agent TUI (WIP Harness Emulator)

The agent lives in `agent/` and is responsible for:

- Rendering Claude-style conversation UI with a prompt box.
- Ingesting structured terminal diffs (matching the `terminal_full` payload emitted by Beach Buggy).
- Running basic Pong logic to align both paddles with the observed ball position.
- Issuing MCP `terminal_write` actions to the parent harness to move paddles or spawn the ball.

See `agent/README.md` for setup, wiring notes, and local testing helpers.

## Smoke Test Helper

`tools/smoke_test.py` performs a lightweight validation pass against Beach Manager:

```bash
python3 tools/smoke_test.py \
  --manager-url https://manager.private-beach.test/api \
  --private-beach-id <beach-id> \
  --auth-token $PB_MANAGER_TOKEN
```

It ensures that paddle sessions are attached with the expected metadata, verifies at least one agent session exists, and checks controller pairings for each agent → paddle relationship. The script exits non-zero if any requirement fails, making it safe to wire into CI or pre-flight scripts.
