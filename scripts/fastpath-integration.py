#!/usr/bin/env python3
"""Headless fast-path integration harness.

Spins up the Pong demo inside docker-compose, captures logs, and fails if
controller fast-path channels churn (timeouts, repeated fallbacks, 404 storms).
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
import json
import re
import shlex
from subprocess import CalledProcessError
from urllib import error as urlerror
from urllib import request as urlrequest

try:
    import tomllib as toml
except ModuleNotFoundError:  # py39 compat
    import tomli as toml  # type: ignore

SERVICES = [
    "postgres",
    "redis",
    "coturn",
    "beach-gate",
    "beach-road",
    "beach-manager",
    "db-migrate",
    "db-seed",
]

REPO_ROOT = Path(__file__).resolve().parents[1]
PONG_STACK_SCRIPT = str(REPO_ROOT / "apps/private-beach/demo/pong/tools/pong-stack.sh")
MANAGER_BASE_URL = os.environ.get("FASTPATH_MANAGER_URL", "http://127.0.0.1:8080").rstrip("/")
HARNESS_LEASE_REASON = os.environ.get("FASTPATH_LEASE_REASON", "fastpath-integration")
AGENT_LEASE_REASON = os.environ.get("FASTPATH_AGENT_LEASE_REASON", "pong_showcase")
AUTH_GATEWAY_URL = os.environ.get(
    "FASTPATH_AUTH_GATEWAY",
    os.environ.get("BEACH_AUTH_GATEWAY", "http://127.0.0.1:4133"),
).rstrip("/")
AUTH_PROFILE = os.environ.get("FASTPATH_AUTH_PROFILE", "local")
FASTPATH_ACCOUNT_ID = os.environ.get(
    "FASTPATH_ACCOUNT_ID", "00000000-0000-0000-0000-000000000001"
)
FASTPATH_ACCOUNT_SUBJECT = os.environ.get(
    "FASTPATH_ACCOUNT_SUBJECT", "fastpath-harness"
)
TCP_PORTS_TO_FREE = [8080, 4132, 4133, 5173]
FRAME_DUMP_REMOTE_DIR = "/tmp/pong-stack/frame-dumps"
FRAME_SAMPLE_COUNT = int(os.environ.get("FASTPATH_FRAME_SAMPLES", "12"))
FRAME_SAMPLE_INTERVAL = float(os.environ.get("FASTPATH_FRAME_SAMPLE_INTERVAL", "0.5"))
FRAME_WARMUP_SECONDS = float(os.environ.get("FASTPATH_FRAME_WARMUP", "1"))
FRAME_CAPTURE_ENABLED = os.environ.get("FASTPATH_CAPTURE_ENABLED", "1").lower() not in {
    "0",
    "false",
    "no",
}
BALL_TRACE_REMOTE_DIR = os.environ.get(
    "FASTPATH_BALL_TRACE_DIR", "/tmp/pong-stack/ball-trace"
)
BALL_TRACE_MIN_SAMPLES = int(os.environ.get("FASTPATH_BALL_TRACE_MIN_SAMPLES", "40"))
BALL_TRACE_SPAN_THRESHOLD = float(
    os.environ.get("FASTPATH_BALL_TRACE_SPAN", "15")
)
BALL_TRACE_DIRECTION_EPSILON = float(
    os.environ.get("FASTPATH_BALL_TRACE_DIRECTION_EPS", "0.25")
)
BALL_TRACE_GAP_MAX = float(os.environ.get("FASTPATH_BALL_TRACE_GAP", "8"))
BALL_TRACE_SEGMENT_GAP = float(
    os.environ.get("FASTPATH_BALL_TRACE_SEGMENT_GAP", "1.5")
)
COMMAND_TRACE_REMOTE_DIR = os.environ.get(
    "FASTPATH_COMMAND_TRACE_DIR", "/tmp/pong-stack/command-trace"
)
RALLY_DELAY_SECONDS = float(os.environ.get("FASTPATH_RALLY_DELAY", "6.0"))
COMMAND_READY_WAIT_SECONDS = float(os.environ.get("FASTPATH_COMMAND_READY_WAIT", "25"))
CONTROLLER_LEASE_TTL_MS = int(os.environ.get("FASTPATH_LEASE_TTL_MS", "120000"))
MANAGER_READY_TIMEOUT = int(os.environ.get("FASTPATH_MANAGER_WAIT", "600"))


def run(cmd: list[str], *, capture: bool = False, check: bool = True, env=None) -> subprocess.CompletedProcess:
    kwargs = {"text": True}
    if capture:
        kwargs["capture_output"] = True
    if env is not None:
        kwargs["env"] = env
    proc = subprocess.run(cmd, check=check, **kwargs)
    return proc


def docker_compose(*args: str, capture: bool = False) -> subprocess.CompletedProcess:
    return run(["docker", "compose", *args], capture=capture)


def require_env(var: str) -> str:
    value = os.environ.get(var)
    if not value:
        print(f"error: {var} must be set for fast-path integration", file=sys.stderr)
        sys.exit(1)
    return value


def write_file(path: Path, content: str) -> None:
    path.write_text(content)


def fetch_log(remote_path: str, local_path: Path) -> tuple[bool, str]:
    try:
        proc = docker_compose(
            "exec",
            "beach-manager",
            "bash",
            "-lc",
            f"cat {remote_path}",
            capture=True,
        )
        local_path.write_text(proc.stdout)
        return True, ""
    except CalledProcessError as exc:
        local_path.write_text(f"<missing: {remote_path}>\n")
        return False, f"missing {remote_path}: {exc}"


def capture_remote_file(remote_path: str) -> tuple[bool, str]:
    try:
        proc = docker_compose(
            "exec",
            "beach-manager",
            "bash",
            "-lc",
            f"cat {shlex.quote(remote_path)}",
            capture=True,
        )
        return True, proc.stdout
    except CalledProcessError as exc:
        return False, f"missing {remote_path}: {exc}"


def capture_frame_snapshots(temp_dir: Path, total_wait: float) -> None:
    if not FRAME_CAPTURE_ENABLED or FRAME_SAMPLE_COUNT <= 0 or FRAME_SAMPLE_INTERVAL <= 0:
        if total_wait > 0:
            time.sleep(total_wait)
        return
    warmup = max(min(FRAME_WARMUP_SECONDS, total_wait), 0)
    if warmup:
        time.sleep(warmup)
    for idx in range(FRAME_SAMPLE_COUNT):
        time.sleep(FRAME_SAMPLE_INTERVAL)
        for role in ("lhs", "rhs"):
            remote = f"{FRAME_DUMP_REMOTE_DIR}/frame-{role}.txt"
            ok, content = capture_remote_file(remote)
            local = temp_dir / f"{role}-frame-{idx}.txt"
            if ok:
                local.write_text(content)
            else:
                local.write_text("")
    remaining = total_wait - warmup - FRAME_SAMPLE_COUNT * FRAME_SAMPLE_INTERVAL
    if remaining > 0:
        time.sleep(remaining)


def resolve_credentials_token(preferred_profile: str | None = None) -> str | None:
    cred_path = Path.home() / ".beach" / "credentials"
    if not cred_path.exists():
        return None
    try:
        parsed = toml.loads(cred_path.read_text())
    except (toml.TOMLDecodeError, OSError):
        return None
    profiles = parsed.get("profiles") or {}
    candidates: list[str] = []
    if preferred_profile:
        candidates.append(preferred_profile)
    env_profile = os.environ.get("BEACH_PROFILE")
    if env_profile and env_profile not in candidates:
        candidates.append(env_profile)
    current_profile = parsed.get("current_profile")
    if current_profile and current_profile not in candidates:
        candidates.append(current_profile)
    for extra in sorted(profiles.keys()):
        if extra not in candidates:
            candidates.append(extra)
    for name in candidates:
        profile = profiles.get(name)
        if not profile:
            continue
        access = profile.get("access_token") or {}
        token = access.get("token")
        expires_at = access.get("expires_at")
        if not token:
            continue
        if expires_at:
            normalized = expires_at.replace("Z", "+00:00")
            try:
                expiry = datetime.fromisoformat(normalized)
                if expiry <= datetime.now(timezone.utc):
                    continue
            except ValueError:
                pass
        return token
    return None


def refresh_credentials_via_cli(profile: str) -> None:
    env = os.environ.copy()
    env.setdefault("BEACH_AUTH_GATEWAY", AUTH_GATEWAY_URL)
    print(f"[fastpath] refreshing Beach Auth profile '{profile}' via beach login...")
    run(
        [
            "cargo",
            "run",
            "--bin",
            "beach",
            "--",
            "login",
            "--name",
            profile,
            "--force",
        ],
        env=env,
    )


def resolve_manager_token(preferred_profile: str | None = None) -> str:
    env_token = os.environ.get("PRIVATE_BEACH_MANAGER_TOKEN", "").strip()
    if env_token:
        return env_token
    token = resolve_credentials_token(preferred_profile)
    if token:
        print("[fastpath] using token from ~/.beach/credentials")
        return token
    profile = preferred_profile or os.environ.get("BEACH_PROFILE") or "local"
    refresh_credentials_via_cli(profile)
    token = resolve_credentials_token(preferred_profile or profile)
    if token:
        print(f"[fastpath] using refreshed Beach Auth profile '{profile}'")
        return token
    print(
        f"error: unable to resolve Beach Auth token for profile '{profile}'",
        file=sys.stderr,
    )
    sys.exit(1)


def assert_single_fast_path(host_log: str, name: str, errors: list[str]) -> None:
    legacy = re.findall(r"fast path controller channel ready", host_log)
    modern = re.findall(
        r"actions channel open[^\n]+channel=mgr-actions", host_log
    )
    if legacy:
        count = len(legacy)
    else:
        count = len(modern)
    if count != 1:
        errors.append(f"{name}: expected 1 fast-path ready, found {count}")
    if '\"pb-controller\"' in host_log:
        errors.append(f"{name}: pb-controller ready detected (fallback)")


def assert_no_timeouts(manager_log: str, errors: list[str]) -> None:
    if "timed out waiting for controller data channel" in manager_log:
        errors.append("manager: controller data channel timeout detected")


def assert_agent_stable(agent_log: str, errors: list[str]) -> None:
    fallback_count = agent_log.count("poller started (fallback)")
    if fallback_count > 2:  # initial attach may log twice (lhs/rhs)
        errors.append(f"agent: fallback occurred {fallback_count} times")


def assert_no_offer_404(manager_log: str, errors: list[str]) -> None:
    if "/webrtc/offer" in manager_log and "404" in manager_log:
        errors.append("manager: detected /webrtc/offer 404 responses")


def assert_action_throughput(agent_log: str, errors: list[str]) -> None:
    if "ACTION" not in agent_log:
        errors.append("agent: no actions forwarded during test run")


def find_ball_column(frame_text: str) -> tuple[int, int] | None:
    lines = frame_text.splitlines()
    if not lines:
        return None
    width = max(len(line) for line in lines)
    for line in lines:
        if "│" not in line:
            continue
        idx = line.find("●")
        if idx != -1:
            return idx, width
    return None


def analyze_ball_motion(temp_dir: Path, errors: list[str]) -> None:
    frame_paths = sorted(temp_dir.glob("lhs-frame-*.txt"))
    positions: list[int] = []
    width = None
    for path in frame_paths:
        text = path.read_text(encoding="utf-8", errors="ignore")
        result = find_ball_column(text)
        if result:
            col, frame_width = result
            positions.append(col)
            if width is None:
                width = frame_width
    if width is None or len(positions) < 5:
        errors.append("gameplay: insufficient ball samples captured from frame dumps")
        return
    left_threshold = max(5, int(width * 0.2))
    right_threshold = max(width - left_threshold - 1, width - 5)
    left_hit = any(col <= left_threshold for col in positions)
    right_hit = any(col >= right_threshold for col in positions)
    last_sign = 0
    direction_changes = 0
    for idx in range(1, len(positions)):
        delta = positions[idx] - positions[idx - 1]
        if delta == 0:
            continue
        sign = 1 if delta > 0 else -1
        if last_sign == 0:
            last_sign = sign
        elif sign != last_sign:
            direction_changes += 1
            last_sign = sign
    if not left_hit or not right_hit:
        errors.append("gameplay: ball never reached both sides of the court")
    if direction_changes < 2:
        errors.append("gameplay: ball did not bounce off both paddles (insufficient direction changes)")


def load_command_trace(temp_dir: Path, role: str) -> list[str]:
    path = temp_dir / f"command-trace-{role}.log"
    if not path.exists():
        return []
    commands: list[str] = []
    for raw in path.read_text(encoding="utf-8", errors="ignore").splitlines():
        if not raw.strip():
            continue
        try:
            record = json.loads(raw)
        except json.JSONDecodeError:
            continue
        cmd = record.get("command")
        if isinstance(cmd, str):
            commands.append(cmd.strip())
    return commands


def analyze_command_traces(temp_dir: Path, errors: list[str]) -> dict[str, list[str]]:
    traces: dict[str, list[str]] = {}
    for role in ("lhs", "rhs"):
        commands = load_command_trace(temp_dir, role)
        if not commands:
            errors.append(f"{role}: no controller commands captured")
        else:
            traces[role] = commands
            if not any(cmd.startswith("b ") for cmd in commands):
                errors.append(f"{role}: no ball spawn command recorded in trace")
    return traces


def load_ball_trace(temp_dir: Path, role: str) -> list[tuple[float, float, float]]:
    path = temp_dir / f"ball-trace-{role}.jsonl"
    if not path.exists():
        return []
    entries: list[tuple[float, float, float]] = []
    for raw in path.read_text(encoding="utf-8", errors="ignore").splitlines():
        raw = raw.strip()
        if not raw:
            continue
        try:
            record = json.loads(raw)
        except json.JSONDecodeError:
            continue
        try:
            timestamp = float(record.get("time", 0.0))
            x = float(record.get("x", 0.0))
            y = float(record.get("y", 0.0))
        except (TypeError, ValueError):
            continue
        entries.append((timestamp, x, y))
    entries.sort(key=lambda item: item[0])
    if not entries:
        return entries
    last_start = 0
    for idx in range(1, len(entries)):
        if entries[idx][0] - entries[idx - 1][0] > BALL_TRACE_SEGMENT_GAP:
            last_start = idx
    if last_start > 0:
        entries = entries[last_start:]
    return entries


def _summarize_trace(
    role: str, trace: list[tuple[float, float, float]], errors: list[str]
) -> dict[str, float] | None:
    if len(trace) < BALL_TRACE_MIN_SAMPLES:
        errors.append(
            f"gameplay: insufficient ball samples for {role} (need {BALL_TRACE_MIN_SAMPLES}, found {len(trace)})"
        )
        return None
    xs = [pt[1] for pt in trace]
    span = max(xs) - min(xs)
    if span < BALL_TRACE_SPAN_THRESHOLD:
        errors.append(
            f"{role}: ball travel span {span:.1f} too small (expected > {BALL_TRACE_SPAN_THRESHOLD})"
        )
    direction_changes = 0
    last_sign = 0
    for idx in range(1, len(xs)):
        delta = xs[idx] - xs[idx - 1]
        if abs(delta) < BALL_TRACE_DIRECTION_EPSILON:
            continue
        sign = 1 if delta > 0 else -1
        if last_sign == 0:
            last_sign = sign
        elif sign != last_sign:
            direction_changes += 1
            last_sign = sign
    if direction_changes < 1:
        errors.append(f"{role}: ball never bounced off paddle (no direction change)")
    initial_sign = 0
    for idx in range(1, len(xs)):
        delta = xs[idx] - xs[idx - 1]
        if abs(delta) >= BALL_TRACE_DIRECTION_EPSILON:
            initial_sign = 1 if delta > 0 else -1
            break
    expected_initial = -1 if role == "lhs" else 1
    if initial_sign != 0 and initial_sign != expected_initial:
        direction = "right" if initial_sign > 0 else "left"
        errors.append(
            f"{role}: unexpected initial direction ({direction}); expected {'right' if expected_initial > 0 else 'left'}"
        )
    return {
        "span": span,
        "start_time": trace[0][0],
        "end_time": trace[-1][0],
        "min_x": min(xs),
        "max_x": max(xs),
    }


def analyze_ball_traces(temp_dir: Path, errors: list[str]) -> bool:
    traces = {role: load_ball_trace(temp_dir, role) for role in ("lhs", "rhs")}
    summaries = {}
    for role, data in traces.items():
        summary = _summarize_trace(role, data, errors)
        if summary:
            summaries[role] = summary
    if len(summaries) < 2:
        return False
    lhs_end = summaries["lhs"]["end_time"]
    rhs_start = summaries["rhs"]["start_time"]
    if rhs_start < lhs_end:
        errors.append("gameplay: RHS rally overlapped with LHS (expected sequential handoff)")
    elif rhs_start - lhs_end > BALL_TRACE_GAP_MAX:
        errors.append(
            f"gameplay: delay between LHS and RHS rallies too long ({rhs_start - lhs_end:.1f}s > {BALL_TRACE_GAP_MAX}s)"
        )
    return True


def free_udp_range(start: int, end: int) -> None:
    for port in range(start, end + 1):
        proc = subprocess.run(
            ["lsof", "-ti", f"udp:{port}"],
            text=True,
            capture_output=True,
            check=False,
        )
        pids = {pid.strip() for pid in proc.stdout.splitlines() if pid.strip()}
        for pid in pids:
            subprocess.run(["kill", "-9", pid], check=False)


def free_tcp_ports(ports: list[int]) -> None:
    for port in ports:
        proc = subprocess.run(
            ["lsof", "-ti", f"tcp:{port}"],
            text=True,
            capture_output=True,
            check=False,
        )
        pids = {pid.strip() for pid in proc.stdout.splitlines() if pid.strip()}
        if pids:
            print(f"[fastpath] killing processes on tcp:{port}: {', '.join(pids)}")
        for pid in pids:
            subprocess.run(["kill", "-9", pid], check=False)


def create_private_beach_via_api(token: str) -> tuple[str | None, str | None]:
    name = f"FastPath {uuid.uuid4().hex[:6]}"
    slug = f"fastpath-{uuid.uuid4().hex[:8]}"
    payload = json.dumps({"name": name, "slug": slug}).encode()
    req = urlrequest.Request(
        f"{MANAGER_BASE_URL}/private-beaches",
        data=payload,
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {token}",
        },
    )
    try:
        with urlrequest.urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode())
            return data.get("id"), data.get("slug")
    except (urlerror.URLError, json.JSONDecodeError) as exc:
        print(f"[fastpath] failed to create private beach via API: {exc}")
        return None, None


def delete_private_beach(beach_id: str) -> None:
    try:
        docker_compose(
            "exec",
            "postgres",
            "psql",
            "-U",
            "postgres",
            "-d",
            "beach_manager",
            "-c",
            f"DELETE FROM private_beach WHERE id='{beach_id}'",
        )
    except CalledProcessError as exc:
        print(f"[fastpath] warning: failed to delete private beach {beach_id}: {exc}")


def wait_for_postgres() -> None:
    time.sleep(2)
    for _ in range(30):
        try:
            docker_compose(
                "exec",
                "postgres",
                "pg_isready",
                "-U",
                "postgres",
                "-d",
                "beach_manager",
            )
            return
        except CalledProcessError:
            time.sleep(1)
    print("error: postgres never became ready", file=sys.stderr)
    sys.exit(1)


def extract_session_info(path: Path) -> tuple[str, str] | None:
    if not path.exists():
        return None
    text = path.read_text()
    marker = '{"schema":2'
    start = text.find(marker)
    if start == -1:
        return None
    line = text[start:].splitlines()[0]
    try:
        data = json.loads(line)
        return data["session_id"], data.get("join_code", "")
    except json.JSONDecodeError:
        return None


def attach_sessions(private_beach_id: str, sessions: dict[str, dict[str, str]], token: str) -> list[str]:
    errors: list[str] = []
    for role, info in sessions.items():
        payload = json.dumps(
            {
                "session_id": info["session_id"],
                "code": info["join_code"],
            }
        ).encode()
        req = urlrequest.Request(
            f"{MANAGER_BASE_URL}/private-beaches/{private_beach_id}/sessions/attach-by-code",
            data=payload,
            method="POST",
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {token}",
            },
        )
        try:
            with urlrequest.urlopen(req, timeout=10):
                continue
        except urlerror.HTTPError as exc:
            body = exc.read().decode() if hasattr(exc, "read") else ""
            errors.append(
                f"attach-by-code failed for {role}: status={exc.code} body={body}"
            )
        except urlerror.URLError as exc:
            errors.append(f"attach-by-code failed for {role}: {exc}")
    return errors


def acquire_controller_token(
    session_id: str,
    manager_token: str,
    reason: str = HARNESS_LEASE_REASON,
) -> str | None:
    body = {"reason": reason}
    if CONTROLLER_LEASE_TTL_MS > 0:
        body["ttl_ms"] = CONTROLLER_LEASE_TTL_MS
    payload = json.dumps(body).encode()
    req = urlrequest.Request(
        f"{MANAGER_BASE_URL}/sessions/{session_id}/controller/lease",
        data=payload,
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {manager_token}",
        },
    )
    try:
        with urlrequest.urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode())
            return data.get("controller_token")
    except (urlerror.URLError, json.JSONDecodeError) as exc:
        print(f"[fastpath] failed to acquire controller lease for {session_id}: {exc}")
        return None


def verify_agent_lease_inside_container(session_id: str) -> tuple[bool, str]:
    """Attempt to acquire a controller lease from inside beach-manager."""

    lease_body = {"reason": AGENT_LEASE_REASON}
    if CONTROLLER_LEASE_TTL_MS > 0:
        lease_body["ttl_ms"] = CONTROLLER_LEASE_TTL_MS
    script = f"""
import json
import sys
from pathlib import Path
try:
    import tomllib as toml
except ModuleNotFoundError:  # py<3.11
    import tomli as toml  # type: ignore
import urllib.request
import urllib.error

profile = {AUTH_PROFILE!r}
path = Path('/root/.beach/credentials')
data = toml.loads(path.read_text())
token = data.get('profiles', {{}}).get(profile, {{}}).get('access_token', {{}}).get('token')
if not token:
    print('error:no_token')
    sys.exit(2)
payload = json.dumps({lease_body!r}).encode()
req = urllib.request.Request(
    'http://localhost:8080/sessions/{session_id}/controller/lease',
    data=payload,
    method='POST',
    headers={{'Content-Type': 'application/json', 'Authorization': f'Bearer {{token}}'}},
)
try:
    with urllib.request.urlopen(req, timeout=5) as resp:
        print(f'ok:{{resp.status}}')
        sys.exit(0)
except urllib.error.HTTPError as exc:  # pragma: no cover - network conditions
    detail = exc.read().decode('utf-8', errors='ignore')
    print(f'error:{{exc.code}}:{{detail}}')
    sys.exit(exc.code or 1)
except Exception as exc:  # pragma: no cover - defensive
    print(f'error:0:{{exc}}')
    sys.exit(1)
"""
    proc = docker_compose(
        "exec",
        "-T",
        "beach-manager",
        "python3",
        "-c",
        script,
        capture=True,
    )
    output = (proc.stdout or proc.stderr).strip()
    if proc.returncode == 0:
        return True, output
    return False, output or f"exit code {proc.returncode}"


def queue_terminal_write(
    session_id: str,
    controller_token: str,
    command: str,
    manager_token: str,
    trace_id: str | None = None,
) -> bool:
    action = {
        "id": str(uuid.uuid4()),
        "action_type": "terminal_write",
        "payload": {"bytes": f"{command}\n"},
        "expires_at": None,
    }
    payload = json.dumps({"controller_token": controller_token, "actions": [action]}).encode()
    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {manager_token}",
    }
    if trace_id:
        headers["X-Trace-Id"] = trace_id
    req = urlrequest.Request(
        f"{MANAGER_BASE_URL}/sessions/{session_id}/actions",
        data=payload,
        method="POST",
        headers=headers,
    )
    try:
        with urlrequest.urlopen(req, timeout=10):
            return True
    except urlerror.HTTPError as exc:
        body = exc.read().decode(errors="ignore") if hasattr(exc, "read") else ""
        print(
            f"[fastpath] queue_actions failed for session {session_id}: status={exc.code} body={body}"
        )
        return False
    except urlerror.URLError as exc:
        print(f"[fastpath] queue_actions failed for session {session_id}: {exc}")
        return False


def trigger_ball_rally(
    sessions: dict[str, dict[str, str]],
    controller_tokens: dict[str, str],
    manager_token: str,
) -> None:
    base_command = os.environ.get("FASTPATH_SPAWN_COMMAND", "b 12 25 0")
    for idx, role in enumerate(("lhs", "rhs")):
        session_id = sessions.get(role, {}).get("session_id")
        token = controller_tokens.get(role)
        if not session_id or not token:
            continue
        command = os.environ.get(
            f"FASTPATH_SPAWN_COMMAND_{role.upper()}", base_command
        )
        trace_id = f"fastpath-harness-{role}-{session_id[:6]}"
        if queue_terminal_write(session_id, token, command, manager_token, trace_id):
            print(f"[fastpath] spawned ball for rally ({role})")
        else:
            print(f"[fastpath] failed to spawn ball for rally ({role})")
        if idx == 0 and RALLY_DELAY_SECONDS > 0:
            time.sleep(RALLY_DELAY_SECONDS)


def wait_for_manager() -> None:
    for _ in range(MANAGER_READY_TIMEOUT):
        try:
            with urlrequest.urlopen(f"{MANAGER_BASE_URL}/healthz", timeout=2):
                return
        except (urlerror.URLError, ConnectionResetError):
            time.sleep(1)
    print("error: manager never became ready", file=sys.stderr)
    sys.exit(1)


def ensure_fastpath_account() -> None:
    sql = (
        "INSERT INTO account (id, type, status, beach_gate_subject, display_name, email) "
        f"VALUES ('{FASTPATH_ACCOUNT_ID}', 'service', 'active', '{FASTPATH_ACCOUNT_SUBJECT}', "
        "'FastPath Harness', 'fastpath@beach.test') "
        "ON CONFLICT (id) DO NOTHING;"
    )
    docker_compose(
        "exec",
        "postgres",
        "psql",
        "-U",
        "postgres",
        "-d",
        "beach_manager",
        "-c",
        sql,
    )


def parse_session_table(output: str) -> dict[str, dict[str, str]]:
    sessions: dict[str, dict[str, str]] = {}
    pattern = re.compile(r"^(lhs|rhs|agent)\s+([0-9a-f\-]+)\s+(\S+)")
    for line in output.splitlines():
        match = pattern.match(line.strip())
        if match:
            sessions[match.group(1)] = {
                "session_id": match.group(2),
                "join_code": match.group(3),
            }
    return sessions


def main() -> tuple[int, str | None]:
    require_env("BEACH_ICE_PUBLIC_IP")
    require_env("BEACH_ICE_PUBLIC_HOST")

    temp_dir = Path("temp/fastpath-integration")
    temp_dir.mkdir(parents=True, exist_ok=True)

    port_start = int(os.environ.get("BEACH_ICE_PORT_START", "62000"))
    port_end = int(os.environ.get("BEACH_ICE_PORT_END", "62100"))
    print(f"[fastpath] ensuring UDP ports {port_start}-{port_end} are free...")
    free_udp_range(port_start, port_end)
    print("[fastpath] ensuring TCP ports are free...")
    free_tcp_ports(TCP_PORTS_TO_FREE)

    print("[fastpath] starting docker services...")
    docker_compose("up", "-d", *SERVICES)
    wait_for_postgres()
    ensure_fastpath_account()
    wait_for_manager()
    manager_token = resolve_manager_token(AUTH_PROFILE)

    run([PONG_STACK_SCRIPT, "stop"], check=False)

    fallback_beach = os.environ.get(
        "FASTPATH_PRIVATE_BEACH_ID", "11111111-1111-1111-1111-111111111111"
    )
    created_beach_id, _ = create_private_beach_via_api(manager_token)
    if created_beach_id:
        stack_beach_id = created_beach_id
    else:
        print("[fastpath] warning: falling back to seeded private beach ID")
        stack_beach_id = fallback_beach

    print(f"[fastpath] launching pong stack {stack_beach_id}...")
    stack_env = os.environ.copy()
    stack_env.setdefault("PONG_FRAME_DUMP_DIR", FRAME_DUMP_REMOTE_DIR)
    stack_env.setdefault("PONG_FRAME_DUMP_INTERVAL", os.environ.get("FASTPATH_FRAME_DUMP_INTERVAL", "0.1"))
    stack_env.setdefault("PONG_BALL_TRACE_DIR", BALL_TRACE_REMOTE_DIR)
    stack_env.setdefault("PONG_COMMAND_TRACE_DIR", COMMAND_TRACE_REMOTE_DIR)
    stack_env.setdefault("PONG_LOG_LEVEL", os.environ.get("FASTPATH_PLAYER_LOG_LEVEL", "debug"))
    start_proc = run(
        [PONG_STACK_SCRIPT, "start", stack_beach_id],
        capture=True,
        env=stack_env,
    )
    start_log = start_proc.stdout
    write_file(temp_dir / "pong-stack-start.log", start_log)
    sessions = parse_session_table(start_log)
    if len(sessions) != 3:
        print(start_log)
        print("error: failed to parse session codes", file=sys.stderr)
        return 1, created_beach_id

    attach_errors = attach_sessions(stack_beach_id, sessions, manager_token)
    controller_tokens: dict[str, str] = {}
    for role in ("lhs", "rhs"):
        session_id = sessions.get(role, {}).get("session_id")
        if not session_id:
            errors_msg = f"{role} host: session id missing; cannot acquire lease"
            attach_errors.append(errors_msg)
            continue
        token = acquire_controller_token(session_id, manager_token)
        if token:
            controller_tokens[role] = token
        else:
            attach_errors.append(f"{role} host: failed to acquire controller lease")

    agent_session_id = sessions.get("agent", {}).get("session_id")
    if agent_session_id:
        agent_token = acquire_controller_token(
            agent_session_id,
            manager_token,
            reason=AGENT_LEASE_REASON,
        )
        if agent_token:
            print(
                "[fastpath] verified agent controller lease via "
                f"reason='{AGENT_LEASE_REASON}'"
            )
        else:
            attach_errors.append(
                "agent: failed to acquire controller lease via manager API"
            )
        ok, detail = verify_agent_lease_inside_container(agent_session_id)
        if ok:
            print("[fastpath] agent controller lease succeeded from beach-manager container")
        else:
            attach_errors.append(
                "agent: controller lease failed inside beach-manager container "
                f"({detail or 'no details'})"
            )
    else:
        attach_errors.append("agent: session id missing; cannot verify controller lease")
    time.sleep(5)
    if COMMAND_READY_WAIT_SECONDS > 0:
        print(
            f"[fastpath] waiting {COMMAND_READY_WAIT_SECONDS:.1f}s for controllers to become ready..."
        )
        time.sleep(COMMAND_READY_WAIT_SECONDS)

    trigger_ball_rally(sessions, controller_tokens, manager_token)

    print("[fastpath] waiting for sessions to settle and capturing gameplay...")
    settle_seconds = float(os.environ.get("FASTPATH_WAIT_SECONDS", "30"))
    capture_frame_snapshots(temp_dir, settle_seconds)

    print("[fastpath] collecting logs...")
    log_map = {
        "beach-host-lhs.log": "/tmp/pong-stack/beach-host-lhs.log",
        "beach-host-rhs.log": "/tmp/pong-stack/beach-host-rhs.log",
        "beach-host-agent.log": "/tmp/pong-stack/beach-host-agent.log",
        "player-lhs.log": "/tmp/pong-stack/player-lhs.log",
        "player-rhs.log": "/tmp/pong-stack/player-rhs.log",
        "agent.log": "/tmp/pong-stack/agent.log",
        "manager.log": "logs/beach-manager/beach-manager.log",
        "bootstrap-lhs.json": "/tmp/pong-stack/bootstrap-lhs.json",
        "bootstrap-rhs.json": "/tmp/pong-stack/bootstrap-rhs.json",
        "bootstrap-agent.json": "/tmp/pong-stack/bootstrap-agent.json",
        "ball-trace-lhs.jsonl": f"{BALL_TRACE_REMOTE_DIR}/ball-trace-lhs.jsonl",
        "ball-trace-rhs.jsonl": f"{BALL_TRACE_REMOTE_DIR}/ball-trace-rhs.jsonl",
        "command-trace-lhs.log": f"{COMMAND_TRACE_REMOTE_DIR}/command-lhs.log",
        "command-trace-rhs.log": f"{COMMAND_TRACE_REMOTE_DIR}/command-rhs.log",
    }
    fetch_failures = []
    for local, remote in log_map.items():
        ok, err = fetch_log(remote, temp_dir / local)
        if not ok:
            fetch_failures.append(err)

    errors: list[str] = attach_errors.copy()
    assert_single_fast_path((temp_dir / "beach-host-lhs.log").read_text(), "lhs host", errors)
    assert_single_fast_path((temp_dir / "beach-host-rhs.log").read_text(), "rhs host", errors)
    assert_single_fast_path((temp_dir / "beach-host-agent.log").read_text(), "agent host", errors)
    manager_log = (temp_dir / "manager.log").read_text()
    assert_no_timeouts(manager_log, errors)
    assert_no_offer_404(manager_log, errors)
    agent_log = (temp_dir / "agent.log").read_text()
    assert_agent_stable(agent_log, errors)

    # Supplement sessions map with data from bootstrap files when available.
    for role in ("lhs", "rhs", "agent"):
        bootstrap = temp_dir / f"bootstrap-{role}.json"
        info = extract_session_info(bootstrap)
        if info:
            sessions[role]["session_id"] = info[0]

    analyze_command_traces(temp_dir, errors)
    traces_present = analyze_ball_traces(temp_dir, errors)
    if not traces_present:
        analyze_ball_motion(temp_dir, errors)
    errors.extend(fetch_failures)

    if errors:
        print("[fastpath] FAIL")
        for err in errors:
            print(f" - {err}")
        return 1, created_beach_id

    print("[fastpath] PASS – fast-path stayed stable for sessions:")
    for role, info in sessions.items():
        print(f"  {role}: {info['session_id']}")
    return 0, created_beach_id


if __name__ == "__main__":
    fallback_id = os.environ.get(
        "FASTPATH_PRIVATE_BEACH_ID", "11111111-1111-1111-1111-111111111111"
    )
    created_id: str | None = None
    keep_stack = os.environ.get("FASTPATH_KEEP_STACK", "0").lower() in {
        "1",
        "true",
        "yes",
    }
    try:
        exit_code, created_id = main()
    finally:
        run([PONG_STACK_SCRIPT, "stop"], check=False)
        if created_id and created_id != fallback_id:
            delete_private_beach(created_id)
        if not keep_stack:
            docker_compose("down")
    sys.exit(exit_code)
