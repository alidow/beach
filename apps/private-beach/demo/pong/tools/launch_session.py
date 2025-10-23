#!/usr/bin/env python3
"""
Utility to launch a local process (e.g. Pong player or agent) inside the Beach
terminal harness so it streams diffs and accepts MCP commands via the Beach
Manager. The script:

 1. Spawns `beach host` with `--bootstrap-output json` to capture the new
    session id.
 2. Attaches the session to the specified Private Beach.
 3. Updates session metadata so other components (agent, UI) can discover role.
 4. Bridges stdin/stdout between the current terminal and the harness process so
    the hosted TUI remains interactive.
"""

from __future__ import annotations

import argparse
import json
import os
import pty
import random
import select
import string
import subprocess
import sys
import time
from typing import Dict, Iterable, List, Optional, Tuple

try:  # pragma: no cover - import shim for script/module usage
    from .manager_client import (
        ManagerRequestError,
        PrivateBeachManagerClient,
    )
except ImportError:  # pragma: no cover - direct execution fallback
    from manager_client import (
        ManagerRequestError,
        PrivateBeachManagerClient,
    )


def _print(msg: str) -> None:
    sys.stderr.write(f"{msg}\n")
    sys.stderr.flush()


def parse_metadata(values: Iterable[str]) -> Dict[str, str]:
    metadata: Dict[str, str] = {}
    for raw in values:
        if "=" not in raw:
            raise argparse.ArgumentTypeError("metadata must be supplied as key=value")
        key, value = raw.split("=", 1)
        key = key.strip()
        value = value.strip()
        if not key:
            raise argparse.ArgumentTypeError("metadata keys cannot be empty")
        metadata[key] = value
    return metadata


def generate_tag(prefix: str = "pong") -> str:
    token = "".join(random.choices(string.ascii_lowercase + string.digits, k=6))
    return f"{prefix}-{token}"


def read_handshake(master_fd: int, timeout: float = 10.0) -> Tuple[dict, bytes]:
    deadline = time.monotonic() + timeout
    buffer = b""
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("timed out waiting for bootstrap handshake")
        rlist, _, _ = select.select([master_fd], [], [], remaining)
        if not rlist:
            continue
        chunk = os.read(master_fd, 4096)
        if not chunk:
            raise RuntimeError("beach host exited before emitting handshake")
        buffer += chunk
        if b"\n" in buffer:
            line, rest = buffer.split(b"\n", 1)
            if not line.strip():
                continue
            try:
                handshake = json.loads(line.decode("utf-8"))
            except json.JSONDecodeError as exc:
                raise RuntimeError(f"invalid handshake payload: {line!r}") from exc
            return handshake, rest


def forward_stdio(master_fd: int, initial: bytes) -> int:
    if initial:
        os.write(sys.stdout.fileno(), initial)
    fds = [master_fd, sys.stdin.fileno()]
    exit_code = 0
    try:
        while True:
            rlist, _, _ = select.select(fds, [], [])
            if master_fd in rlist:
                try:
                    data = os.read(master_fd, 4096)
                except OSError:
                    data = b""
                if not data:
                    break
                os.write(sys.stdout.fileno(), data)
            if sys.stdin.fileno() in rlist:
                try:
                    data = os.read(sys.stdin.fileno(), 4096)
                except OSError:
                    data = b""
                if not data:
                    os.close(master_fd)
                    break
                os.write(master_fd, data)
    except KeyboardInterrupt:
        try:
            os.close(master_fd)
        except OSError:
            pass
        exit_code = 130
    return exit_code


def build_host_command(args: argparse.Namespace, metadata_tag: str) -> List[str]:
    beach_binary = args.beach_binary
    if beach_binary:
        base = [beach_binary]
    else:
        base = ["cargo", "run", "-p", "beach", "--"]

    command = base + [
        "--session-server",
        args.manager_url,
        "host",
        "--bootstrap-output",
        "json",
        "--mcp",
        "--mcp-allow-write",
    ]

    if args.wait:
        command.append("--wait")
    if args.extra_host_arg:
        command.extend(args.extra_host_arg)
    command.append("--command")
    command.extend(args.command)

    env = os.environ.copy()
    env.setdefault("BEACH_LOG_LEVEL", args.beach_log_level)
    env.setdefault("PONG_SESSION_TAG", metadata_tag)
    return command, env


def parse_args(argv: Optional[List[str]] = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Launch a Beach harness session for Pong components."
    )
    parser.add_argument(
        "--manager-url",
        required=True,
        help="Beach Manager base URL (e.g. https://manager.private-beach.test/api).",
    )
    parser.add_argument(
        "--private-beach-id",
        required=True,
        help="Private Beach identifier to attach the session to.",
    )
    parser.add_argument(
        "--auth-token",
        help="Bearer token with pb:sessions.write scope. Defaults to $PB_MANAGER_TOKEN.",
    )
    parser.add_argument(
        "--role",
        choices=["lhs", "rhs", "agent", "scoreboard", "custom"],
        help="Pong role metadata label.",
    )
    parser.add_argument(
        "--tag",
        help="Custom metadata tag for discovery. Defaults to a random value.",
    )
    parser.add_argument(
        "--metadata",
        action="append",
        default=[],
        help="Additional metadata key=value pairs (repeatable).",
    )
    parser.add_argument(
        "--location-hint",
        help="Optional location hint to store with the session record.",
    )
    parser.add_argument(
        "--beach-binary",
        help="Path to an existing beach binary. If omitted, runs via `cargo run -p beach`.",
    )
    parser.add_argument(
        "--beach-log-level",
        default="warn",
        help="Log level passed to the beach harness (default: warn).",
    )
    parser.add_argument(
        "--wait",
        action="store_true",
        help="Pass --wait to beach host so the command starts after the first client joins.",
    )
    parser.add_argument(
        "--extra-host-arg",
        action="append",
        default=[],
        help="Additional argument to forward to `beach host` (repeatable).",
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="Command to execute inside the harness (e.g. python3 player/main.py --mode lhs).",
    )
    parsed = parser.parse_args(argv)
    if not parsed.command:
        parser.error("command to execute is required (use -- to separate arguments)")
    if parsed.command[0] == "--":
        parsed.command = parsed.command[1:]
    if not parsed.command:
        parser.error("command to execute is required")
    if parsed.auth_token is None:
        parsed.auth_token = os.environ.get("PB_MANAGER_TOKEN") or os.environ.get(
            "PB_MCP_TOKEN"
        )
    return parsed


def main(argv: Optional[List[str]] = None) -> int:
    args = parse_args(argv)
    metadata = parse_metadata(args.metadata)
    tag = args.tag or generate_tag()
    metadata.setdefault("pong_tag", tag)
    if args.role and args.role != "custom":
        metadata.setdefault("pong_role", args.role)
    client = PrivateBeachManagerClient(args.manager_url, args.auth_token)

    cmd, env = build_host_command(args, metadata_tag=tag)

    master_fd, slave_fd = pty.openpty()
    try:
        proc = subprocess.Popen(
            cmd,
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
            env=env,
            close_fds=True,
        )
    except FileNotFoundError as exc:
        _print(f"failed to spawn beach harness: {exc}")
        return 1
    finally:
        os.close(slave_fd)

    try:
        handshake, remaining = read_handshake(master_fd)
    except Exception as exc:  # pragma: no cover - initialization failure
        _print(f"error while reading bootstrap handshake: {exc}")
        try:
            proc.terminate()
        except Exception:
            pass
        return 1

    session_id = handshake.get("session_id")
    if not session_id:
        _print(f"bootstrap response missing session_id: {handshake}")
        try:
            proc.terminate()
        except Exception:
            pass
        return 1

    try:
        client.attach_session(args.private_beach_id, session_id)
    except ManagerRequestError as exc:
        _print(f"warning: failed to attach session {session_id}: {exc}")

    try:
        client.update_session_metadata(session_id, metadata, args.location_hint)
    except ManagerRequestError as exc:
        _print(f"warning: failed to update metadata for {session_id}: {exc}")

    _print(
        f"Beach session ready: id={session_id} tag={tag} role={metadata.get('pong_role','n/a')}"
    )

    exit_code = forward_stdio(master_fd, remaining)
    proc.wait()
    return exit_code or proc.returncode


if __name__ == "__main__":  # pragma: no cover - CLI entrypoint
    raise SystemExit(main())
