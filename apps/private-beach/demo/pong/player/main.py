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
import json
import os
import sys
import time
from dataclasses import dataclass
from typing import Optional, Tuple, Set


FRAME_DELAY_SECONDS = 1.0 / 30.0
PADDLE_HEIGHT = 5
PADDLE_MARGIN_X = 3
INSTRUCTION_LINES = 3  # Lines reserved at bottom

MIN_PLAYFIELD_WIDTH = 20
MIN_PLAYFIELD_HEIGHT = PADDLE_HEIGHT + 4

FRAME_TOP_LEFT = "╭"
FRAME_TOP_RIGHT = "╮"
FRAME_BOTTOM_LEFT = "╰"
FRAME_BOTTOM_RIGHT = "╯"
FRAME_HORIZONTAL = "─"
FRAME_VERTICAL = "│"

CENTER_NET_PATTERN = ("┊", "┆")
BALL_GLYPH = "●"
PADDLE_KEYSTEP = 1.0

TITLE_TEMPLATE = " PRIVATE BEACH PONG · {mode} "
STATUS_ICON = "●"
COMMAND_ICON = "⌨"
PROMPT_ICON = "›"
DIVIDER_DOT = "·"

PADDLE_GLYPHS = {
    "lhs": {"cap_top": "▛", "body": "▌", "cap_bottom": "▙", "accent": "▏"},
    "rhs": {"cap_top": "▜", "body": "▐", "cap_bottom": "▟", "accent": "▕"},
}


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
    def __init__(self, stdscr: "curses._CursesWindow", mode: str) -> None:
        self.stdscr = stdscr
        self.mode = mode
        self.running = True

        self.height = 0
        self.width = 0

        self.paddle = Paddle(x=0, y=0.0)
        self.ball: Optional[Ball] = None

        self.command_buffer = bytearray()
        self.status_message = "Ready."
        self.last_frame_time = time.monotonic()
        self._last_hud_rows: Set[int] = set()
        self._last_paddle_cells: Set[Tuple[int, int]] = set()
        self._last_ball_cell: Optional[Tuple[int, int]] = None
        self.frame_dump_path = os.environ.get("PONG_FRAME_DUMP_PATH")
        self.verbose_diag = os.environ.get("PONG_VERBOSE_DIAG") not in (None, "", "0", "false", "False", "no")
        interval_env = os.environ.get("PONG_FRAME_DUMP_INTERVAL")
        try:
            self.frame_dump_interval = float(interval_env) if interval_env else 0.0
        except ValueError:
            self.frame_dump_interval = 0.0
        self._next_frame_dump_time = 0.0
        self.ball_trace_path = os.environ.get("PONG_BALL_TRACE_PATH")
        self.command_trace_path = os.environ.get("PONG_COMMAND_TRACE_PATH")
        if self.verbose_diag and self.command_trace_path:
            self._trace_diag(
                "init",
                {
                    "mode": self.mode,
                    "frame_dump_path": self.frame_dump_path,
                    "ball_trace_path": self.ball_trace_path,
                    "command_trace_path": self.command_trace_path,
                },
            )

        self._colors_initialized = False
        self.color_border = curses.A_BOLD
        self.color_centerline = curses.A_DIM
        self.color_paddle = curses.A_BOLD
        self.color_paddle_cap = curses.A_BOLD
        self.color_ball = curses.A_BOLD
        self.color_status = curses.A_BOLD
        self.color_commands = curses.A_NORMAL
        self.color_prompt = curses.A_BOLD
        self.color_title = curses.A_BOLD

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
        max_value = self._play_area_height() - (PADDLE_HEIGHT / 2) - 2
        min_value = self._paddle_min_y()
        if max_value < min_value:
            return min_value
        return max_value

    def move_paddle(self, delta: float) -> None:
        new_y = self.paddle.y - delta  # negative delta moves down
        self.paddle.y = self._clamp(new_y, self._paddle_min_y(), self._paddle_max_y())

    def spawn_ball(self, y: float, vx: float, vy: float) -> None:
        if self.width <= 0 or self.height <= INSTRUCTION_LINES:
            self.log_status("Cannot spawn ball: screen too small.")
            if self.verbose_diag:
                self._trace_diag("spawn_ball_blocked", {"width": self.width, "height": self.height})
            return

        play_top = 1
        play_bottom = self._play_area_height() - 2

        if play_bottom <= play_top:
            self.log_status("Cannot spawn ball: play area too small.")
            return

        lower_limit = play_top + 1
        upper_limit = play_bottom - 1
        if upper_limit <= lower_limit:
            clamped_y = (lower_limit + upper_limit) / 2
        else:
            clamped_y = self._clamp(y, lower_limit, upper_limit)
        abs_vx = abs(vx) if vx != 0 else 1.0
        if self.mode == "lhs":
            x = float(self.width - PADDLE_MARGIN_X - 2)
            vx_final = -abs_vx
        else:
            x = float(PADDLE_MARGIN_X + 1)
            vx_final = abs_vx

        self.ball = Ball(x=x, y=clamped_y, vx=vx_final, vy=vy)
        self.log_status(
            f"Ball spawned at y={clamped_y:.1f} with velocity ({vx_final:.1f}, {vy:.1f})."
        )

    def _process_command(self, command: str) -> None:
        self._trace_command(command)
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
        if self.verbose_diag:
            self._trace_diag(
                "command_applied",
                {
                    "command": command,
                    "ball": bool(self.ball),
                    "width": self.width,
                    "height": self.height,
                },
            )

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
            if ch == curses.KEY_UP:
                self.move_paddle(PADDLE_KEYSTEP)
                self.log_status("Paddle moved up.")
                continue
            if ch == curses.KEY_DOWN:
                self.move_paddle(-PADDLE_KEYSTEP)
                self.log_status("Paddle moved down.")
                continue
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

    def _init_colors(self) -> None:
        if self._colors_initialized:
            return
        self._colors_initialized = True

        if not curses.has_colors():
            # Fall back to attribute-only styling.
            self.color_border = curses.A_BOLD
            self.color_centerline = curses.A_DIM
            self.color_paddle = curses.A_BOLD
            self.color_paddle_cap = curses.A_BOLD
            self.color_ball = curses.A_BOLD
            self.color_status = curses.A_BOLD
            self.color_commands = curses.A_NORMAL
            self.color_prompt = curses.A_BOLD
            self.color_title = curses.A_BOLD
            return

        curses.start_color()
        try:
            curses.use_default_colors()
        except curses.error:
            # Some terminals do not support default color merging; ignore.
            pass

        curses.init_pair(1, curses.COLOR_CYAN, -1)
        curses.init_pair(2, curses.COLOR_MAGENTA, -1)
        curses.init_pair(3, curses.COLOR_YELLOW, -1)
        curses.init_pair(4, curses.COLOR_WHITE, -1)
        curses.init_pair(5, curses.COLOR_GREEN, -1)

        self.color_border = curses.color_pair(1) | curses.A_BOLD
        self.color_centerline = curses.color_pair(1) | curses.A_DIM
        self.color_paddle = curses.color_pair(2) | curses.A_BOLD
        self.color_paddle_cap = curses.color_pair(2) | curses.A_BOLD
        self.color_ball = curses.color_pair(3) | curses.A_BOLD
        self.color_status = curses.color_pair(4) | curses.A_BOLD
        self.color_commands = curses.color_pair(4)
        self.color_prompt = curses.color_pair(5) | curses.A_BOLD
        self.color_title = curses.color_pair(1) | curses.A_BOLD

    def _update_ball(self, dt: float) -> None:
        if not self.ball:
            return

        ball = self.ball
        ball.x += ball.vx * dt
        ball.y += ball.vy * dt

        top_limit = 1
        bottom_limit = self._play_area_height() - 2

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

        self._trace_ball_position()
        # Out of bounds
        if ball.x < 1 or ball.x > self.width - 2:
            self.ball = None
            self.log_status("Ball left the arena.")

    def _draw(self) -> None:
        self.stdscr.erase()

        play_height = self._play_area_height()

        if self.height < INSTRUCTION_LINES + 2 or self.width < 3:
            self._render_resize_hint("Waiting for viewer…")
            self.stdscr.refresh()
            return

        if play_height < MIN_PLAYFIELD_HEIGHT or self.width < MIN_PLAYFIELD_WIDTH:
            self._render_resize_hint("Resize for the full Pong experience.")
            self.stdscr.refresh()
            return

        play_top = 0
        play_bottom = play_height - 1
        inner_top = play_top + 1
        inner_bottom = play_bottom - 1
        inner_left = 1
        inner_right = self.width - 2

        for row in range(play_height, self.height):
            self._clear_line(row)

        self._draw_frame(play_top, play_bottom)
        self._clear_play_area(inner_top, inner_bottom, inner_left, inner_right)
        self._draw_centerline(inner_top, inner_bottom)
        self._draw_paddle(inner_top, inner_bottom, inner_left, inner_right)
        self._draw_ball(inner_top, inner_bottom, inner_left, inner_right)
        self._draw_hud(play_bottom)

        self._maybe_dump_frame()

        # Force the entire window to be considered dirty so every refresh
        # emits a complete frame. This helps environments that fall out of
        # sync when incremental terminal escape sequences are dropped.
        try:
            self.stdscr.touchwin()
        except curses.error:
            pass
        self.stdscr.refresh()

    def _render_resize_hint(self, message: str) -> None:
        for row in self._last_hud_rows:
            self._clear_line(row)
        self._last_hud_rows.clear()

        hint_y = max(self.height // 2, 0)
        intro = message
        detail = "Expand the terminal to enjoy Private Beach Pong."

        intro_x = max((self.width - len(intro)) // 2, 0)
        detail_x = max((self.width - len(detail)) // 2, 0)

        self._addstr_safe(hint_y, intro_x, intro, self.color_status)
        if hint_y + 2 < self.height:
            self._addstr_safe(hint_y + 2, detail_x, detail, self.color_commands)

    def _draw_frame(self, top: int, bottom: int) -> None:
        if self.width < 2 or bottom <= top:
            return

        horizontal_width = max(self.width - 2, 0)
        top_line = (
            FRAME_TOP_LEFT
            + (FRAME_HORIZONTAL * horizontal_width)
            + FRAME_TOP_RIGHT
        )
        bottom_line = (
            FRAME_BOTTOM_LEFT
            + (FRAME_HORIZONTAL * horizontal_width)
            + FRAME_BOTTOM_RIGHT
        )

        self._addstr_safe(top, 0, top_line, self.color_border)
        self._addstr_safe(bottom, 0, bottom_line, self.color_border)

        for row in range(top + 1, bottom):
            self._addch_safe(row, 0, FRAME_VERTICAL, self.color_border)
            self._addch_safe(row, self.width - 1, FRAME_VERTICAL, self.color_border)

        self._draw_frame_title(top)

    def _draw_frame_title(self, y: int) -> None:
        max_len = self.width - 2
        if max_len <= 2:
            return

        title = TITLE_TEMPLATE.format(mode=self.mode.upper())
        if len(title) > max_len:
            if max_len <= 1:
                return
            title = title[: max_len - 1] + "…"

        start_x = max((self.width - len(title)) // 2, 1)
        if start_x + len(title) >= self.width:
            start_x = self.width - len(title) - 1
            if start_x < 1:
                return

        self._addstr_safe(y, start_x, title, self.color_title)

    def _draw_centerline(self, inner_top: int, inner_bottom: int) -> None:
        if inner_top > inner_bottom or self.width < 3:
            return
        mid_x = self.width // 2
        pattern_length = len(CENTER_NET_PATTERN)
        if pattern_length == 0:
            return
        for offset, row in enumerate(range(inner_top, inner_bottom + 1)):
            glyph = CENTER_NET_PATTERN[offset % pattern_length]
            self._addch_safe(row, mid_x, glyph, self.color_centerline)

    def _draw_paddle(
        self,
        inner_top: int,
        inner_bottom: int,
        inner_left: int,
        inner_right: int,
    ) -> None:
        if self._last_paddle_cells:
            for row, col in self._last_paddle_cells:
                if inner_top <= row <= inner_bottom and inner_left <= col <= inner_right:
                    self._addch_safe(row, col, " ")
            self._last_paddle_cells.clear()

        glyph_set = PADDLE_GLYPHS["lhs" if self.mode == "lhs" else "rhs"]
        half_height = PADDLE_HEIGHT / 2
        top_row = int(round(self.paddle.y - half_height))
        bottom_row = int(round(self.paddle.y + half_height))

        rows = [
            row
            for row in range(top_row, bottom_row + 1)
            if inner_top <= row <= inner_bottom
        ]
        if not rows:
            self._last_paddle_cells = set()
            return

        new_cells: Set[Tuple[int, int]] = set()
        for index, row in enumerate(rows):
            if len(rows) > 1 and index == 0:
                glyph = glyph_set["cap_top"]
                attr = self.color_paddle_cap
            elif len(rows) > 1 and index == len(rows) - 1:
                glyph = glyph_set["cap_bottom"]
                attr = self.color_paddle_cap
            else:
                glyph = glyph_set["body"]
                attr = self.color_paddle

            self._addch_safe(row, self.paddle.x, glyph, attr)
            new_cells.add((row, self.paddle.x))

            accent_x = self.paddle.x + (1 if self.mode == "lhs" else -1)
            if inner_left <= accent_x <= inner_right:
                self._addch_safe(
                    row,
                    accent_x,
                    glyph_set["accent"],
                    self.color_paddle | curses.A_DIM,
                )
                new_cells.add((row, accent_x))

        self._last_paddle_cells = new_cells

    def _draw_ball(
        self,
        inner_top: int,
        inner_bottom: int,
        inner_left: int,
        inner_right: int,
    ) -> None:
        if self._last_ball_cell:
            last_y, last_x = self._last_ball_cell
            if inner_top <= last_y <= inner_bottom and inner_left <= last_x <= inner_right:
                self._addch_safe(last_y, last_x, " ")
            self._last_ball_cell = None

        if not self.ball:
            return

        bx = int(round(self.ball.x))
        by = int(round(self.ball.y))
        if inner_left <= bx <= inner_right and inner_top <= by <= inner_bottom:
            self._addch_safe(by, bx, BALL_GLYPH, self.color_ball)
            self._last_ball_cell = (by, bx)

    def _draw_hud(self, play_bottom: int) -> None:
        status_y = play_bottom + 1
        commands_y = status_y + 1
        prompt_y = commands_y + 1

        hud_rows = {row for row in (status_y, commands_y, prompt_y) if 0 <= row < self.height}

        for row in self._last_hud_rows:
            if row not in hud_rows:
                self._clear_line(row)

        status_parts = [
            f"{STATUS_ICON} {self.status_message}",
            f"Mode {self.mode.upper()} @ X{self.paddle.x}",
        ]
        if self.ball:
            status_parts.append(
                f"Ball {int(round(self.ball.x))},{int(round(self.ball.y))}"
            )
        status_line = f" {DIVIDER_DOT} ".join(status_parts)

        if status_y in hud_rows:
            self._clear_line(status_y)
            self._addstr_safe(status_y, 1, status_line, self.color_status)

        if commands_y in hud_rows:
            commands_line = (
                f"{COMMAND_ICON} Commands  m <Δ>   {DIVIDER_DOT}   b <y> <dx> <dy>   "
                f"{DIVIDER_DOT}   quit"
            )
            self._clear_line(commands_y)
            self._addstr_safe(commands_y, 1, commands_line, self.color_commands)

        if prompt_y in hud_rows:
            buffer_display = self.command_buffer.decode("ascii", errors="ignore")
            prompt_line = f"{PROMPT_ICON} {buffer_display}"
            self._clear_line(prompt_y)
            self._addstr_safe(prompt_y, 1, prompt_line, self.color_prompt)

        self._last_hud_rows = hud_rows

    def _maybe_dump_frame(self) -> None:
        if not self.frame_dump_path or self.frame_dump_interval <= 0:
            return

        now = time.monotonic()
        if now < self._next_frame_dump_time:
            return
        self._next_frame_dump_time = now + self.frame_dump_interval

        lines: list[str] = []
        for row in range(self.height):
            try:
                raw = self.stdscr.instr(row, 0, self.width)
            except curses.error:
                continue
            if not raw:
                lines.append("")
                continue
            try:
                text = raw.decode("utf-8", errors="ignore")
            except Exception:
                text = "".join(chr(b) if 32 <= b <= 126 else " " for b in raw)
            lines.append(text.rstrip("\n\r"))

        snapshot = "\n".join(lines)
        try:
            dump_dir = os.path.dirname(self.frame_dump_path)
            if dump_dir:
                os.makedirs(dump_dir, exist_ok=True)
            with open(self.frame_dump_path, "w", encoding="utf-8") as fp:
                fp.write(snapshot)
        except OSError:
            pass

    def _trace_ball_position(self) -> None:
        if not self.ball_trace_path or not self.ball:
            if not self.ball and self.ball_trace_path:
                # Diagnostic: note that we expected to trace but ball is missing.
                try:
                    trace_dir = os.path.dirname(self.ball_trace_path)
                    if trace_dir:
                        os.makedirs(trace_dir, exist_ok=True)
                    with open(self.ball_trace_path, "a", encoding="utf-8") as fp:
                        fp.write(json.dumps({"time": time.time(), "missing": True}))
                        fp.write("\n")
                except OSError:
                    pass
            return
        record = {
            "time": time.time(),
            "x": self.ball.x,
            "y": self.ball.y,
        }
        trace_dir = os.path.dirname(self.ball_trace_path)
        try:
            if trace_dir:
                os.makedirs(trace_dir, exist_ok=True)
            with open(self.ball_trace_path, "a", encoding="utf-8") as fp:
                fp.write(json.dumps(record))
                fp.write("\n")
        except OSError:
            pass

    def _trace_command(self, command: str) -> None:
        if not self.command_trace_path:
            return
        trace_dir = os.path.dirname(self.command_trace_path)
        try:
            if trace_dir:
                os.makedirs(trace_dir, exist_ok=True)
            with open(self.command_trace_path, "a", encoding="utf-8") as fp:
                fp.write(json.dumps({"time": time.time(), "command": command}))
                fp.write("\n")
        except OSError:
            pass

    def _trace_diag(self, kind: str, payload: Dict[str, Any]) -> None:
        if not self.command_trace_path:
            return
        trace_dir = os.path.dirname(self.command_trace_path)
        try:
            if trace_dir:
                os.makedirs(trace_dir, exist_ok=True)
            with open(self.command_trace_path, "a", encoding="utf-8") as fp:
                fp.write(json.dumps({"time": time.time(), "diag": kind, **payload}))
                fp.write("\n")
        except OSError:
            pass

    def _clear_play_area(
        self,
        inner_top: int,
        inner_bottom: int,
        inner_left: int,
        inner_right: int,
    ) -> None:
        width = inner_right - inner_left + 1
        if width <= 0 or inner_top > inner_bottom:
            return
        blank_line = " " * width
        for row in range(inner_top, inner_bottom + 1):
            self._addstr_safe(row, inner_left, blank_line)

    def _addstr_safe(self, y: int, x: int, text: str, attr: int = 0) -> None:
        if y < 0 or y >= self.height:
            return
        max_len = max(self.width - x, 0)
        if max_len <= 0:
            return
        trimmed = text[:max_len]
        if not trimmed:
            return
        try:
            self.stdscr.addstr(y, x, trimmed, attr)
        except curses.error:
            pass

    def _addch_safe(self, y: int, x: int, ch: str, attr: int = 0) -> None:
        if y < 0 or y >= self.height or x < 0 or x >= self.width:
            return
        try:
            self.stdscr.addch(y, x, ch, attr)
        except curses.error:
            pass

    def _clear_line(self, y: int) -> None:
        if y < 0 or y >= self.height:
            return
        try:
            self.stdscr.move(y, 0)
            self.stdscr.clrtoeol()
        except curses.error:
            pass

    def run(self) -> None:
        curses.curs_set(0)
        self.stdscr.nodelay(True)
        self._disable_insert_delete_sequences()
        self._init_colors()
        frame_delay = FRAME_DELAY_SECONDS

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

    def _disable_insert_delete_sequences(self) -> None:
        def _invoke(name: str) -> None:
            target = getattr(self.stdscr, name, None)
            if callable(target):
                try:
                    target(False)
                except curses.error:
                    return
                return
            func = getattr(curses, name, None)
            if callable(func):
                try:
                    func(self.stdscr, False)
                except curses.error:
                    return

        for method_name in ("idlok", "idcok"):
            _invoke(method_name)


def parse_args(argv: Tuple[str, ...]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Private Beach Pong Paddle TUI")
    parser.add_argument(
        "--mode",
        choices=("lhs", "rhs"),
        default="lhs",
        help="Which side to render. Run two instances for both paddles.",
    )
    return parser.parse_args(argv)


def curses_main(stdscr: "curses._CursesWindow", args: argparse.Namespace) -> None:
    view = PongView(stdscr, mode=args.mode)
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
