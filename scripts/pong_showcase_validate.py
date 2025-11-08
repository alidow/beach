#!/usr/bin/env python3
"""
Automated Private Beach Pong showcase validation.

This script launches two mock player sessions and the automation agent inside
Beach harnesses, wires controller pairings with trace headers, and verifies
that controller SSE + state SSE streams emit events while the agent logs
trace-aware actions. It is intended for local smoke testing and CI sanity
checks.
"""

from __future__ import annotations

import argparse
import json
import os
import pty
import select
import signal
import subprocess
import sys
import threading
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Sequence, Tuple

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_DIR = REPO_ROOT / "apps/private-beach/demo/pong/tools"
PLAYER_MAIN = REPO_ROOT / "apps/private-beach/demo/pong/player/main.py"
AGENT_MAIN = REPO_ROOT / "apps/private-beach/demo/pong/agent/main.py"

sys.path.insert(0, str(TOOLS_DIR))
from manager_client import (  # type: ignore  # noqa: E402
    ManagerRequestError,
    PrivateBeachManagerClient,
)


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
            handshake = json.loads(line.decode("utf-8"))
            return handshake, rest


def build_host_command(
    manager_url: str,
    session_server_url: Optional[str],
    beach_binary: Optional[str],
    extra_args: Optional[Sequence[str]],
    command: Sequence[str],
) -> List[str]:
    if beach_binary:
        base = [beach_binary]
    else:
        base = ["cargo", "run", "-p", "beach", "--"]
    session_server = session_server_url or manager_url
    cmd = list(base) + [
        "--session-server",
        session_server,
        "host",
        "--bootstrap-output",
        "json",
        "--mcp",
        "--mcp-allow-write",
    ]
    if extra_args:
        cmd.extend(extra_args)
    cmd.append("--")
    cmd.extend(command)
    return cmd


@dataclass
class HarnessSession:
    role: str
    session_id: str
    process: subprocess.Popen
    master_fd: int
    log_path: Path
    output_lines: List[str]
    tag: str
    reader_thread: threading.Thread
    stop_event: threading.Event

    def stop(self) -> None:
        self.stop_event.set()
        if self.process.poll() is None:
            try:
                self.process.terminate()
            except OSError:
                pass
        try:
            os.close(self.master_fd)
        except OSError:
            pass
        self.reader_thread.join(timeout=1.0)
        try:
            self.process.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            try:
                self.process.kill()
            except OSError:
                pass


def spawn_harnessed_session(
    *,
    role: str,
    command: Sequence[str],
    manager_url: str,
    private_beach_id: str,
    client: PrivateBeachManagerClient,
    metadata_tag: str,
    beach_binary: Optional[str],
    session_server_url: Optional[str],
    beach_log_level: str,
) -> HarnessSession:
    env = os.environ.copy()
    env.setdefault("BEACH_LOG_LEVEL", beach_log_level)
    env["PONG_SESSION_TAG"] = metadata_tag
    master_fd, slave_fd = pty.openpty()
    log_path = REPO_ROOT / "temp" / f"pong-{role}-{metadata_tag}.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    cmd = build_host_command(manager_url, session_server_url, beach_binary, None, command)
    try:
        proc = subprocess.Popen(
            cmd,
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
            close_fds=True,
            env=env,
        )
    finally:
        os.close(slave_fd)
    handshake, remaining = read_handshake(master_fd)
    session_id = handshake.get("session_id")
    if not isinstance(session_id, str):
        proc.terminate()
        raise RuntimeError(f"invalid bootstrap handshake: {handshake}")
    client.attach_session(private_beach_id, session_id)
    client.update_session_metadata(
        session_id,
        {"pong_role": role, "pong_tag": metadata_tag},
    )
    output_lines: List[str] = []
    stop_flag = threading.Event()

    def reader() -> None:
        buffer = remaining
        with log_path.open("ab", buffering=0) as fp:
            while not stop_flag.is_set():
                if buffer:
                    chunk = buffer
                    buffer = b""
                else:
                    try:
                        chunk = os.read(master_fd, 4096)
                    except OSError:
                        break
                if not chunk:
                    break
                fp.write(chunk)
                try:
                    text = chunk.decode("utf-8", errors="ignore")
                except Exception:
                    continue
                for line in text.splitlines():
                    if not line.strip():
                        continue
                    output_lines.append(line.strip())
                    if len(output_lines) > 500:
                        del output_lines[: len(output_lines) - 500]

    reader_thread = threading.Thread(target=reader, daemon=True)
    reader_thread.start()
    return HarnessSession(
        role=role,
        session_id=session_id,
        process=proc,
        master_fd=master_fd,
        log_path=log_path,
        output_lines=output_lines,
        tag=metadata_tag,
        reader_thread=reader_thread,
        stop_event=stop_flag,
    )


class SSECollector(threading.Thread):
    def __init__(
        self,
        label: str,
        generator_fn,
        expected_events: int,
    ) -> None:
        super().__init__(daemon=True)
        self.label = label
        self.generator_fn = generator_fn
        self.expected = expected_events
        self.events: List[Dict[str, object]] = []
        self.error: Optional[str] = None
        self.ready = threading.Event()

    def run(self) -> None:
        try:
            for payload in self.generator_fn():
                if not isinstance(payload, dict):
                    continue
                self.events.append(payload)
                if len(self.events) >= self.expected:
                    self.ready.set()
                    break
        except ManagerRequestError as exc:
            self.error = str(exc)
            self.ready.set()


def wait_for(condition, timeout: float, interval: float = 0.2) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if condition():
            return True
        time.sleep(interval)
    return False


def build_argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Validate Private Beach Pong showcase wiring.")
    parser.add_argument("--manager-url", required=True, help="Beach Manager base URL.")
    parser.add_argument("--private-beach-id", required=True, help="Private Beach identifier.")
    parser.add_argument(
        "--auth-token",
        help="Bearer token with pb:sessions.write and pb:control.write scopes (defaults to $PB_MANAGER_TOKEN).",
    )
    parser.add_argument(
        "--beach-binary",
        help="Path to an existing beach binary (default: cargo run -p beach -- ...).",
    )
    parser.add_argument(
        "--session-server-url",
        help="Override session server URL (defaults to manager URL).",
    )
    parser.add_argument(
        "--beach-log-level",
        default="warn",
        help="BEACH_LOG_LEVEL for launched harnesses (default: warn).",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=20.0,
        help="Seconds to wait for SSE events/logs before failing (default: 20).",
    )
    return parser


def main() -> int:
    parser = build_argument_parser()
    args = parser.parse_args()
    token = args.auth_token or os.environ.get("PB_MANAGER_TOKEN")
    client = PrivateBeachManagerClient(args.manager_url, token)
    shared_tag = f"pong-validate-{uuid.uuid4().hex[:6]}"
    launched: List[HarnessSession] = []
    trace_id = uuid.uuid4().hex
    try:
        lhs = spawn_harnessed_session(
            role="lhs",
            command=[sys.executable, str(PLAYER_MAIN), "--mode", "lhs"],
            manager_url=args.manager_url,
            private_beach_id=args.private_beach_id,
            client=client,
            metadata_tag=shared_tag,
            beach_binary=args.beach_binary,
            session_server_url=args.session_server_url,
            beach_log_level=args.beach_log_level,
        )
        launched.append(lhs)
        rhs = spawn_harnessed_session(
            role="rhs",
            command=[sys.executable, str(PLAYER_MAIN), "--mode", "rhs"],
            manager_url=args.manager_url,
            private_beach_id=args.private_beach_id,
            client=client,
            metadata_tag=shared_tag,
            beach_binary=args.beach_binary,
            session_server_url=args.session_server_url,
            beach_log_level=args.beach_log_level,
        )
        launched.append(rhs)
        agent_cmd = [
            sys.executable,
            str(AGENT_MAIN),
            "--auto-pair",
            "--private-beach-id",
            args.private_beach_id,
            "--mcp-base-url",
            args.manager_url,
            "--session-tag",
            shared_tag,
            "--headless",
            "--discovery-attempts",
            "6",
            "--discovery-interval",
            "1.0",
        ]
        if token:
            agent_cmd.extend(["--mcp-token", token])
        agent = spawn_harnessed_session(
            role="agent",
            command=agent_cmd,
            manager_url=args.manager_url,
            private_beach_id=args.private_beach_id,
            client=client,
            metadata_tag=shared_tag,
            beach_binary=args.beach_binary,
            session_server_url=args.session_server_url,
            beach_log_level=args.beach_log_level,
        )
        launched.append(agent)

        onboarding = client.onboard_agent(agent.session_id, "pong", ["agent"], {})
        metadata = {
            "pong_role": "agent",
            "pong_tag": shared_tag,
            "agent": {
                "prompt_pack": onboarding.get("prompt_pack"),
                "trace": {"enabled": True, "trace_id": trace_id},
                "mcp_bridges": onboarding.get("mcp_bridges", []),
            },
        }
        client.update_session_metadata(agent.session_id, metadata)
        assignments = []
        for role_name, session in (("lhs", lhs), ("rhs", rhs)):
            prompt_template = (
                f"Role:\nAutomation Agent\n\n"
                f"Responsibility:\nKeep the {role_name.upper()} paddle aligned with the ball.\n\n"
                "Instructions:\nMaintain rally stability and report anomalies."
            )
            assignments.append(
                {
                    "controller_session_id": agent.session_id,
                    "child_session_id": session.session_id,
                    "prompt_template": prompt_template,
                    "update_cadence": "fast",
                }
            )
        results = list(
            client.batch_controller_assignments(
                args.private_beach_id, assignments, trace_id=trace_id
            )
        )
        if not results or any(not item.get("ok") for item in results):
            raise RuntimeError(f"controller assignment failed: {results}")

        pairing_monitor = SSECollector(
            "pairings",
            lambda: client.subscribe_controller_pairings(agent.session_id, trace_id=trace_id),
            expected_events=2,
        )
        pairing_monitor.start()
        state_monitors = []
        for session in (lhs, rhs):
            monitor = SSECollector(
                f"state-{session.role}",
                lambda sid=session.session_id: client.subscribe_state(sid, trace_id=trace_id),
                expected_events=1,
            )
            monitor.start()
            state_monitors.append(monitor)

        pairing_ready = pairing_monitor.ready.wait(timeout=args.timeout)
        states_ready = all(
            monitor.ready.wait(timeout=args.timeout) for monitor in state_monitors
        )
        agent_trace_logged = any(trace_id in line for line in agent.output_lines)

        if not pairing_ready:
            raise RuntimeError(f"pairing SSE stream incomplete: {pairing_monitor.events}")
        if not states_ready:
            raise RuntimeError("state SSE streams did not emit diffs in time")
        if not agent_trace_logged:
            raise RuntimeError("agent logs missing trace identifier")

        print("✓ Pong showcase validation succeeded")
        print(f"  Trace ID: {trace_id}")
        print(f"  Sessions: agent={agent.session_id} lhs={lhs.session_id} rhs={rhs.session_id}")
        return 0
    except Exception as exc:
        print(f"✗ Validation failed: {exc}", file=sys.stderr)
        return 1
    finally:
        for session in launched:
            session.stop()


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    sys.exit(main())
