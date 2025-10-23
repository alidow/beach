#!/usr/bin/env python3
"""
Private Beach Pong automation agent.

This curses TUI emulates the manager-side agent session. It ingests terminal
state diffs (matching the `terminal_full` payload emitted by Beach Buggy),
tracks score/ball state across both paddles, renders a Claude-style prompt
interface, and drives paddle/ball control by issuing MCP `terminal_write`
actions.
"""

from __future__ import annotations

import argparse
import curses
import json
import os
import queue
import random
import socket
import threading
import time
import uuid
from dataclasses import dataclass, field
from getpass import getpass
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Tuple
import sys
import urllib.error
import urllib.parse
import urllib.request


TOOLS_DIR = Path(__file__).resolve().parent.parent / "tools"
if str(TOOLS_DIR) not in sys.path:  # pragma: no cover - import path shim
    sys.path.insert(0, str(TOOLS_DIR))

from manager_client import (  # type: ignore
    ControllerLease,
    ManagerRequestError,
    PrivateBeachManagerClient,
)


LOG_LIMIT = 200
PROMPT_BOX_HEIGHT = 3
DEFAULT_SERVE_INTERVAL = 3.0
DEFAULT_MAX_STEP = 2.5
DEFAULT_MIN_THRESHOLD = 0.4
DEFAULT_COMMAND_INTERVAL = 0.08


def parse_host_port(value: str) -> Optional[Tuple[str, int]]:
    value = value.strip()
    if value.lower() in {"", "none", "off"}:
        return None
    if ":" not in value:
        raise argparse.ArgumentTypeError("expected <host>:<port>")
    host, port_str = value.rsplit(":", 1)
    try:
        port = int(port_str)
    except ValueError as exc:  # pragma: no cover - defensive
        raise argparse.ArgumentTypeError(f"invalid port: {port_str}") from exc
    return host.strip(), port


def parse_session_mapping(values: Iterable[str]) -> Dict[str, str]:
    mapping: Dict[str, str] = {}
    auto_sides = ["lhs", "rhs"]
    for raw in values:
        item = raw.strip()
        if not item:
            continue
        side: Optional[str] = None
        session_id: Optional[str] = None
        if "=" in item:
            left, right = item.split("=", 1)
            # Support lhs=session-id and session-id=lhs forms.
            if left.strip() in {"lhs", "rhs"}:
                side = left.strip()
                session_id = right.strip()
            elif right.strip() in {"lhs", "rhs"}:
                side = right.strip()
                session_id = left.strip()
        elif ":" in item:
            left, right = item.split(":", 1)
            if right.strip() in {"lhs", "rhs"}:
                session_id = left.strip()
                side = right.strip()
            elif left.strip() in {"lhs", "rhs"}:
                side = left.strip()
                session_id = right.strip()
        if not session_id:
            session_id = item
        if not side:
            side = auto_sides[len(mapping) % len(auto_sides)]
        mapping[session_id] = side
    return mapping


def parse_token_mapping(values: Iterable[str]) -> Dict[str, str]:
    tokens: Dict[str, str] = {}
    for raw in values:
        item = raw.strip()
        if not item:
            continue
        if "=" not in item:
            raise argparse.ArgumentTypeError("token mapping must look like session=token")
        key, value = item.split("=", 1)
        key = key.strip()
        value = value.strip()
        if not key or not value:
            raise argparse.ArgumentTypeError("token mapping must look like session=token")
        tokens[key] = value
    return tokens


class StateSubscriber(threading.Thread):
    """Streams terminal diffs from the manager via SSE."""

    def __init__(
        self,
        client: PrivateBeachManagerClient,
        session_id: str,
        output: "queue.Queue[Tuple[str, object]]",
        stop_event: threading.Event,
        role: Optional[str] = None,
    ) -> None:
        super().__init__(daemon=True)
        self.client = client
        self.session_id = session_id
        self.output = output
        self.stop_event = stop_event
        self.role = role

    def run(self) -> None:  # pragma: no cover - network path
        label = self.role or "session"
        while not self.stop_event.is_set():
            try:
                for payload in self.client.subscribe_state(self.session_id):
                    event = {
                        "session_id": self.session_id,
                        "payload": payload,
                        "received_at": time.time(),
                    }
                    self.output.put(("diff", event))
                    if self.stop_event.is_set():
                        break
            except ManagerRequestError as exc:
                self.output.put(
                    (
                        "error",
                        f"state stream error ({label} {self.session_id}): {exc}",
                    )
                )
                if self.stop_event.wait(2.0):
                    break
            else:
                # Stream ended without error; avoid tight loop.
                if self.stop_event.wait(1.0):
                    break


class LeaseRenewer(threading.Thread):
    def __init__(
        self,
        client: PrivateBeachManagerClient,
        controller_session_id: str,
        ttl_ms: int,
        output: "queue.Queue[Tuple[str, object]]",
        stop_event: threading.Event,
        on_update,
        reason: Optional[str] = None,
    ) -> None:
        super().__init__(daemon=True)
        self.client = client
        self.controller_session_id = controller_session_id
        self.ttl_ms = ttl_ms
        self.output = output
        self.stop_event = stop_event
        self.on_update = on_update
        self.reason = reason

    def run(self) -> None:  # pragma: no cover - timer loop
        interval = max(self.ttl_ms / 2000.0, 5.0)
        while not self.stop_event.wait(interval):
            try:
                lease = self.client.acquire_controller_lease(
                    self.controller_session_id, self.ttl_ms, self.reason
                )
                self.on_update(lease)
                now_ms = int(time.time() * 1000)
                remaining_ms = max(lease.expires_at_ms - now_ms, 0)
                interval = max(remaining_ms / 2000.0, 5.0)
                self.output.put(
                    (
                        "info",
                        f"controller lease renewed; expires in {remaining_ms / 1000:.1f}s",
                    )
                )
            except ManagerRequestError as exc:
                interval = 5.0
                self.output.put(("error", f"controller lease renewal failed: {exc}"))
        self.output.put(("info", "controller lease renewer stopped"))


@dataclass
class AutopairContext:
    controller_session_id: str
    controller_token: str
    child_sessions: Dict[str, str]
    session_roles: Dict[str, str]
    lease_expires_at_ms: int


def _metadata_dict(value: Optional[object]) -> Dict[str, object]:
    if value is None:
        return {}
    if isinstance(value, dict):
        return value
    if isinstance(value, str):
        try:
            parsed = json.loads(value)
        except json.JSONDecodeError:
            return {}
        if isinstance(parsed, dict):
            return parsed
    return {}


def autopair_sessions(
    args:
        argparse.Namespace,
    client: PrivateBeachManagerClient,
    diff_queue: "queue.Queue[Tuple[str, object]]",
) -> Optional[AutopairContext]:
    private_beach_id = getattr(args, "private_beach_id", None)
    if not private_beach_id:
        return None
    session_tag = getattr(args, "session_tag", None) or os.environ.get(
        "PONG_SESSION_TAG"
    )
    attempts = getattr(args, "discovery_attempts", 12)
    interval = getattr(args, "discovery_interval", 1.0)

    agent_session_id: Optional[str] = None
    child_sessions: Dict[str, str] = {}

    for attempt in range(max(1, attempts)):
        try:
            summaries = list(client.list_sessions(private_beach_id))
        except ManagerRequestError as exc:
            diff_queue.put(("error", f"session discovery failed: {exc}"))
            if attempt == attempts - 1:
                return None
            time.sleep(interval)
            continue

        for summary in summaries:
            session_id = summary.get("session_id")
            metadata = _metadata_dict(summary.get("metadata"))
            role = metadata.get("pong_role")
            tag = metadata.get("pong_tag")

            if not isinstance(session_id, str):
                continue
            if isinstance(role, str) and role in {"lhs", "rhs"}:
                child_sessions.setdefault(role, session_id)
            if role == "agent":
                if session_tag is None and agent_session_id is None:
                    agent_session_id = session_id
                elif session_tag and tag == session_tag:
                    agent_session_id = session_id

        if agent_session_id and child_sessions:
            break
        if attempt < attempts - 1:
            time.sleep(interval)

    if not agent_session_id:
        diff_queue.put(("warn", "autopair: no agent session discovered"))
        return None

    if not child_sessions:
        diff_queue.put(("warn", "autopair: no paddle sessions discovered"))
        return None

    session_roles = {
        agent_session_id: "agent",
        **{sid: role for role, sid in child_sessions.items()},
    }

    prompt_template = getattr(args, "pair_template", None)
    update_cadence = getattr(args, "pair_cadence", None)

    for role, session_id in child_sessions.items():
        try:
            client.create_controller_pairing(
                agent_session_id, session_id, prompt_template, update_cadence
            )
            diff_queue.put(
                (
                    "info",
                    f"paired agent {agent_session_id} -> {role} session {session_id}",
                )
            )
        except ManagerRequestError as exc:
            diff_queue.put(
                (
                    "error",
                    f"failed to pair agent with {role} session {session_id}: {exc}",
                )
            )

    lease_reason = getattr(args, "lease_reason", "pong_autopilot")
    lease_ttl = getattr(args, "lease_ttl", 30_000)
    try:
        lease = client.acquire_controller_lease(
            agent_session_id, lease_ttl, lease_reason
        )
        diff_queue.put(
            (
                "info",
                f"controller lease acquired (expires at {lease.expires_at_ms})",
            )
        )
    except ManagerRequestError as exc:
        diff_queue.put(("error", f"failed to acquire controller lease: {exc}"))
        return None

    return AutopairContext(
        controller_session_id=agent_session_id,
        controller_token=lease.controller_token,
        child_sessions=child_sessions,
        session_roles=session_roles,
        lease_expires_at_ms=lease.expires_at_ms,
    )


class MCPClient:
    """Thin MCP client that can queue actions over HTTP and/or a local sink."""

    def __init__(
        self,
        action_callback,
        base_url: Optional[str],
        auth_token: Optional[str],
        default_controller_token: Optional[str],
        session_tokens: Optional[Dict[str, str]],
        target: Optional[Tuple[str, int]] = None,
        log_path: Optional[Path] = None,
        timeout: float = 5.0,
    ) -> None:
        self._callback = action_callback
        if base_url:
            cleaned = base_url.strip()
            if cleaned and not cleaned.endswith("/"):
                cleaned = f"{cleaned}/"
            self._base_url = cleaned or None
        else:
            self._base_url = None
        self._auth_token = auth_token.strip() if auth_token else None
        self._default_controller_token = (
            default_controller_token.strip() if default_controller_token else None
        )
        self._session_tokens: Dict[str, str] = dict(session_tokens or {})
        self._timeout = timeout
        self._target = target
        self._lock = threading.Lock()
        self._socket: Optional[socket.socket] = None
        self._log_path = log_path
        self._log_fp = None
        if self._log_path:
            self._log_fp = self._log_path.open("a", encoding="utf-8")
        if self._target:
            try:
                self._socket = socket.create_connection(self._target)
            except OSError as exc:  # pragma: no cover - networking path
                self._socket = None
                self._callback(
                    {
                        "level": "error",
                        "message": f"failed to connect to action sink {self._target}: {exc}",
                    }
                )

    def set_session_token(self, session_id: str, token: str) -> None:
        self._session_tokens[session_id] = token

    def set_default_controller_token(self, token: Optional[str]) -> None:
        self._default_controller_token = token

    def current_session_token(self, session_id: str) -> Optional[str]:
        return self._session_tokens.get(session_id) or self._default_controller_token

    def queue_terminal_write(self, session_id: str, data: str) -> None:
        command_id = str(uuid.uuid4())
        action_payload = {
            "id": command_id,
            "action_type": "terminal_write",
            "payload": {"bytes": data},
        }
        transport_label = "log"
        status = "recorded"

        if self._base_url:
            transport_label = "http"
            success = self._send_http(session_id, action_payload)
            status = "sent" if success else "error"

        serialized = json.dumps(
            {
                "session_id": session_id,
                "action": action_payload,
                "timestamp": time.time(),
            },
            separators=(",", ":"),
        )
        if self._log_fp:
            self._log_fp.write(serialized + "\n")
            self._log_fp.flush()
        if self._socket:
            try:
                with self._lock:
                    self._socket.sendall(serialized.encode("utf-8") + b"\n")
            except OSError as exc:  # pragma: no cover - networking path
                self._callback(
                    {
                        "level": "error",
                        "message": f"action sink write failed ({exc}); disabling forwarder",
                    }
                )
                try:
                    self._socket.close()
                except OSError:
                    pass
                self._socket = None
            else:
                if transport_label == "log":
                    transport_label = "pipe"

        self._callback(
            {
                "level": "action",
                "session_id": session_id,
                "id": command_id,
                "command": data.strip(),
                "transport": transport_label,
                "status": status,
            }
        )

    def _send_http(self, session_id: str, action_payload: Dict[str, object]) -> bool:
        token = self.current_session_token(session_id)
        if not token:
            self._callback(
                {
                    "level": "warn",
                    "message": f"no controller token configured for {session_id}; skipping queue_action",
                }
            )
            return False

        if not self._base_url:
            return False

        encoded_session = urllib.parse.quote(session_id, safe="")
        url = urllib.parse.urljoin(
            self._base_url, f"sessions/{encoded_session}/actions"
        )
        body = json.dumps(
            {"controller_token": token, "actions": [action_payload]},
            separators=(",", ":"),
        ).encode("utf-8")
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json",
        }
        if self._auth_token:
            headers["Authorization"] = f"Bearer {self._auth_token}"

        request = urllib.request.Request(url, data=body, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(request, timeout=self._timeout) as response:
                # Drain response to allow connection reuse.
                response.read()
            return True
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="ignore")
            self._callback(
                {
                    "level": "error",
                    "message": f"queue_action HTTP {exc.code} for {session_id}: {detail}",
                }
            )
            return False
        except urllib.error.URLError as exc:
            self._callback(
                {
                    "level": "error",
                    "message": f"queue_action transport error for {session_id}: {exc.reason}",
                }
            )
            return False

    def close(self) -> None:
        if self._socket:  # pragma: no cover - cleanup path
            try:
                self._socket.close()
            except OSError:
                pass
            self._socket = None
        if self._log_fp:
            self._log_fp.close()
            self._log_fp = None


@dataclass
class SessionState:
    session_id: str
    side: Optional[str] = None
    last_sequence: int = 0
    lines: List[str] = field(default_factory=list)
    cursor: Optional[Tuple[int, int]] = None
    paddle_center: Optional[float] = None
    paddle_column: Optional[int] = None
    ball_position: Optional[Tuple[float, float]] = None
    previous_ball_position: Optional[Tuple[float, float]] = None
    previous_ball_time: Optional[float] = None
    ball_velocity: Optional[Tuple[float, float]] = None
    last_update: float = 0.0
    last_move_time: float = 0.0
    ball_exit: Optional[str] = None
    ball_exit_time: float = 0.0
    last_velocity: Optional[Tuple[float, float]] = None


    def apply_terminal_frame(
        self,
        lines: List[str],
        cursor: Optional[Tuple[int, int]],
        sequence: int,
        now: float,
    ) -> None:
        if sequence <= self.last_sequence:
            return
        self.last_sequence = sequence
        self.lines = lines
        self.cursor = cursor
        self.last_update = now
        self._detect_paddle()
        self._detect_ball(now)
        if self.ball:
            self.ball_exit = None
        else:
            self.ball_velocity = None

    def _detect_paddle(self) -> None:
        rows: List[int] = []
        cols: List[int] = []
        for row_idx, line in enumerate(self.lines):
            for col_idx, char in enumerate(line):
                if char == "#":
                    rows.append(row_idx)
                    cols.append(col_idx)
        if rows:
            self.paddle_center = sum(rows) / len(rows)
            col_avg = sum(cols) / len(cols)
            self.paddle_column = int(round(col_avg))
        else:
            self.paddle_center = None
            self.paddle_column = None

    def _detect_ball(self, now: float) -> None:
        found: Optional[Tuple[float, float]] = None
        for row_idx, line in enumerate(self.lines):
            col_idx = line.find("o")
            if col_idx != -1:
                found = (float(row_idx), float(col_idx))
                break
        if found:
            if self.ball_position is not None:
                self.previous_ball_position = self.ball_position
                self.previous_ball_time = self.last_update
            self.ball_position = found
            if (
                self.previous_ball_position
                and self.previous_ball_time is not None
                and now > self.previous_ball_time
            ):
                dt = now - self.previous_ball_time
                vx = (found[1] - self.previous_ball_position[1]) / dt
                vy = (found[0] - self.previous_ball_position[0]) / dt
                self.ball_velocity = (vx, vy)
                self.last_velocity = self.ball_velocity
            self.ball_exit = None
        else:
            if self.ball_position is not None:
                self.ball_exit = "miss"
                self.ball_exit_time = now
                if self.ball_velocity is not None:
                    self.last_velocity = self.ball_velocity
            self.ball_position = None
            self.ball_velocity = None

    @property
    def height(self) -> int:
        return len(self.lines)

    @property
    def width(self) -> int:
        return max((len(line) for line in self.lines), default=0)


class AgentApp:
    def __init__(
        self,
        stdscr: "curses._CursesWindow",
        session_roles: Dict[str, str],
        diff_queue: "queue.Queue[Tuple[str, object]]",
        mcp_client: MCPClient,
        serve_interval: float,
        serve_dx: Tuple[float, float],
        serve_dy: Tuple[float, float],
        max_step: float,
        min_threshold: float,
        command_interval: float,
    ) -> None:
        self.stdscr = stdscr
        self.session_roles = session_roles
        self.incoming = diff_queue
        self.mcp = mcp_client
        self.serve_interval = serve_interval
        self.serve_dx = serve_dx
        self.serve_dy = serve_dy
        self.max_step = max_step
        self.min_threshold = min_threshold
        self.command_interval = command_interval

        self.sessions: Dict[str, SessionState] = {}
        self.logs: List[str] = []
        self.actions: List[Dict[str, object]] = []
        self.input_buffer: str = ""
        self.autopilot_enabled = True
        self.running = True
        self.last_spawn_time = 0.0
        self.last_draw_time = 0.0
        self.score = {"lhs": 0, "rhs": 0}

    # ------------------------------------------------------------------ Logging
    def log(self, message: str, level: str = "info") -> None:
        timestamp = time.strftime("%H:%M:%S", time.localtime())
        entry = f"[{timestamp}] {level.upper():<6} {message}"
        self.logs.append(entry)
        if len(self.logs) > LOG_LIMIT:
            self.logs = self.logs[-LOG_LIMIT:]

    def log_event(self, event: Dict[str, object]) -> None:
        level = str(event.get("level", "info"))
        if level == "action":
            self.actions.append(event)
            if len(self.actions) > LOG_LIMIT:
                self.actions = self.actions[-LOG_LIMIT:]
            session_id = event.get("session_id", "?")
            command = event.get("command", "")
            transport = event.get("transport", "log")
            status = event.get("status", "queued")
            self.log(
                f"[{status}] {session_id} via {transport}: {command}",
                level="action",
            )
        else:
            message = event.get("message", "")
            self.log(str(message), level=level)

    # ---------------------------------------------------------- Session helpers
    def ensure_session(self, session_id: str) -> SessionState:
        session = self.sessions.get(session_id)
        if session is None:
            session = SessionState(session_id=session_id)
            session.side = self.session_roles.get(session_id)
            if not session.side and not self.session_roles:
                # Assign sides deterministically for first two sessions.
                order = ["lhs", "rhs"]
                session.side = order[len(self.sessions) % len(order)]
            self.sessions[session_id] = session
            self.log(
                f"registered session {session_id} (side={session.side or 'unknown'})",
                level="info",
            )
        return session

    def resolve_session(self, identifier: str) -> Optional[SessionState]:
        for session in self.sessions.values():
            if session.session_id == identifier or session.side == identifier:
                return session
        return None

    def find_session_by_side(self, side: str) -> Optional[SessionState]:
        for session in self.sessions.values():
            if session.side == side:
                return session
        return None

    # ------------------------------------------------------------ Main control
    def run(self) -> None:
        curses.curs_set(0)
        self.stdscr.nodelay(True)
        while self.running:
            now = time.monotonic()
            self._drain_incoming(now)
            if self.autopilot_enabled:
                self._autopilot_tick(now)
            self._draw(now)
            self._handle_keys()
            time.sleep(0.03)
        self.mcp.close()

    # ---------------------------------------------------------- Event handling
    def _drain_incoming(self, now: float) -> None:
        while True:
            try:
                kind, payload = self.incoming.get_nowait()
            except queue.Empty:
                break
            if kind == "diff":
                self._handle_diff(payload, now)
            elif kind == "info":
                self.log(str(payload), level="info")
            elif kind == "error":
                self.log(str(payload), level="error")
            elif kind == "mcp":
                if isinstance(payload, str):
                    try:
                        event = json.loads(payload)
                    except json.JSONDecodeError:
                        self.log(f"invalid MCP event: {payload}", level="error")
                    else:
                        self.log_event(event)
                elif isinstance(payload, dict):
                    self.log_event(payload)

    def _handle_diff(self, source: object, now: float) -> None:
        if isinstance(source, str):
            try:
                event = json.loads(source)
            except json.JSONDecodeError as exc:
                self.log(f"failed to decode diff: {exc}: {source}", level="error")
                return
        elif isinstance(source, dict):
            event = source
        else:
            return
        session_id = event.get("session_id")
        payload = event.get("payload", {})
        if not isinstance(session_id, str) or not isinstance(payload, dict):
            self.log(f"invalid diff structure: {event}", level="warn")
            return
        payload_type = payload.get("type")
        if payload_type != "terminal_full":
            self.log(f"ignoring payload type {payload_type}", level="debug")
            return
        lines = payload.get("lines", [])
        cursor_raw = payload.get("cursor")
        cursor = None
        if isinstance(cursor_raw, dict):
            row = cursor_raw.get("row")
            col = cursor_raw.get("col")
            if isinstance(row, int) and isinstance(col, int):
                cursor = (row, col)
        sequence = event.get("sequence", 0)
        session = self.ensure_session(session_id)
        if session.side is None and session_id in self.session_roles:
            session.side = self.session_roles[session_id]
        session.apply_terminal_frame(
            [str(line) for line in lines if isinstance(line, str)], cursor, int(sequence), now
        )

    # ------------------------------------------------------------- Autopilot
    def _autopilot_tick(self, now: float) -> None:
        for session in list(self.sessions.values()):
            if session.ball_exit:
                self._handle_ball_exit(session, now)
                session.ball_exit = None
        self._maybe_spawn_ball(now)
        for session in self.sessions.values():
            self._drive_paddle(session, now)

    def _maybe_spawn_ball(self, now: float, force_session: Optional[SessionState] = None) -> None:
        if force_session:
            target = force_session
        else:
            if now - self.last_spawn_time < self.serve_interval:
                return
            if any(session.ball_position for session in self.sessions.values()):
                return
            candidates = [
                session
                for session in self.sessions.values()
                if session.height > 0 and session.side in {"lhs", "rhs"}
            ]
            if not candidates:
                return
            target = random.choice(candidates)
        spawn_y = self._random_spawn_row(target)
        dx_mag = random.uniform(*self.serve_dx)
        dy_mag = random.uniform(*self.serve_dy)
        command = f"b {spawn_y:.1f} {dx_mag:.1f} {dy_mag:.1f}"
        self._send_command(target, command)
        self.last_spawn_time = now

    def _random_spawn_row(self, session: SessionState) -> float:
        height = max(session.height, 12)
        margin = 3
        lower = margin
        upper = max(margin + 1, height - margin)
        return random.uniform(lower, upper)

    def _drive_paddle(self, session: SessionState, now: float) -> None:
        if session.paddle_center is None:
            return
        if session.side not in {"lhs", "rhs"}:
            return
        if now - session.last_move_time < self.command_interval:
            return
        target_row = self._target_row(session)
        delta = session.paddle_center - target_row
        if abs(delta) < self.min_threshold:
            return
        clamped = max(-self.max_step, min(self.max_step, delta))
        command = f"m {clamped:.2f}"
        self._send_command(session, command)
        session.last_move_time = now

    def _handle_ball_exit(self, session: SessionState, now: float) -> None:
        if session.side not in {"lhs", "rhs"}:
            return
        target_side = "rhs" if session.side == "lhs" else "lhs"
        target = self.find_session_by_side(target_side)
        if not target:
            for session_id, side in self.session_roles.items():
                if side == target_side:
                    target = self.ensure_session(session_id)
                    break
        if not target:
            self.log(
                f"ball exit from {session.side}, but target {target_side} session unavailable",
                level="warn",
            )
            return

        if session.previous_ball_position:
            spawn_y = session.previous_ball_position[0]
        elif target.paddle_center is not None:
            spawn_y = target.paddle_center
        else:
            spawn_y = max(target.height / 2.0, 3.0)

        spawn_y = max(3.0, min(spawn_y, max(target.height - 3, 3)))

        dx_mag = random.uniform(*self.serve_dx)
        dx = dx_mag if target_side == "rhs" else -dx_mag
        dy = random.uniform(*self.serve_dy)
        if session.last_velocity:
            vy = session.last_velocity[1]
            if vy < 0:
                dy = -abs(dy)
            elif vy > 0:
                dy = abs(dy)

        command = f"b {spawn_y:.1f} {dx:.1f} {dy:.1f}"
        self._send_command(target, command)
        self.last_spawn_time = now
        self.score[target_side] = self.score.get(target_side, 0) + 1
        self.log(
            f"score update - LHS {self.score.get('lhs', 0)} | RHS {self.score.get('rhs', 0)}",
            level="info",
        )

    def _target_row(self, session: SessionState) -> float:
        if session.ball_position is None:
            return max(session.height / 2.0, 1.0)
        row, _ = session.ball_position
        if session.ball_velocity:
            _, vy = session.ball_velocity
            lead = vy * 0.25
            return row + lead
        return row

    def _send_command(self, session: SessionState, command: str) -> None:
        payload = f"{command}\n"
        self.mcp.queue_terminal_write(session.session_id, payload)

    # -------------------------------------------------------------- Commands
    def _handle_keys(self) -> None:
        while True:
            ch = self.stdscr.getch()
            if ch == -1:
                break
            if ch in (3, 4):  # Ctrl+C / Ctrl+D
                self.running = False
                break
            if ch in (curses.KEY_ENTER, 10, 13):
                cmd = self.input_buffer.strip()
                self.input_buffer = ""
                self._execute_command(cmd)
                continue
            if ch in (curses.KEY_BACKSPACE, 127, 8):
                self.input_buffer = self.input_buffer[:-1]
                continue
            if 32 <= ch <= 126:
                self.input_buffer += chr(ch)

    def _execute_command(self, command: str) -> None:
        if not command:
            return
        tokens = command.split()
        verb = tokens[0].lower()
        self.log(f"> {command}", level="prompt")
        if verb == "quit":
            self.running = False
            return
        if verb == "pause":
            self.autopilot_enabled = False
            self.log("autopilot paused", level="info")
            return
        if verb == "resume":
            self.autopilot_enabled = True
            self.log("autopilot resumed", level="info")
            return
        if verb == "serve":
            target = self.resolve_session(tokens[1]) if len(tokens) > 1 else None
            if target is None and len(tokens) > 1:
                self.log(f"unknown session '{tokens[1]}'", level="warn")
                return
            self._maybe_spawn_ball(time.monotonic(), force_session=target)
            return
        if verb == "token":
            if len(tokens) == 1:
                self.log(
                    "usage: token <session|side> <value> | token default <value>",
                    level="warn",
                )
                return
            target_key = tokens[1]
            if target_key in {"default", "*"}:
                if len(tokens) >= 3:
                    self.mcp.set_default_controller_token(tokens[2])
                    self.log("default controller token updated", level="info")
                else:
                    self.mcp.set_default_controller_token(None)
                    self.log("default controller token cleared", level="info")
                return
            if len(tokens) < 3:
                self.log(
                    "usage: token <session|side> <value> | token default <value>",
                    level="warn",
                )
                return
            session = self.resolve_session(target_key)
            token_value = tokens[2]
            if session is None:
                if target_key in {"lhs", "rhs"}:
                    self.log(
                        f"no session currently registered for side '{target_key}'",
                        level="warn",
                    )
                    return
                self.mcp.set_session_token(target_key, token_value)
                self.log(
                    f"stored token for pending session '{target_key}'",
                    level="info",
                )
            else:
                self.mcp.set_session_token(session.session_id, token_value)
                self.log(
                    f"stored token for session '{session.session_id}'",
                    level="info",
                )
            return
        if verb == "m" and len(tokens) >= 3:
            session = self.resolve_session(tokens[1])
            if not session:
                self.log(f"unknown session '{tokens[1]}'", level="warn")
                return
            try:
                delta = float(tokens[2])
            except ValueError:
                self.log("delta must be numeric", level="warn")
                return
            command = f"m {delta:.2f}"
            self._send_command(session, command)
            return
        if verb == "actions":
            self.log(f"{len(self.actions)} actions captured", level="info")
            return
        self.log(f"unrecognised command '{command}'", level="warn")

    # ------------------------------------------------------------------- Draw
    def _draw(self, now: float) -> None:
        height, width = self.stdscr.getmaxyx()
        self.stdscr.erase()
        title = f" Private Beach Pong Agent — autopilot {'ON' if self.autopilot_enabled else 'OFF'} "
        self.stdscr.addstr(0, 0, title[: max(width - 1, 0)])
        score_text = f"LHS {self.score.get('lhs', 0)} | RHS {self.score.get('rhs', 0)}"
        self._addstr_safe(0, max(width - len(score_text) - 2, 0), width, score_text)
        separator_y = 1
        if width > 0:
            self.stdscr.hline(separator_y, 0, curses.ACS_HLINE, width)
        log_top = separator_y + 1
        prompt_height = PROMPT_BOX_HEIGHT + 1
        usable_height = max(height - log_top - prompt_height, 1)
        status_width = min(40, max(width // 3, 20))
        log_width = max(width - status_width - 1, 20)

        # Draw logs
        start_idx = max(len(self.logs) - usable_height, 0)
        for idx in range(usable_height):
            line_idx = start_idx + idx
            if line_idx >= len(self.logs):
                break
            line = self.logs[line_idx][: log_width - 1]
            self.stdscr.addstr(log_top + idx, 0, line)

        # Draw vertical divider
        divider_x = log_width
        if divider_x < width:
            for y in range(log_top, log_top + usable_height):
                self.stdscr.addch(y, divider_x, curses.ACS_VLINE)

        # Draw status blocks
        status_x = divider_x + 1
        status_sessions = list(self.sessions.values())
        block_height = 6
        for idx, session in enumerate(status_sessions):
            base_y = log_top + idx * block_height
            if base_y >= log_top + usable_height:
                break
            self._draw_session_block(session, base_y, status_x, status_width)

        # Prompt box
        prompt_y = height - PROMPT_BOX_HEIGHT - 1
        if prompt_y > log_top:
            if width > 0:
                self.stdscr.hline(prompt_y, 0, curses.ACS_HLINE, width)
            prompt_label = " Prompt "
            self.stdscr.addstr(prompt_y, 1, prompt_label[: max(width - 2, 0)])
            buffer_display = self.input_buffer[-(width - 4) :] if width > 4 else ""
            self.stdscr.addstr(prompt_y + 1, 2, "> " + buffer_display)

        self.last_draw_time = now
        self.stdscr.refresh()

    def _draw_session_block(
        self, session: SessionState, y: int, x: int, width: int
    ) -> None:
        label = f"{session.side or '?'} :: {session.session_id}"
        self._addstr_safe(y, x, width, label)
        paddle_line = (
            f" paddle={session.paddle_center:.1f}"
            if session.paddle_center is not None
            else " paddle=–"
        )
        ball_line = (
            f" ball={session.ball_position[0]:.1f},{session.ball_position[1]:.1f}"
            if session.ball_position
            else " ball=–"
        )
        self._addstr_safe(y + 1, x, width, paddle_line + ball_line)
        if session.ball_velocity:
            vx, vy = session.ball_velocity
            self._addstr_safe(y + 2, x, width, f" velocity=({vx:.2f},{vy:.2f})")
        else:
            self._addstr_safe(y + 2, x, width, " velocity=–")
        if session.last_update:
            age = time.monotonic() - session.last_update
        else:
            age = 0.0
        self._addstr_safe(y + 3, x, width, f" last update {age:.2f}s ago")
        token_marker = (
            "token=✓"
            if self.mcp.current_session_token(session.session_id)
            else "token=–"
        )
        self._addstr_safe(y + 4, x, width, f" {token_marker}")

    def _addstr_safe(self, y: int, x: int, width: int, text: str) -> None:
        if width <= 0 or y < 0:
            return
        truncated = text[: max(width - 1, 0)]
        if truncated:
            self.stdscr.addstr(y, x, truncated)


def parse_args(argv: Optional[List[str]] = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Private Beach Pong agent TUI")
    parser.add_argument(
        "--session",
        action="append",
        default=[],
        help="Bind a session to a side (e.g. lhs:sess-1 or sess-1=lhs). May repeat.",
    )
    parser.add_argument(
        "--private-beach-id",
        help="Private Beach identifier used for discovery and pairing.",
    )
    parser.add_argument(
        "--session-tag",
        help="Metadata tag identifying this agent session (defaults to $PONG_SESSION_TAG).",
    )
    parser.add_argument(
        "--mcp-base-url",
        help="Beach Manager base URL for queue_action calls (e.g. https://manager.example/api/).",
    )
    parser.add_argument(
        "--mcp-token",
        help="Bearer token with pb:control.write scope. Leave blank to skip authorization.",
    )
    parser.add_argument(
        "--default-controller-token",
        help="Fallback controller token applied to all sessions unless overridden.",
    )
    parser.add_argument(
        "--session-token",
        action="append",
        default=[],
        help="Override controller token per session (format session_id=token). May repeat.",
    )
    parser.add_argument(
        "--auto-pair",
        dest="auto_pair",
        action="store_true",
        help="Enable automatic discovery/pairing (default).",
    )
    parser.add_argument(
        "--no-auto-pair",
        dest="auto_pair",
        action="store_false",
        help="Disable automatic discovery/pairing.",
    )
    parser.set_defaults(auto_pair=True)
    parser.add_argument(
        "--pair-template",
        help="Prompt template to associate with controller pairings.",
    )
    parser.add_argument(
        "--pair-cadence",
        choices=["fast", "balanced", "slow"],
        default="balanced",
        help="Update cadence for controller pairings (default: balanced).",
    )
    parser.add_argument(
        "--lease-ttl",
        type=int,
        default=30_000,
        help="Controller lease TTL in milliseconds (default: 30000).",
    )
    parser.add_argument(
        "--lease-reason",
        default="pong_autopilot",
        help="Reason string logged with controller lease requests.",
    )
    parser.add_argument(
        "--discovery-attempts",
        type=int,
        default=12,
        help="Number of discovery polls when auto pairing (default: 12).",
    )
    parser.add_argument(
        "--discovery-interval",
        type=float,
        default=1.0,
        help="Seconds between discovery polls (default: 1.0).",
    )
    parser.add_argument(
        "--actions-target",
        type=parse_host_port,
        help="Optional host:port to forward MCP actions as JSON lines.",
    )
    parser.add_argument(
        "--action-log",
        type=Path,
        help="Optional path to append JSON action log.",
    )
    parser.add_argument(
        "--serve-interval",
        type=float,
        default=DEFAULT_SERVE_INTERVAL,
        help="Seconds between automatic ball spawns.",
    )
    parser.add_argument(
        "--serve-dx",
        type=float,
        nargs=2,
        metavar=("MIN", "MAX"),
        default=(18.0, 26.0),
        help="Range for horizontal velocity magnitude when serving.",
    )
    parser.add_argument(
        "--serve-dy",
        type=float,
        nargs=2,
        metavar=("MIN", "MAX"),
        default=(-8.0, 8.0),
        help="Range for vertical velocity component when serving.",
    )
    parser.add_argument(
        "--max-step",
        type=float,
        default=DEFAULT_MAX_STEP,
        help="Maximum paddle move per command.",
    )
    parser.add_argument(
        "--min-threshold",
        type=float,
        default=DEFAULT_MIN_THRESHOLD,
        help="Ignore adjustments smaller than this delta.",
    )
    parser.add_argument(
        "--command-interval",
        type=float,
        default=DEFAULT_COMMAND_INTERVAL,
        help="Minimum seconds between paddle commands per session.",
    )
    return parser.parse_args(argv)


def main() -> int:
    args = parse_args()
    session_tokens: Dict[str, str] = {}

    def prompt_if_tty(message: str, secret: bool = False) -> str:
        if not sys.stdin or not sys.stdin.isatty():
            return ""
        try:
            return getpass(message) if secret else input(message)
        except EOFError:
            return ""

    if args.mcp_base_url is None:
        args.mcp_base_url = os.environ.get("PB_MCP_BASE_URL")
    if args.mcp_base_url is None:
        entered = prompt_if_tty(
            "Enter Beach Manager base URL for queue_action (blank to skip): "
        )
        args.mcp_base_url = entered.strip() or None

    if args.mcp_base_url:
        if args.mcp_token is None:
            args.mcp_token = (
                os.environ.get("PB_MCP_TOKEN")
                or os.environ.get("PB_MANAGER_TOKEN")
                or os.environ.get("PB_BUGGY_TOKEN")
            )
        if args.mcp_token is None:
            entered = prompt_if_tty("Bearer token (blank if none): ", secret=True)
            args.mcp_token = entered.strip() or None

        if args.default_controller_token is None:
            args.default_controller_token = os.environ.get("PB_CONTROLLER_TOKEN")
        if args.default_controller_token is None:
            entered = prompt_if_tty(
                "Controller token (optional, blank to skip): ",
                secret=True,
            )
            args.default_controller_token = entered.strip() or None

    try:
        session_tokens = parse_token_mapping(args.session_token)
    except argparse.ArgumentTypeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    session_role_map = parse_session_mapping(args.session)
    diff_queue: "queue.Queue[Tuple[str, object]]" = queue.Queue()
    stop_event = threading.Event()

    manager_client: Optional[PrivateBeachManagerClient] = None
    if args.mcp_base_url:
        manager_client = PrivateBeachManagerClient(args.mcp_base_url, args.mcp_token)
    elif args.auto_pair:
        diff_queue.put(
            (
                "warn",
                "auto pairing requires --mcp-base-url; disabling auto pair",
            )
        )
        args.auto_pair = False

    autopair_ctx: Optional[AutopairContext] = None
    if args.auto_pair and manager_client and args.private_beach_id:
        autopair_ctx = autopair_sessions(args, manager_client, diff_queue)
        if autopair_ctx:
            session_tokens.setdefault(
                autopair_ctx.controller_session_id, autopair_ctx.controller_token
            )
            for child_session in autopair_ctx.child_sessions.values():
                session_tokens.setdefault(child_session, autopair_ctx.controller_token)
            session_role_map.update(autopair_ctx.session_roles)

    diff_queue.put(("info", f"session roles: {session_role_map}"))

    def log_action_event(event: Dict[str, object]) -> None:
        diff_queue.put(("mcp", json.dumps(event)))

    default_token = (
        autopair_ctx.controller_token
        if autopair_ctx and autopair_ctx.controller_token
        else args.default_controller_token
    )

    mcp_client = MCPClient(
        action_callback=log_action_event,
        base_url=args.mcp_base_url,
        auth_token=args.mcp_token,
        default_controller_token=default_token,
        session_tokens=session_tokens,
        target=args.actions_target,
        log_path=args.action_log,
    )

    subscribers: List[StateSubscriber] = []
    if manager_client:
        subscribe_roles = {
            session_id
            for session_id, role in session_role_map.items()
            if role in {"lhs", "rhs"}
        }
        if autopair_ctx:
            subscribe_roles.update(autopair_ctx.child_sessions.values())
        for session_id in sorted(subscribe_roles):
            subscriber = StateSubscriber(
                manager_client,
                session_id,
                diff_queue,
                stop_event,
                role=session_role_map.get(session_id),
            )
            subscriber.start()
            subscribers.append(subscriber)
    else:
        diff_queue.put(("warn", "manager client unavailable; skipping state streams"))

    renewer: Optional[LeaseRenewer] = None
    if autopair_ctx and manager_client:

        def _update_token(lease: ControllerLease) -> None:
            mcp_client.set_default_controller_token(lease.controller_token)

        renewer = LeaseRenewer(
            manager_client,
            autopair_ctx.controller_session_id,
            args.lease_ttl,
            diff_queue,
            stop_event,
            _update_token,
            reason=args.lease_reason,
        )
        renewer.start()

    def run_app(stdscr: "curses._CursesWindow") -> None:
        app = AgentApp(
            stdscr=stdscr,
            session_roles=session_role_map,
            diff_queue=diff_queue,
            mcp_client=mcp_client,
            serve_interval=args.serve_interval,
            serve_dx=tuple(args.serve_dx),
            serve_dy=tuple(args.serve_dy),
            max_step=args.max_step,
            min_threshold=args.min_threshold,
            command_interval=args.command_interval,
        )
        try:
            app.log("agent ready", level="info")
            app.run()
        finally:
            pass

    try:
        curses.wrapper(run_app)
    finally:
        stop_event.set()
        for subscriber in subscribers:
            subscriber.join(timeout=1.0)
        if renewer:
            renewer.join(timeout=1.0)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
