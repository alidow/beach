#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import pty
import select
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Optional

ROOT = Path(__file__).resolve().parent.parent
REPO_ROOT = ROOT.parent
DEFAULT_HANDSHAKE = ROOT / "results" / "host-handshake.json"
DEFAULT_LOG = ROOT / "results" / "client.log"
DEFAULT_CAPTURE = ROOT / "results" / "client-capture.log"
DEFAULT_SUMMARY = ROOT / "results" / "client-summary.json"
DEFAULT_ECHO_LOG = ROOT / "results" / "echo-server.log"


def load_handshake(path: Path, timeout: float) -> dict:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if path.exists():
            try:
                return json.loads(path.read_text())
            except json.JSONDecodeError:
                time.sleep(0.5)
                continue
        time.sleep(0.5)
    raise RuntimeError(f"handshake file not found or unreadable: {path}")


def extract_response(buffer: str) -> Optional[dict]:
    for line in buffer.splitlines():
        if "ECHO_RESPONSE" not in line:
            continue
        _, _, tail = line.partition("ECHO_RESPONSE")
        try:
            return json.loads(tail.strip())
        except json.JSONDecodeError:
            continue
    return None


def wait_for_output(
    master_fd: int,
    payload: str,
    capture_path: Path,
    timeout: float,
    hold_secs: float,
) -> dict:
    capture_path.parent.mkdir(parents=True, exist_ok=True)
    with capture_path.open("a", encoding="utf-8") as capture:
        buffer = ""
        ready_sent = False
        session_ready = False
        last_send = 0.0
        deadline = time.time() + timeout
        response = None
        hold_deadline = None
        while time.time() < deadline:
            rlist, _, _ = select.select([master_fd], [], [], 0.5)
            if master_fd in rlist:
                try:
                    chunk = os.read(master_fd, 4096).decode(errors="ignore")
                except OSError:
                    break
                if not chunk:
                    break
                capture.write(chunk)
                capture.flush()
                buffer += chunk
                if not session_ready and "Listening for session events" in buffer:
                    session_ready = True
                if ("ECHO_SERVER_READY" in buffer or session_ready) and (
                    not ready_sent or time.time() - last_send > 1.0
                ):
                    os.write(master_fd, (payload + "\n").encode())
                    ready_sent = True
                    last_send = time.time()
                    if hold_secs > 0 and hold_deadline is None:
                        hold_deadline = time.time() + hold_secs
                resp = extract_response(buffer)
                if resp:
                    response = resp
                    if hold_secs == 0:
                        break
                    if hold_deadline is None:
                        hold_deadline = time.time() + hold_secs
            if hold_deadline and time.time() >= hold_deadline:
                break
        return {
            "response": response,
            "buffer": buffer,
            "ready_sent": ready_sent,
        }


def parse_candidate(log_path: Optional[Path]) -> Optional[str]:
    if not log_path or not log_path.exists():
        return None
    for line in log_path.read_text(errors="ignore").splitlines():
        lower = line.lower()
        if "candidate" in lower and ("selected" in lower or "nominated" in lower or "pair" in lower):
            return line.strip()
    return None


def parse_echo_log(echo_log: Path, payload: str) -> Optional[dict]:
    if not echo_log.exists():
        return None
    found = None
    for line in echo_log.read_text(errors="ignore").splitlines():
        if payload not in line:
            continue
        if "responded" not in line:
            continue
        parts = line.split("responded", 1)
        if len(parts) < 2:
            continue
        tail = parts[1].strip()
        try:
            found = json.loads(tail)
        except json.JSONDecodeError:
            continue
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description="Run a beach client echo smoke")
    parser.add_argument("--handshake", type=Path, default=DEFAULT_HANDSHAKE)
    parser.add_argument("--session-server", type=str, default=None)
    parser.add_argument("--label", type=str, default="webrtc-tester")
    parser.add_argument("--payload", type=str, required=True)
    parser.add_argument("--log-file", type=Path, default=DEFAULT_LOG)
    parser.add_argument("--capture", type=Path, default=DEFAULT_CAPTURE)
    parser.add_argument("--summary", type=Path, default=DEFAULT_SUMMARY)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--hold-secs", type=float, default=0, help="keep the client session alive after receiving echo")
    parser.add_argument("--mode", type=str, default="client")
    parser.add_argument("--echo-log", type=Path, default=DEFAULT_ECHO_LOG)
    args = parser.parse_args()

    handshake = load_handshake(args.handshake, args.timeout)
    session_id = handshake.get("session_id")
    join_code = handshake.get("join_code")
    base_url = args.session_server or handshake.get("session_server")
    if not session_id or not join_code or not base_url:
        raise RuntimeError("handshake file missing session_id/join_code/session_server")

    env = os.environ.copy()
    env.setdefault("BEACH_PUBLIC_MODE", "1")
    env.setdefault("BEACH_LOG_FILE", str(args.log_file))
    env.setdefault("BEACH_LOG_LEVEL", "info")
    env.setdefault("BEACH_SESSION_SERVER_BASE", base_url)
    env.setdefault("BEACH_SESSION_SERVER", base_url)
    env.setdefault("BEACH_PUBLIC_SESSION_SERVER", base_url)
    env.setdefault(
        "RUST_LOG",
        env.get("RUST_LOG", "webrtc::peer_connection=info,webrtc::ice_transport=info,beach::transport::webrtc=debug"),
    )

    log_level = env.get("BEACH_LOG_LEVEL", "info")
    cmd = [
        "cargo",
        "run",
        "-p",
        "beach",
        "--",
        "--session-server",
        base_url,
        "--log-level",
        log_level,
        "join",
        session_id,
        "--passcode",
        join_code,
        "--label",
        args.label,
    ]

    master_fd, slave_fd = pty.openpty()
    proc = subprocess.Popen(
        cmd,
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        env=env,
        cwd=str(REPO_ROOT),
    )
    os.close(slave_fd)

    expected_checksum = hashlib.sha256(args.payload.encode()).hexdigest()
    start_ts = time.time()
    report = wait_for_output(
        master_fd,
        args.payload,
        args.capture,
        args.timeout,
        args.hold_secs,
    )
    elapsed = time.time() - start_ts

    try:
        os.write(master_fd, b"\x03")
    except OSError:
        pass

    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    response = report.get("response")
    if response is None:
        response = parse_echo_log(args.echo_log, args.payload)
    status = "pass"
    error = None
    if not report.get("ready_sent"):
        status = "fail"
        error = "ready marker not seen"
    elif not response:
        status = "fail"
        error = "no echo response captured"
    else:
        received_checksum = response.get("sha256")
        if received_checksum != expected_checksum:
            status = "fail"
            error = "checksum mismatch"

    candidate_line = parse_candidate(args.log_file)
    summary = {
        "mode": args.mode,
        "status": status,
        "error": error,
        "session_id": session_id,
        "session_server": base_url,
        "payload": args.payload,
        "expected_checksum": expected_checksum,
        "received": response,
        "hold_secs": args.hold_secs,
        "elapsed_secs": elapsed,
        "command": cmd,
        "env": {
            "BEACH_SESSION_SERVER": env.get("BEACH_SESSION_SERVER"),
            "BEACH_PUBLIC_MODE": env.get("BEACH_PUBLIC_MODE"),
            "BEACH_MANAGER_AUTH_OPTIONAL": env.get("BEACH_MANAGER_AUTH_OPTIONAL"),
            "BEACH_LOG_LEVEL": env.get("BEACH_LOG_LEVEL"),
            "BEACH_TOKEN": env.get("BEACH_TOKEN"),
        },
        "handshake_path": str(args.handshake),
        "capture_log": str(args.capture),
        "structured_log": str(args.log_file),
        "candidate_log": candidate_line,
        "echo_log": str(args.echo_log),
    }
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    args.summary.write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
