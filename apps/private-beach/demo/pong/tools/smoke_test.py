#!/usr/bin/env python3
"""
Lightweight smoke test for the Private Beach Pong harness integration.

The script verifies that:
  * Sessions attached to the specified Private Beach expose `pong_role` metadata.
  * At least one agent session exists (role == "agent").
  * Paddle sessions (`lhs`, `rhs`) are discoverable.
  * Controller pairings are present for each agent→paddle mapping.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from typing import Dict, Iterable

from manager_client import (  # type: ignore
    ManagerRequestError,
    PrivateBeachManagerClient,
)


def metadata_dict(raw: object) -> Dict[str, object]:
    if isinstance(raw, dict):
        return raw
    if isinstance(raw, str):
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            return {}
        if isinstance(parsed, dict):
            return parsed
    return {}


def parse_args(argv: Iterable[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Pong harness smoke test")
    parser.add_argument(
        "--manager-url",
        required=True,
        help="Beach Manager base URL (e.g. https://manager.private-beach.test/api).",
    )
    parser.add_argument(
        "--private-beach-id",
        required=True,
        help="Private Beach identifier to inspect.",
    )
    parser.add_argument(
        "--auth-token",
        help="Bearer token with pb:sessions.read/pb:control.write scope.",
    )
    parser.add_argument(
        "--expected-roles",
        default="lhs,rhs",
        help="Comma-separated list of paddle roles that must be present (default: lhs,rhs).",
    )
    parser.add_argument(
        "--controller-tag",
        help="Restrict checks to the agent whose metadata pong_tag matches this value.",
    )
    return parser.parse_args(argv)


def main() -> int:
    args = parse_args()
    token = args.auth_token or os.environ.get("PB_MANAGER_TOKEN")
    client = PrivateBeachManagerClient(args.manager_url, token)
    expected_roles = {role.strip() for role in args.expected_roles.split(",") if role.strip()}

    try:
        sessions = list(client.list_sessions(args.private_beach_id))
    except ManagerRequestError as exc:
        print(f"error: failed to list sessions — {exc}", file=sys.stderr)
        return 1

    if not sessions:
        print("error: no sessions attached to the private beach", file=sys.stderr)
        return 1

    agents = []
    paddles: Dict[str, str] = {}

    for summary in sessions:
        session_id = summary.get("session_id")
        metadata = metadata_dict(summary.get("metadata"))
        role = metadata.get("pong_role")
        tag = metadata.get("pong_tag")

        if role == "agent":
            if args.controller_tag and tag != args.controller_tag:
                continue
            agents.append(session_id)
        elif role in expected_roles and isinstance(session_id, str):
            paddles[role] = session_id

    missing_roles = [role for role in expected_roles if role not in paddles]
    if missing_roles:
        print(f"error: missing paddle sessions for roles: {', '.join(missing_roles)}", file=sys.stderr)
        return 1

    if not agents:
        print("error: no agent sessions discovered with matching tag", file=sys.stderr)
        return 1

    for agent_session in agents:
        try:
            pairings = list(client.list_controller_pairings(agent_session))
        except ManagerRequestError as exc:
            print(f"error: failed to list pairings for {agent_session}: {exc}", file=sys.stderr)
            return 1

        paired_children = {item.get("child_session_id") for item in pairings}
        missing = [
            role
            for role, session_id in paddles.items()
            if session_id not in paired_children
        ]
        if missing:
            print(
                f"error: controller {agent_session} missing pairings for roles: {', '.join(missing)}",
                file=sys.stderr,
            )
            return 1

    print("✓ Pong harness smoke test passed")
    print(f"  Agents: {len(agents)} — Paddles: {', '.join(sorted(paddles.keys()))}")
    return 0


if __name__ == "__main__":  # pragma: no cover - CLI entrypoint
    raise SystemExit(main())
