#!/usr/bin/env python3
import hashlib
import json
import os
import sys
from datetime import datetime

READY_MARKER = os.environ.get("ECHO_READY_MARKER", "ECHO_SERVER_READY")
LOG_PATH = os.environ.get("ECHO_LOG_PATH")


def log_line(message: str) -> None:
    if not LOG_PATH:
        return
    try:
        with open(LOG_PATH, "a", encoding="utf-8") as fh:
            fh.write(message + "\n")
    except Exception:
        pass


def main() -> None:
    payload_prefix = os.environ.get("ECHO_PREFIX", "")
    log_line("echo_server: starting up")
    sys.stdout.write(f"{READY_MARKER}\n")
    sys.stdout.flush()
    log_line(f"echo_server: sent ready marker ({READY_MARKER})")
    for raw in sys.stdin:
        data = raw.rstrip("\r\n")
        if not data:
            continue
        payload = f"{payload_prefix}{data}" if payload_prefix else data
        checksum = hashlib.sha256(payload.encode()).hexdigest()
        log_line(f"echo_server: received '{data}' ({len(payload)} bytes)")
        envelope = {
            "payload": payload,
            "sha256": checksum,
            "received_at": datetime.utcnow().isoformat() + "Z",
            "length": len(payload),
        }
        sys.stdout.write(f"ECHO_RESPONSE {json.dumps(envelope)}\n")
        sys.stdout.flush()
        log_line(f"echo_server: responded {json.dumps(envelope)}")


if __name__ == "__main__":
    try:
        main()
    except BrokenPipeError:
        pass
