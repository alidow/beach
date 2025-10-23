#!/usr/bin/env python3
"""
Minimal curses-based Pong paddle TUI for Private Beach demos.

The app renders a single paddle and optionally a moving ball. It accepts
byte-sequence commands (newline-terminated ASCII) on stdin:
  - `m <delta>`: Move paddle vertically by <delta> rows (positive=up).
  - `b <y> <dx> <dy>`: Spawn a ball at screen edge opposite the paddle with
                        the given vertical coordinate and velocity vector.
  - `quit` / `exit`: Terminate the TUI.

Run two instances (lhs/rhs) to represent both paddles:
  python main.py --mode lhs
  python main.py --mode rhs
"""

from __future__ import annotations

import argparse
import curses
import sys
import time
from dataclasses import dataclass
from typing import Optional, Tuple


DEFAULT_FPS = 30.0
PADDLE_HEIGHT = 5
PADDLE_MARGIN_X = 3
INSTRUCTION_LINES = 3  # Lines reserved at bottom


@dataclass
class Ball:
    x: float
    y: float
    vx: float
    vy: float


@dataclass
class Paddle:
    x: int
    y: float  # Center position


class PongView:
    def __init__(self, stdscr: "curses._CursesWindow", mode: str, fps: float) -> None:
        self.stdscr = stdscr
        self.mode = mode
        self.fps = fps
        self.running = True

        self.height = 0
        self.width = 0

        self.paddle = Paddle(x=0, y=0.0)
        self.ball: Optional[Ball] = None

        self.command_buffer = bytearray()
        self.status_message = "Ready."
        self.last_frame_time = time.monotonic()

    def log_status(self, message: str) -> None:
        self.status_message = message

    def update_dimensions(self) -> None:
        self.height, self.width = self.stdscr.getmaxyx()
        usable_width = max(self.width - 2, 1)
        if self.mode == "lhs":
            paddle_x = min(PADDLE_MARGIN_X, usable_width)
        else:
            paddle_x = max(self.width - PADDLE_MARGIN_X - 1, 1)
        paddle_y = max((self.height - INSTRUCTION_LINES) / 2, PADDLE_HEIGHT / 2 + 1)
        self.paddle.x = max(1, min(paddle_x, max(self.width - 2, 1)))
        self.paddle.y = self._clamp(
            self.paddle.y or paddle_y,
            self._paddle_min_y(),
            self._paddle_max_y(),
        )

    def _play_area_height(self) -> int:
        return max(self.height - INSTRUCTION_LINES, 0)

    def _paddle_min_y(self) -> float:
        return PADDLE_HEIGHT / 2 + 1

    def _paddle_max_y(self) -> float:
        return self._play_area_height() - (PADDLE_HEIGHT / 2)

    def move_paddle(self, delta: float) -> None:
        new_y = self.paddle.y - delta  # negative delta moves down
        self.paddle.y = self._clamp(new_y, self._paddle_min_y(), self._paddle_max_y())

    def spawn_ball(self, y: float, vx: float, vy: float) -> None:
        if self.width <= 0 or self.height <= INSTRUCTION_LINES:
            self.log_status("Cannot spawn ball: screen too small.")
            return

        play_top = 1
        play_bottom = self._play_area_height() - 1

        clamped_y = self._clamp(y, play_top + 1, play_bottom - 1)
        abs_vx = abs(vx) if vx != 0 else 1.0
        if self.mode == "lhs":
            x = float(self.width - PADDLE_MARGIN_X - 1)
            vx_final = -abs_vx
        else:
            x = float(PADDLE_MARGIN_X + 1)
            vx_final = abs_vx

        self.ball = Ball(x=x, y=clamped_y, vx=vx_final, vy=vy)
        self.log_status(
            f"Ball spawned at y={clamped_y:.1f} with velocity ({vx_final:.1f}, {vy:.1f})."
        )

    def _process_command(self, command: str) -> None:
        if not command:
            return
        if command in {"quit", "exit"}:
            self.running = False
            return
        if command.startswith("m"):
            parts = command.split()
            if len(parts) != 2:
                self.log_status("Usage: m <delta>")
                return
            try:
                delta = float(parts[1])
            except ValueError:
                self.log_status("Invalid delta. Provide a number.")
                return
            self.move_paddle(delta)
            self.log_status(f"Paddle moved by {delta:.1f}.")
            return
        if command.startswith("b"):
            parts = command.split()
            if len(parts) != 4:
                self.log_status("Usage: b <y> <dx> <dy>")
                return
            try:
                y_val = float(parts[1])
                dx = float(parts[2])
                dy = float(parts[3])
            except ValueError:
                self.log_status("Invalid ball parameters. Use numbers.")
                return
            self.spawn_ball(y_val, dx, dy)
            return
        self.log_status(f"Unknown command: {command}")

    def _read_input(self) -> None:
        while True:
            ch = self.stdscr.getch()
            if ch == -1:
                break
            if ch in (curses.KEY_RESIZE,):
                continue
            if ch in (3, 4):  # Ctrl+C / Ctrl+D
                self.running = False
                return
            if ch in (10, 13):  # Enter
                command = self.command_buffer.decode("ascii", errors="ignore").strip()
                self.command_buffer.clear()
                self._process_command(command)
                continue
            if ch in (8, 127):  # Backspace
                if self.command_buffer:
                    self.command_buffer = self.command_buffer[:-1]
                continue
            # Filter to printable ASCII range
            if 32 <= ch <= 126 or ch == 9:
                self.command_buffer.append(ch)

    def _clamp(self, value: float, minimum: float, maximum: float) -> float:
        if minimum > maximum:
            return (minimum + maximum) / 2
        return max(minimum, min(value, maximum))

    def _update_ball(self, dt: float) -> None:
        if not self.ball:
            return

        ball = self.ball
        ball.x += ball.vx * dt
        ball.y += ball.vy * dt

        top_limit = 1
        bottom_limit = self._play_area_height() - 1

        if ball.y <= top_limit:
            ball.y = top_limit + (top_limit - ball.y)
            ball.vy = -ball.vy
        elif ball.y >= bottom_limit:
            ball.y = bottom_limit - (ball.y - bottom_limit)
            ball.vy = -ball.vy

        # Paddle collision
        paddle_top = self.paddle.y - PADDLE_HEIGHT / 2
        paddle_bottom = self.paddle.y + PADDLE_HEIGHT / 2

        if self.mode == "lhs":
            if ball.x <= self.paddle.x + 1 and paddle_top <= ball.y <= paddle_bottom:
                ball.x = self.paddle.x + 1 + (self.paddle.x + 1 - ball.x)
                ball.vx = abs(ball.vx)
        else:
            if ball.x >= self.paddle.x - 1 and paddle_top <= ball.y <= paddle_bottom:
                ball.x = self.paddle.x - 1 - (ball.x - (self.paddle.x - 1))
                ball.vx = -abs(ball.vx)

        # Out of bounds
        if ball.x < 1 or ball.x > self.width - 2:
            self.ball = None
            self.log_status("Ball left the arena.")

    def _draw(self) -> None:
        self.stdscr.erase()

        play_height = min(self._play_area_height(), self.height)
        if self.width >= 2:
            for row in range(play_height):
                if row >= self.height:
                    break
                self.stdscr.addch(row, 0, "|")
                self.stdscr.addch(row, self.width - 1, "|")
        if play_height < self.height:
            for col in range(self.width):
                self.stdscr.addch(play_height, col, "-")

        # Center divider for visual alignment
        if self.width >= 2:
            for row in range(1, play_height):
                if row % 2 == 0 and 0 <= row < self.height:
                    mid_x = self.width // 2
                    if 0 <= mid_x < self.width:
                        self.stdscr.addch(row, mid_x, "|")

        # Paddle
        top = int(round(self.paddle.y - PADDLE_HEIGHT / 2))
        bottom = int(round(self.paddle.y + PADDLE_HEIGHT / 2))
        for row in range(top, bottom + 1):
            if 1 <= row < play_height and 0 <= self.paddle.x < self.width:
                self.stdscr.addch(row, self.paddle.x, "#")

        # Ball
        if self.ball:
            bx = int(round(self.ball.x))
            by = int(round(self.ball.y))
            if 1 <= by < play_height and 1 <= bx < self.width - 1:
                self.stdscr.addch(by, bx, "o")

        # HUD
        instruction_y = self.height - INSTRUCTION_LINES
        if instruction_y >= 0:
            controls = "Commands: m <delta> | b <y> <dx> <dy> | quit"
            self._addstr_safe(
                instruction_y,
                1,
                controls,
            )
            mode_hint = (
                f"Mode: {self.mode.upper()} — Paddle X={self.paddle.x}"
                " (positive delta moves up)"
            )
            self._addstr_safe(instruction_y + 1, 1, mode_hint)
            buffer_display = self.command_buffer.decode("ascii", errors="ignore")
            self._addstr_safe(
                instruction_y + 2,
                1,
                f"> {buffer_display}",
            )

        # Status message just above instruction area if space allows
        status_y = instruction_y - 1
        if status_y >= 0:
            self._addstr_safe(status_y, 1, self.status_message)

        self.stdscr.refresh()

    def _addstr_safe(self, y: int, x: int, text: str) -> None:
        if y < 0 or y >= self.height:
            return
        max_len = max(self.width - x - 1, 0)
        if max_len <= 0:
            return
        trimmed = text[:max_len]
        self.stdscr.addstr(y, x, trimmed)

    def run(self) -> None:
        curses.curs_set(0)
        self.stdscr.nodelay(True)
        frame_delay = 1.0 / self.fps if self.fps > 0 else 0.033

        while self.running:
            self.update_dimensions()
            now = time.monotonic()
            dt = now - self.last_frame_time
            if dt <= 0:
                dt = frame_delay
            self.last_frame_time = now

            self._read_input()
            self._update_ball(dt)
            self._draw()

            if frame_delay > 0:
                time.sleep(frame_delay)


def parse_args(argv: Tuple[str, ...]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Private Beach Pong Paddle TUI")
    parser.add_argument(
        "--mode",
        choices=("lhs", "rhs"),
        default="lhs",
        help="Which side to render. Run two instances for both paddles.",
    )
    parser.add_argument(
        "--fps",
        type=float,
        default=DEFAULT_FPS,
        help="Target frame rate for rendering/physics.",
    )
    return parser.parse_args(argv)


def curses_main(stdscr: "curses._CursesWindow", args: argparse.Namespace) -> None:
    view = PongView(stdscr, mode=args.mode, fps=args.fps)
    view.run()


def main(argv: Optional[Tuple[str, ...]] = None) -> int:
    args = parse_args(tuple(argv) if argv is not None else tuple(sys.argv[1:]))
    try:
        curses.wrapper(curses_main, args)
    except KeyboardInterrupt:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
