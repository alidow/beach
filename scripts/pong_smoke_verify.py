#!/usr/bin/env python3
import argparse
import json
import sys
from pathlib import Path
from typing import Dict, List, Tuple


def find_run_dir(log_root: Path) -> Path:
    latest = log_root / "latest"
    if latest.is_dir():
        return latest.resolve()
    candidates = [p for p in log_root.iterdir() if p.is_dir()]
    if not candidates:
        raise FileNotFoundError(f"no run directories under {log_root}")
    candidates.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    return candidates[0]


def load_state(path: Path) -> List[Dict]:
    records: List[Dict] = []
    with path.open("r", encoding="utf-8") as fh:
        for raw in fh:
            line = raw.strip()
            if not line:
                continue
            try:
                records.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    if not records:
        raise ValueError(f"{path} is empty or invalid")
    return records


def collect_ball_points(records: List[Dict]) -> List[Tuple[float, float]]:
    points: List[Tuple[float, float]] = []
    for rec in records:
        ball = rec.get("ball")
        if ball is None:
            continue
        if not isinstance(ball, dict):
            raise ValueError("ball entry is not an object")
        x = ball.get("x")
        if isinstance(x, (int, float)):
            t = rec.get("time")
            try:
                ts = float(t)
            except (TypeError, ValueError):
                ts = 0.0
            points.append((ts, float(x)))
    points.sort(key=lambda pair: pair[0])
    return points


def paddle_motion(records: List[Dict]) -> float:
    ys: List[float] = []
    for rec in records:
        paddle = rec.get("paddle") or {}
        y = paddle.get("y")
        if isinstance(y, (int, float)):
            ys.append(float(y))
    if not ys:
        return 0.0
    return max(ys) - min(ys)


def check_left_right_left(points: List[Tuple[float, float]], threshold: float = 1.0) -> bool:
    if len(points) < 3:
        return False
    xs = [p[1] for p in points]
    min_idx = xs.index(min(xs))
    max_idx = xs.index(max(xs))
    if min_idx >= max_idx or max_idx >= len(xs) - 1:
        return False
    tail_min = min(xs[max_idx + 1 :])
    return (xs[max_idx] - tail_min) >= threshold


def check_ball_gaps(points: List[Tuple[float, float]], max_gap: float) -> bool:
    if len(points) < 2:
        return True
    last = points[0][0]
    for ts, _ in points[1:]:
        if ts - last > max_gap:
            return False
        last = ts
    return True


def scan_logs(log_dir: Path) -> List[str]:
    hits: List[str] = []
    for log_file in log_dir.rglob("*.log"):
        try:
            text = log_file.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        lower = text.lower()
        if "peer left" in lower:
            hits.append(f"{log_file}: contains 'peer left'")
        if "rtc_ready=false" in lower:
            hits.append(f"{log_file}: contains 'rtc_ready=false'")
    return hits


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify browserless Pong smoke output")
    parser.add_argument("--log-root", default="/tmp/pong-stack", help="Root directory containing pong-stack runs")
    parser.add_argument("--run-dir", default=None, help="Explicit run directory (defaults to latest under log root)")
    parser.add_argument("--beach-id", default=None, help="Optional beach id for logging")
    parser.add_argument("--max-missing-seconds", type=float, default=3.0, help="Max allowed gap without a ball sample")
    args = parser.parse_args()

    log_root = Path(args.log_root)
    run_dir = Path(args.run_dir) if args.run_dir else find_run_dir(log_root)
    state_dir = run_dir / "state-trace"
    lhs_path = state_dir / "state-lhs.jsonl"
    rhs_path = state_dir / "state-rhs.jsonl"

    if not lhs_path.exists() or not rhs_path.exists():
        raise SystemExit(f"missing state trace files in {state_dir}")

    lhs_records = load_state(lhs_path)
    rhs_records = load_state(rhs_path)

    lhs_motion = paddle_motion(lhs_records)
    rhs_motion = paddle_motion(rhs_records)
    if lhs_motion < 0.5 or rhs_motion < 0.5:
        raise SystemExit(
            f"paddle motion too low (lhs range {lhs_motion:.2f}, rhs range {rhs_motion:.2f}); controllers may be idle"
        )

    ball_points = collect_ball_points(lhs_records) + collect_ball_points(rhs_records)
    if not ball_points:
        raise SystemExit("no ball samples found in state traces")

    if not check_left_right_left(ball_points):
        raise SystemExit("ball x did not traverse left→right→left in traces")

    if not check_ball_gaps(ball_points, args.max_missing_seconds):
        raise SystemExit(f"ball missing for more than {args.max_missing_seconds}s")

    log_hits = scan_logs(run_dir)
    if log_hits:
        formatted = "\n".join(log_hits)
        raise SystemExit(f"transport/log warnings detected:\n{formatted}")

    print(
        f"[pong-smoke] PASS for run {run_dir} "
        f"(beach_id={args.beach_id or 'n/a'}, paddle_motion=lhs:{lhs_motion:.2f}/rhs:{rhs_motion:.2f}, samples={len(ball_points)})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
