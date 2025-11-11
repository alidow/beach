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
from collections import deque
from dataclasses import dataclass, field
from getpass import getpass
from pathlib import Path
from typing import Callable, Deque, Dict, Iterable, List, Optional, Set, Tuple
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

PADDLE_GLYPHS = frozenset(
    {
        "#",
        "|",
        "║",
        "│",
        "\u258c",  # ▌
        "\u2590",  # ▐
        "\u259b",  # ▛
        "\u259c",  # ▜
        "\u2599",  # ▙
        "\u259f",  # ▟
    }
)

BALL_GLYPHS = frozenset({"o", ".", "*", "\u25cf"})  # supports ● plus ascii fallbacks


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
        self._local_stop = threading.Event()

    def stop(self) -> None:
        self._local_stop.set()

    def run(self) -> None:  # pragma: no cover - network path
        label = self.role or "session"
        while not self.stop_event.is_set() and not self._local_stop.is_set():
            try:
                for payload in self.client.subscribe_state(self.session_id):
                    if self.stop_event.is_set() or self._local_stop.is_set():
                        return
                    event = {
                        "session_id": self.session_id,
                        "payload": payload,
                        "received_at": time.time(),
                    }
                    self.output.put(("diff", event))
            except ManagerRequestError as exc:
                self.output.put(
                    (
                        "error",
                        f"state stream error ({label} {self.session_id}): {exc}",
                    )
                )
                if self.stop_event.wait(2.0) or self._local_stop.wait(2.0):
                    break
            else:
                # Stream ended without error; avoid tight loop.
                if self.stop_event.wait(1.0) or self._local_stop.wait(1.0):
                    break


class PairingSubscriber(threading.Thread):
    def __init__(
        self,
        client: PrivateBeachManagerClient,
        controller_session_id: str,
        private_beach_id: str,
        output: "queue.Queue[Tuple[str, object]]",
        stop_event: threading.Event,
        on_pairing: Callable[[str, str, Dict[str, object]], None],
    ) -> None:
        super().__init__(daemon=True)
        self.client = client
        self.controller_session_id = controller_session_id
        self.private_beach_id = private_beach_id
        self.output = output
        self.stop_event = stop_event
        self._local_stop = threading.Event()
        self._callback = on_pairing

    def stop(self) -> None:
        self._local_stop.set()

    def run(self) -> None:  # pragma: no cover - network path
        while not self.stop_event.is_set() and not self._local_stop.is_set():
            try:
                for payload in self.client.subscribe_controller_pairings(
                    self.controller_session_id
                ):
                    if self.stop_event.is_set() or self._local_stop.is_set():
                        return
                    self._handle_event(payload)
            except ManagerRequestError as exc:
                self.output.put(("error", f"pairing stream error: {exc}"))
                if self.stop_event.wait(2.0) or self._local_stop.wait(2.0):
                    break

    def _handle_event(self, payload: Dict[str, object]) -> None:
        child_session_id = payload.get("child_session_id")
        action_raw = payload.get("action")
        pairing = payload.get("pairing") or {}
        if not isinstance(child_session_id, str):
            return
        action = str(action_raw or "").lower()
        metadata = fetch_relationship_metadata(
            self.client,
            self.private_beach_id,
            self.controller_session_id,
            child_session_id,
            self.output,
        )
        metadata["update_cadence"] = pairing.get("update_cadence")
        self._callback(child_session_id, action, metadata)
        self.output.put(
            (
                "info",
                f"pairing {action or 'unknown'} child={child_session_id} cadence={pairing.get('update_cadence')} trace={metadata.get('trace_id') or 'off'} poll={metadata.get('poll_frequency')}",
            )
        )


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


class StatePoller(threading.Thread):
    def __init__(
        self,
        client: PrivateBeachManagerClient,
        session_id: str,
        output: "queue.Queue[Tuple[str, object]]",
        global_stop: threading.Event,
        interval: float,
    ) -> None:
        super().__init__(daemon=True)
        self.client = client
        self.session_id = session_id
        self.output = output
        self._global_stop = global_stop
        self._local_stop = threading.Event()
        self._interval = max(float(interval), 0.5)
        self._interval_lock = threading.Lock()

    def update_interval(self, interval: float) -> None:
        with self._interval_lock:
            self._interval = max(float(interval), 0.5)

    def stop(self) -> None:
        self._local_stop.set()

    def _current_interval(self) -> float:
        with self._interval_lock:
            return self._interval

    def run(self) -> None:  # pragma: no cover - network path
        while not self._global_stop.is_set() and not self._local_stop.is_set():
            try:
                payload = self.client.fetch_state_snapshot(self.session_id)
                if payload:
                    event = {
                        "session_id": self.session_id,
                        "payload": payload,
                        "received_at": time.time(),
                    }
                    self.output.put(("diff", event))
            except ManagerRequestError as exc:
                self.output.put(
                    ("error", f"state poll error ({self.session_id}): {exc}")
                )
                if self._wait_interval(2.0):
                    break
                continue
            if self._wait_interval(self._current_interval()):
                break
        self.output.put(("info", f"state poller stopped for {self.session_id}"))

    def _wait_interval(self, interval: float) -> bool:
        delay = max(interval, 0.5)
        elapsed = 0.0
        step = min(0.5, delay)
        while elapsed < delay:
            remaining = delay - elapsed
            window = min(step, remaining)
            if self._global_stop.wait(window) or self._local_stop.is_set():
                return True
            elapsed += window
        return False


@dataclass
class AutopairContext:
    controller_session_id: str
    controller_token: str
    child_sessions: Dict[str, str]
    session_roles: Dict[str, str]
    lease_expires_at_ms: int
    prompt_pack: Optional[Dict[str, object]] = None
    mcp_bridges: Optional[List[Dict[str, object]]] = None


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


def _normalize_session_role(value: Optional[object]) -> Optional[str]:
    if not isinstance(value, str):
        return None
    normalized = value.strip().lower()
    if normalized in {"agent", "lhs", "rhs"}:
        return normalized
    if normalized in {"application", "app"}:
        return "application"
    return None


def _build_tile_indexes(
    layout: Optional[Dict[str, object]],
) -> Tuple[Dict[str, Dict[str, object]], Dict[str, Dict[str, object]]]:
    tile_lookup: Dict[str, Dict[str, object]] = {}
    session_lookup: Dict[str, Dict[str, object]] = {}
    if not layout or not isinstance(layout, dict):
        return tile_lookup, session_lookup
    tiles = layout.get("tiles")
    if not isinstance(tiles, dict):
        return tile_lookup, session_lookup
    for tile_id, raw in tiles.items():
        if not isinstance(raw, dict):
            continue
        metadata = _metadata_dict(raw.get("metadata"))
        session_meta = _metadata_dict(metadata.get("sessionMeta"))
        agent_meta = _metadata_dict(metadata.get("agentMeta"))
        session_id = session_meta.get("sessionId")
        position = raw.get("position")
        x_pos: Optional[float] = None
        if isinstance(position, dict):
            x_val = position.get("x")
            if isinstance(x_val, (int, float)):
                x_pos = float(x_val)
        tile_info = {
            "tile_id": tile_id,
            "session_id": session_id if isinstance(session_id, str) else None,
            "metadata": metadata,
            "agent_meta": agent_meta if agent_meta else None,
            "node_type": metadata.get("nodeType"),
            "position_x": x_pos,
        }
        tile_lookup[tile_id] = tile_info
        session_key = tile_info["session_id"]
        if isinstance(session_key, str) and session_key:
            session_lookup[session_key] = tile_info
    return tile_lookup, session_lookup


def _build_agent_relationship_index(
    layout: Optional[Dict[str, object]],
    tile_lookup: Dict[str, Dict[str, object]],
) -> Dict[str, List[Tuple[str, Optional[float]]]]:
    index: Dict[str, List[Tuple[str, Optional[float]]]] = {}
    if not layout or not isinstance(layout, dict):
        return index
    metadata = _metadata_dict(layout.get("metadata"))
    relationships = metadata.get("agentRelationships")
    if not isinstance(relationships, dict):
        return index
    for record in relationships.values():
        entry = _metadata_dict(record)
        source_id = entry.get("sourceId")
        target_id = entry.get("targetId")
        if not isinstance(source_id, str) or not isinstance(target_id, str):
            continue
        source_tile = tile_lookup.get(source_id)
        target_tile = tile_lookup.get(target_id)
        if not source_tile or not target_tile:
            continue
        source_session = source_tile.get("session_id")
        target_session = target_tile.get("session_id")
        if not isinstance(source_session, str) or not isinstance(target_session, str):
            continue
        target_x = target_tile.get("position_x")
        index.setdefault(source_session, []).append((target_session, target_x))
    return index


def _assign_children_from_layout(
    agent_session_id: str,
    current_children: Dict[str, str],
    relationship_index: Dict[str, List[Tuple[str, Optional[float]]]],
    session_tile_lookup: Dict[str, Dict[str, object]],
) -> Dict[str, str]:
    """Use agentRelationships/positions to infer lhs/rhs when metadata is missing."""
    updated = dict(current_children)
    agent_tile = session_tile_lookup.get(agent_session_id, {})
    agent_x = agent_tile.get("position_x")
    relationships = relationship_index.get(agent_session_id, [])
    if not relationships:
        return updated
    used_sessions = set(updated.values())
    left_candidates: List[Tuple[str, Optional[float]]] = []
    right_candidates: List[Tuple[str, Optional[float]]] = []
    fallback: List[Tuple[str, Optional[float]]] = []
    for session_id, pos in relationships:
        if not isinstance(session_id, str) or session_id == agent_session_id:
            continue
        if session_id in used_sessions:
            continue
        if isinstance(agent_x, (int, float)) and isinstance(pos, (int, float)):
            if pos <= agent_x:
                left_candidates.append((session_id, pos))
            else:
                right_candidates.append((session_id, pos))
        else:
            fallback.append((session_id, pos))

    def _assign_from(bucket, side):
        if side in updated:
            return
        bucket.sort(key=lambda entry: (float("inf") if entry[1] is None else entry[1], entry[0]))
        while bucket:
            session_id, _ = bucket.pop(0)
            if session_id in used_sessions:
                continue
            updated[side] = session_id
            used_sessions.add(session_id)
            return

    _assign_from(left_candidates, "lhs")
    _assign_from(right_candidates, "rhs")
    # If one side is still missing, fall back to any remaining related tiles.
    fallback.sort(key=lambda entry: (float("inf") if entry[1] is None else entry[1], entry[0]))
    for side in ("lhs", "rhs"):
        if side in updated:
            continue
        while fallback:
            session_id, _ = fallback.pop(0)
            if session_id in used_sessions:
                continue
            updated[side] = session_id
            used_sessions.add(session_id)
            break
    return updated


def _infer_tile_role(tile_info: Optional[Dict[str, object]]) -> Optional[str]:
    if not tile_info:
        return None
    node_type = tile_info.get("node_type")
    if isinstance(node_type, str) and node_type.lower() == "agent":
        return "agent"
    agent_meta = tile_info.get("agent_meta")
    if isinstance(agent_meta, dict) and agent_meta:
        return "agent"
    return "application"


def fetch_relationship_metadata(
    client: PrivateBeachManagerClient,
    private_beach_id: str,
    controller_session_id: str,
    child_session_id: str,
    output: "queue.Queue[Tuple[str, object]]",
) -> Dict[str, object]:
    meta: Dict[str, object] = {}
    now = time.time()
    layout: Optional[Dict[str, object]] = None
    cached = LAYOUT_CACHE.get(private_beach_id)
    if cached and now - cached[0] < LAYOUT_CACHE_TTL:
        layout = cached[1]
    else:
        try:
            layout = client.get_canvas_layout(private_beach_id)
            LAYOUT_CACHE[private_beach_id] = (now, layout)
        except ManagerRequestError as exc:
            output.put(("error", f"layout fetch failed: {exc}"))
            return meta
    session_to_tile: Dict[str, str] = {}
    tiles = layout.get("tiles")
    if isinstance(tiles, dict):
        for tile_id, tile_entry in tiles.items():
            metadata = _metadata_dict(_metadata_dict(tile_entry).get("metadata"))
            session_meta = _metadata_dict(metadata.get("sessionMeta"))
            session_id = session_meta.get("sessionId")
            if isinstance(session_id, str) and session_id.strip():
                session_to_tile[session_id.strip()] = tile_id
    agent_tile_id = session_to_tile.get(controller_session_id)
    child_tile_id = session_to_tile.get(child_session_id)
    if not agent_tile_id or not child_tile_id:
        return meta
    metadata = _metadata_dict(layout.get("metadata"))
    relationships = metadata.get("agentRelationships")
    if isinstance(relationships, dict):
        for raw in relationships.values():
            record = _metadata_dict(raw)
            if record.get("sourceId") == agent_tile_id and record.get("targetId") == child_tile_id:
                poll_freq = record.get("pollFrequency")
                if isinstance(poll_freq, (int, float)):
                    meta["poll_frequency"] = float(poll_freq)
                else:
                    meta["poll_frequency"] = None
                trace_id = resolve_trace_id_from_layout(layout, agent_tile_id)
                if trace_id:
                    meta["trace_id"] = trace_id
                return meta
    return meta


def resolve_trace_id_from_layout(layout: Dict[str, object], tile_id: Optional[object]) -> Optional[str]:
    if not isinstance(tile_id, str):
        return None
    tiles = layout.get("tiles")
    if not isinstance(tiles, dict):
        return None
    tile_entry = tiles.get(tile_id)
    if not isinstance(tile_entry, dict):
        return None
    metadata = _metadata_dict(tile_entry.get("metadata"))
    agent_meta = _metadata_dict(metadata.get("agentMeta"))
    trace_meta = _metadata_dict(agent_meta.get("trace"))
    enabled = trace_meta.get("enabled")
    trace_id = trace_meta.get("trace_id")
    if enabled and isinstance(trace_id, str) and trace_id.strip():
        return trace_id.strip()
    return None


# cache for expensive layout fetches: private_beach_id -> (timestamp, layout)
LAYOUT_CACHE: Dict[str, Tuple[float, Dict[str, object]]] = {}
LAYOUT_CACHE_TTL = 2.0  # seconds


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
    agent_prompt_pack: Optional[Dict[str, object]] = None
    agent_bridges: Optional[List[Dict[str, object]]] = None

    for attempt in range(max(1, attempts)):
        try:
            summaries = list(client.list_sessions(private_beach_id))
        except ManagerRequestError as exc:
            diff_queue.put(("error", f"session discovery failed: {exc}"))
            if attempt == attempts - 1:
                return None
            time.sleep(interval)
            continue
        layout: Optional[Dict[str, object]] = None
        try:
            layout = client.get_canvas_layout(private_beach_id)
        except ManagerRequestError as exc:
            diff_queue.put(("warn", f"layout discovery failed: {exc}"))
        tile_lookup, session_tile_lookup = _build_tile_indexes(layout)
        relationship_index = _build_agent_relationship_index(layout, tile_lookup)

        current_agent: Optional[str] = None
        current_children: Dict[str, str] = {}
        current_prompt_pack: Optional[Dict[str, object]] = None
        current_bridges: Optional[List[Dict[str, object]]] = None
        application_candidates: List[Tuple[str, Optional[float]]] = []

        for summary in summaries:
            session_id = summary.get("session_id")
            metadata = _metadata_dict(summary.get("metadata"))
            declared_role = _normalize_session_role(metadata.get("pong_role"))
            if declared_role is None:
                declared_role = _normalize_session_role(metadata.get("role"))
            tag = metadata.get("pong_tag")
            tile_info = session_tile_lookup.get(session_id or "") if session_id else None
            if not tile_info:
                tile_id = metadata.get("rewrite_tile_id")
                if isinstance(tile_id, str):
                    tile_info = tile_lookup.get(tile_id)
            tile_role = _infer_tile_role(tile_info)
            if declared_role == "application" and tile_role == "agent":
                declared_role = "agent"
            if declared_role is None and tile_role:
                declared_role = tile_role
            tile_tag = tile_info.get("tile_id") if tile_info else None
            if not isinstance(tag, str) and isinstance(tile_tag, str):
                tag = tile_tag

            if not isinstance(session_id, str):
                continue
            if declared_role in {"lhs", "rhs"}:
                current_children.setdefault(declared_role, session_id)
            elif declared_role == "application":
                if tile_role != "agent":
                    application_candidates.append(
                        (session_id, tile_info.get("position_x") if tile_info else None)
                    )
            if declared_role == "agent":
                if session_tag is None and current_agent is None:
                    current_agent = session_id
                elif session_tag and tag == session_tag:
                    current_agent = session_id
                agent_meta = metadata.get("agent")
                if not isinstance(agent_meta, dict) and tile_info:
                    agent_meta = _metadata_dict(tile_info.get("agent_meta"))
                if isinstance(agent_meta, dict):
                    pack = agent_meta.get("prompt_pack")
                    if isinstance(pack, dict):
                        current_prompt_pack = pack
                    bridges = agent_meta.get("mcp_bridges")
                    if isinstance(bridges, list):
                        filtered = [entry for entry in bridges if isinstance(entry, dict)]
                        if filtered:
                            current_bridges = filtered

        if current_agent:
            current_children = _assign_children_from_layout(
                current_agent,
                current_children,
                relationship_index,
                session_tile_lookup,
            )

        missing_sides = [side for side in ("lhs", "rhs") if side not in current_children]
        if missing_sides and application_candidates:
            application_candidates.sort(
                key=lambda entry: (float("inf") if entry[1] is None else entry[1], entry[0])
            )
            used = set(current_children.values())
            for side in missing_sides:
                assigned: Optional[str] = None
                for session_id, _ in application_candidates:
                    if session_id in used:
                        continue
                    assigned = session_id
                    used.add(session_id)
                    break
                if assigned:
                    current_children[side] = assigned

        if len(current_children) < 2 and application_candidates:
            # Fallback: pick the first two application sessions even if we cannot
            # confidently map them via layout metadata. This prevents the agent from
            # running without children when tiles were recreated without persisted roles.
            used = set(current_children.values())
            for session_id, _ in application_candidates:
                if session_id in used:
                    continue
                if "lhs" not in current_children:
                    current_children["lhs"] = session_id
                elif "rhs" not in current_children:
                    current_children["rhs"] = session_id
                used.add(session_id)
                if "lhs" in current_children and "rhs" in current_children:
                    diff_queue.put(
                        (
                            "warn",
                            "autopair fallback assigned paddle sessions "
                            f"(lhs={current_children['lhs']} rhs={current_children['rhs']})",
                        )
                    )
                    break

        if current_agent and current_children:
            agent_session_id = current_agent
            child_sessions = current_children
            agent_prompt_pack = current_prompt_pack
            agent_bridges = current_bridges
            break
        if attempt < attempts - 1:
            time.sleep(interval)

    if not agent_session_id:
        session_count = len(child_sessions)
        diff_queue.put(
            ("warn", f"autopair: no agent session discovered (children={session_count})")
        )
        return None

    if not child_sessions:
        diff_queue.put(("warn", "autopair: no paddle sessions discovered"))
        return None

    diff_queue.put(
        (
            "info",
            f"autopair discovered agent={agent_session_id} children={child_sessions}",
        )
    )

    session_roles = {
        agent_session_id: "agent",
        **{sid: role for role, sid in child_sessions.items()},
    }

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

    return AutopairContext(
        controller_session_id=agent_session_id,
        controller_token=lease.controller_token,
        child_sessions=child_sessions,
        session_roles=session_roles,
        lease_expires_at_ms=lease.expires_at_ms,
        prompt_pack=agent_prompt_pack,
        mcp_bridges=agent_bridges,
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
        on_conflict: Optional[Callable[[str], None]] = None,
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
        self._trace_ids: Dict[str, Optional[str]] = {}
        self._timeout = timeout
        self._target = target
        self._lock = threading.Lock()
        self._socket: Optional[socket.socket] = None
        self._log_path = log_path
        self._log_fp = None
        self._on_conflict = on_conflict
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

    def clear_session_token(self, session_id: str) -> None:
        if session_id in self._session_tokens:
            del self._session_tokens[session_id]

    def set_default_controller_token(self, token: Optional[str]) -> None:
        self._default_controller_token = token

    def current_session_token(self, session_id: str) -> Optional[str]:
        return self._session_tokens.get(session_id) or self._default_controller_token

    def set_trace_id(self, session_id: str, trace_id: Optional[str]) -> None:
        if trace_id:
            self._trace_ids[session_id] = trace_id
        elif session_id in self._trace_ids:
            del self._trace_ids[session_id]

    @property
    def http_enabled(self) -> bool:
        return bool(self._base_url)

    def queue_terminal_write(self, session_id: str, data: str) -> bool:
        command_id = str(uuid.uuid4())
        action_payload = {
            "id": command_id,
            "action_type": "terminal_write",
            "payload": {"bytes": data},
        }
        trace_id = self._trace_ids.get(session_id)
        if trace_id:
            action_payload["meta"] = {"trace_id": trace_id}
        transport_label = "log"
        status = "recorded"

        send_success = True
        if self._base_url:
            transport_label = "http"
            send_success = self._send_http(session_id, action_payload)
            status = "sent" if send_success else "error"

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
                "trace_id": trace_id,
            }
        )
        return send_success

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
        trace_id = self._trace_ids.get(session_id)
        if trace_id:
            headers["X-Trace-Id"] = trace_id

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
            if exc.code == 409 and self._on_conflict:
                self._on_conflict(session_id)
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

    def _detect_paddle(self) -> None:
        rows: List[int] = []
        cols: List[int] = []
        for row_idx, line in enumerate(self.lines):
            for col_idx, char in enumerate(line):
                if char in PADDLE_GLYPHS:
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
            for col_idx, char in enumerate(line):
                if char in BALL_GLYPHS:
                    found = (float(row_idx), float(col_idx))
                    break
            if found:
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
        stdscr: Optional["curses._CursesWindow"],
        session_roles: Dict[str, str],
        diff_queue: "queue.Queue[Tuple[str, object]]",
        mcp_client: MCPClient,
        serve_interval: float,
        serve_dx: Tuple[float, float],
        serve_dy: Tuple[float, float],
        max_step: float,
        min_threshold: float,
        command_interval: float,
        prompt_pack: Optional[Dict[str, object]] = None,
        mcp_bridges: Optional[List[Dict[str, object]]] = None,
        headless: bool = False,
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
        self.prompt_pack = prompt_pack or {}
        self.headless = headless

        self.sessions: Dict[str, SessionState] = {}
        self.logs: List[str] = []
        self.actions: List[Dict[str, object]] = []
        self.input_buffer: str = ""
        self.autopilot_enabled = True
        self.running = True
        self.last_spawn_time = 0.0
        self.last_draw_time = 0.0
        self.score = {"lhs": 0, "rhs": 0}
        self.prompt_lines: List[str] = []
        self.prompt_directives: Dict[str, object] = {}
        self.prompt_panel_visible = False
        self.bridge_defs = self._normalize_bridges(mcp_bridges)
        self.bridge_states: Dict[str, str] = {}
        self._bridge_endpoint_lookup = {
            entry["endpoint"]: entry["id"]
            for entry in self.bridge_defs
            if isinstance(entry.get("endpoint"), str)
        }
        self._serve_preference: str = "random"
        self._next_forced_side: Optional[str] = None
        self._last_serve_side: Optional[str] = None
        self._apply_prompt_pack()
        for bridge in self.bridge_defs:
            bridge_id = bridge["id"]
            state = "pending"
            endpoint = bridge.get("endpoint")
            if endpoint == "private_beach.queue_action" and not self.mcp.http_enabled:
                state = "disabled"
            self.bridge_states[bridge_id] = state

    # ------------------------------------------------------------------ Logging
    def log(self, message: str, level: str = "info") -> None:
        timestamp = time.strftime("%H:%M:%S", time.localtime())
        entry = f"[{timestamp}] {level.upper():<6} {message}"
        self.logs.append(entry)
        if len(self.logs) > LOG_LIMIT:
            self.logs = self.logs[-LOG_LIMIT:]
        if self.headless:
            print(entry, flush=True)

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
            trace_note = ""
            trace_value = event.get("trace_id")
            if isinstance(trace_value, str) and trace_value.strip():
                trace_note = f" trace={trace_value.strip()}"
            self.log(
                f"[{status}] {session_id} via {transport}: {command}{trace_note}",
                level="action",
            )
        else:
            message = event.get("message", "")
            self.log(str(message), level=level)

    # ----------------------------------------------------------- Prompt/Bridges
    def _normalize_bridges(
        self, entries: Optional[List[Dict[str, object]]]
    ) -> List[Dict[str, object]]:
        normalized: List[Dict[str, object]] = []
        if not entries:
            return normalized
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            bridge_id = str(entry.get("id") or entry.get("name") or "").strip()
            if not bridge_id:
                continue
            record: Dict[str, object] = {
                "id": bridge_id,
                "name": str(entry.get("name") or bridge_id),
            }
            endpoint = entry.get("endpoint")
            if isinstance(endpoint, str) and endpoint.strip():
                record["endpoint"] = endpoint.strip()
            normalized.append(record)
        return normalized

    def _apply_prompt_pack(self) -> None:
        instructions = self.prompt_pack.get("instructions")
        if isinstance(instructions, str) and instructions.strip():
            self.prompt_lines = [line.rstrip() for line in instructions.strip().splitlines()]
            if not self.headless:
                self.log("Prompt instructions synchronized.", level="info")
        options = self.prompt_pack.get("options")
        directives: Dict[str, object] = {}
        if isinstance(options, dict):
            autopilot_opts = options.get("autopilot")
            if isinstance(autopilot_opts, dict):
                directives = autopilot_opts
            else:
                directives = options
        self.prompt_directives = directives
        if directives:
            self._configure_from_prompt_options(directives)

    def _configure_from_prompt_options(self, directives: Dict[str, object]) -> None:
        serve_pref = directives.get("serve_preference")
        if isinstance(serve_pref, str):
            normalized = serve_pref.lower()
            if normalized in {"lhs", "rhs", "alternate", "random"}:
                self._serve_preference = normalized
        initial_serve = directives.get("initial_serve")
        if isinstance(initial_serve, str):
            lowered = initial_serve.lower()
            if lowered in {"lhs", "rhs"}:
                self._next_forced_side = lowered
        strategy = directives.get("paddle_strategy")
        if isinstance(strategy, str):
            self._apply_paddle_strategy(strategy.lower())
        serve_interval = directives.get("serve_interval")
        if isinstance(serve_interval, (int, float)) and serve_interval > 0:
            self.serve_interval = float(serve_interval)
        command_interval = directives.get("command_interval")
        if isinstance(command_interval, (int, float)) and command_interval > 0:
            self.command_interval = float(command_interval)
        max_step = directives.get("max_step")
        if isinstance(max_step, (int, float)) and max_step > 0:
            self.max_step = float(max_step)
        min_threshold = directives.get("min_threshold")
        if isinstance(min_threshold, (int, float)) and min_threshold > 0:
            self.min_threshold = float(min_threshold)
        autopilot_enabled = directives.get("autopilot_enabled")
        if isinstance(autopilot_enabled, bool):
            self.autopilot_enabled = autopilot_enabled

    def _apply_paddle_strategy(self, strategy: str) -> None:
        if strategy == "aggressive":
            self.max_step = max(self.max_step, self.max_step * 1.25)
            self.min_threshold = max(0.1, self.min_threshold * 0.6)
        elif strategy == "defensive":
            self.max_step = max(0.5, self.max_step * 0.75)
            self.min_threshold = min(1.0, self.min_threshold * 1.3)

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
        if not self.headless and self.stdscr:
            curses.curs_set(0)
            self.stdscr.nodelay(True)
        while self.running:
            now = time.monotonic()
            self._drain_incoming(now)
            if self.autopilot_enabled:
                self._autopilot_tick(now)
            if not self.headless:
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
                self._set_bridge_state_for_endpoint(
                    "private_beach.subscribe_state", "streaming"
                )
            elif kind == "info":
                self.log(str(payload), level="info")
            elif kind == "error":
                message = str(payload)
                if "state stream error" in message:
                    self._set_bridge_state_for_endpoint(
                        "private_beach.subscribe_state", "error"
                    )
                self.log(message, level="error")
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
        raw_payload = event.get("payload", {})
        if not isinstance(session_id, str) or not isinstance(raw_payload, dict):
            self.log(f"invalid diff structure: {event}", level="warn")
            return
        sequence = event.get("sequence")
        if not isinstance(sequence, int):
            seq_value = raw_payload.get("sequence")
            sequence = int(seq_value) if isinstance(seq_value, int) else 0

        payload = raw_payload
        if payload.get("type") is None and isinstance(payload.get("payload"), dict):
            payload = payload.get("payload", {})
        if not isinstance(payload, dict):
            self.log(f"invalid payload envelope: {event}", level="warn")
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
            target: Optional[SessionState] = None
            if self._next_forced_side:
                forced = self._select_session_by_side(candidates, self._next_forced_side)
                if forced:
                    target = forced
                self._next_forced_side = None
            if target is None:
                target = self._select_serve_target(candidates)
            if target is None:
                return
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

    def _select_session_by_side(
        self, candidates: List[SessionState], side: str
    ) -> Optional[SessionState]:
        for session in candidates:
            if session.side == side:
                self._last_serve_side = side
                return session
        return None

    def _select_serve_target(
        self, candidates: List[SessionState]
    ) -> Optional[SessionState]:
        if not candidates:
            return None
        if self._serve_preference in {"lhs", "rhs"}:
            match = self._select_session_by_side(candidates, self._serve_preference)
            if match:
                return match
        if self._serve_preference == "alternate" and self._last_serve_side in {"lhs", "rhs"}:
            next_side = "rhs" if self._last_serve_side == "lhs" else "lhs"
            match = self._select_session_by_side(candidates, next_side)
            if match:
                return match
        choice = random.choice(candidates)
        if choice.side in {"lhs", "rhs"}:
            self._last_serve_side = choice.side
        return choice

    def _build_status_line(self, width: int) -> str:
        parts: List[str] = []
        if self.bridge_defs:
            bridge_summary = " ".join(
                self._format_bridge_status(bridge) for bridge in self.bridge_defs[:3]
            )
            if bridge_summary:
                parts.append(f"Bridges: {bridge_summary}")
        if self.prompt_lines:
            toggle = "ON" if self.prompt_panel_visible else "OFF"
            parts.append(f"[P]rompt {toggle}")
        if not parts:
            return ""
        summary = "  ".join(parts)
        return summary[: max(width - 1, 0)]

    def _format_bridge_status(self, bridge: Dict[str, object]) -> str:
        bridge_id = bridge.get("id") or "bridge"
        name = bridge.get("name") or bridge_id
        status = self.bridge_states.get(str(bridge_id), "pending")
        return f"{name}[{status}]"

    def _draw_prompt_panel(self, start_y: int, width: int, max_height: int) -> int:
        if max_height <= 2:
            return 0
        lines = self.prompt_lines or []
        panel_height = min(max_height, len(lines) + 2)
        self._addstr_safe(start_y, 1, width - 2, " Prompt Pack ")
        available = panel_height - 1
        for idx, line in enumerate(lines[: available]):
            self._addstr_safe(start_y + 1 + idx, 2, width - 4, line)
        summary = self._prompt_directives_summary()
        if summary:
            footer_row = start_y + panel_height - 1
            self._addstr_safe(footer_row, 2, width - 4, f"Directives: {summary}")
        return panel_height

    def _prompt_directives_summary(self) -> str:
        if not self.prompt_directives:
            return ""
        parts: List[str] = []
        serve_pref = self.prompt_directives.get("serve_preference") or self.prompt_directives.get(
            "initial_serve"
        )
        if isinstance(serve_pref, str):
            parts.append(f"serve={serve_pref}")
        strategy = self.prompt_directives.get("paddle_strategy")
        if isinstance(strategy, str):
            parts.append(f"strategy={strategy}")
        return ", ".join(parts)

    def _set_bridge_state(self, bridge_id: str, state: str) -> None:
        if bridge_id not in self.bridge_states:
            return
        previous = self.bridge_states.get(bridge_id)
        if previous == state:
            return
        self.bridge_states[bridge_id] = state
        if self.headless:
            self.log(f"bridge {bridge_id} -> {state}", level="debug")

    def _set_bridge_state_for_endpoint(self, endpoint: Optional[str], state: str) -> None:
        if not endpoint:
            return
        bridge_id = self._bridge_endpoint_lookup.get(endpoint)
        if bridge_id:
            self._set_bridge_state(bridge_id, state)

    def _send_command(self, session: SessionState, command: str) -> None:
        payload = f"{command}\n"
        success = self.mcp.queue_terminal_write(session.session_id, payload)
        if success:
            self._set_bridge_state_for_endpoint("private_beach.queue_action", "sent")
        else:
            self._set_bridge_state_for_endpoint("private_beach.queue_action", "error")

    # -------------------------------------------------------------- Commands
    def _handle_keys(self) -> None:
        if self.headless or not self.stdscr:
            return
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
            if ch in (ord("p"), ord("P")):
                self.prompt_panel_visible = not self.prompt_panel_visible
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
        if not self.stdscr:
            self.last_draw_time = now
            return
        height, width = self.stdscr.getmaxyx()
        self.stdscr.erase()
        title = f" Private Beach Pong Agent — autopilot {'ON' if self.autopilot_enabled else 'OFF'} "
        self.stdscr.addstr(0, 0, title[: max(width - 1, 0)])
        score_text = f"LHS {self.score.get('lhs', 0)} | RHS {self.score.get('rhs', 0)}"
        self._addstr_safe(0, max(width - len(score_text) - 2, 0), width, score_text)
        status_line = self._build_status_line(width)
        status_y = 1
        if status_line:
            self._addstr_safe(status_y, 0, width, status_line)
        separator_y = status_y + 1
        if width > 0:
            self.stdscr.hline(separator_y, 0, curses.ACS_HLINE, width)
        log_top = separator_y + 1
        prompt_panel_space = max(height - (PROMPT_BOX_HEIGHT + 4), 3)
        panel_height = 0
        if self.prompt_panel_visible and self.prompt_lines and prompt_panel_space > 2:
            panel_height = self._draw_prompt_panel(log_top, width, prompt_panel_space)
            log_top += panel_height
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
        "--headless",
        action="store_true",
        help="Run without the curses UI (automation/test mode).",
    )
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
    state_subscribers: Dict[str, StateSubscriber] = {}
    state_pollers: Dict[str, StatePoller] = {}
    child_lease_renewers: Dict[str, Tuple[LeaseRenewer, threading.Event]] = {}

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

    def log_action_event(event: Dict[str, object]) -> None:
        diff_queue.put(("mcp", json.dumps(event)))

    default_token = args.default_controller_token

    mcp_client = MCPClient(
        action_callback=log_action_event,
        base_url=args.mcp_base_url,
        auth_token=args.mcp_token,
        default_controller_token=default_token,
        session_tokens=session_tokens,
        target=args.actions_target,
        log_path=args.action_log,
        on_conflict=lambda sid: schedule_autopair_refresh(f"queue_action 409 for {sid}"),
    )

    def start_state_stream(session_id: str, role: Optional[str] = None) -> None:
        if not manager_client or session_id in state_subscribers:
            return
        subscriber = StateSubscriber(
            manager_client,
            session_id,
            diff_queue,
            stop_event,
            role=role,
        )
        subscriber.start()
        state_subscribers[session_id] = subscriber

    def stop_state_stream(session_id: str) -> None:
        subscriber = state_subscribers.pop(session_id, None)
        if subscriber:
            subscriber.stop()
            subscriber.join(timeout=1.0)

    def stop_child_lease(session_id: str) -> None:
        renewer_entry = child_lease_renewers.pop(session_id, None)
        if renewer_entry:
            renewer, local_stop = renewer_entry
            local_stop.set()
            renewer.join(timeout=1.0)
            session_tokens.pop(session_id, None)
            mcp_client.clear_session_token(session_id)

    def ensure_child_lease(session_id: str, role: Optional[str], *, force: bool = False) -> None:
        if not manager_client:
            return
        if session_id in child_lease_renewers:
            if not force:
                return
            stop_child_lease(session_id)
        lease_reason = args.lease_reason or "pong_autopilot"
        reason = f"{lease_reason}:{role}" if role else lease_reason
        try:
            lease = manager_client.acquire_controller_lease(
                session_id, args.lease_ttl, reason
            )
        except ManagerRequestError as exc:
            diff_queue.put(
                (
                    "error",
                    f"failed to acquire controller lease for {session_id}: {exc}",
                )
            )
            return

        session_tokens[session_id] = lease.controller_token
        mcp_client.set_session_token(session_id, lease.controller_token)
        diff_queue.put(
            (
                "info",
                f"controller lease acquired for {session_id} (expires at {lease.expires_at_ms})",
            )
        )

        local_stop = threading.Event()

        def _update_token(updated: ControllerLease) -> None:
            session_tokens[session_id] = updated.controller_token
            mcp_client.set_session_token(session_id, updated.controller_token)

        renewer = LeaseRenewer(
            manager_client,
            session_id,
            args.lease_ttl,
            diff_queue,
            local_stop,
            _update_token,
            reason=reason,
        )
        renewer.start()
        child_lease_renewers[session_id] = (renewer, local_stop)

    def start_or_update_poller(session_id: str, interval: float) -> None:
        if not manager_client:
            return
        poller = state_pollers.get(session_id)
        if poller:
            poller.update_interval(interval)
            return
        poller = StatePoller(
            manager_client,
            session_id,
            diff_queue,
            stop_event,
            interval,
        )
        poller.start()
        state_pollers[session_id] = poller

    def stop_state_poller(session_id: str) -> None:
        poller = state_pollers.pop(session_id, None)
        if poller:
            poller.stop()
            poller.join(timeout=1.0)

    def cleanup_session(session_id: str) -> None:
        stop_state_poller(session_id)
        stop_child_lease(session_id)
        stop_state_stream(session_id)
        mcp_client.clear_session_token(session_id)
        mcp_client.set_trace_id(session_id, None)

    if manager_client:
        subscribe_roles = {
            session_id
            for session_id, role in session_role_map.items()
            if role in {"lhs", "rhs"}
        }
        for session_id in sorted(subscribe_roles):
            start_state_stream(session_id, session_role_map.get(session_id))
            ensure_child_lease(session_id, session_role_map.get(session_id))
    else:
        diff_queue.put(("warn", "manager client unavailable; skipping state streams"))

    renewer: Optional[LeaseRenewer] = None

    def _update_token(lease: ControllerLease) -> None:
        mcp_client.set_default_controller_token(lease.controller_token)

    pairing_thread: Optional[PairingSubscriber] = None

    def schedule_autopair_refresh(reason: str) -> None:
        if not (args.auto_pair and manager_client and autopair_ready.is_set()):
            return
        with autopair_refresh_lock:
            autopair_refresh_reasons.append(reason)
        autopair_refresh_event.set()

    def handle_pairing_event(child_session_id: str, action: str, metadata: Dict[str, object]) -> None:
        trace_value = metadata.get("trace_id")
        if isinstance(trace_value, str) and trace_value.strip():
            mcp_client.set_trace_id(child_session_id, trace_value.strip())
        else:
            mcp_client.set_trace_id(child_session_id, None)
        poll_freq = metadata.get("poll_frequency")
        if isinstance(poll_freq, (int, float)) and poll_freq > 0:
            start_or_update_poller(child_session_id, float(poll_freq))
        else:
            stop_state_poller(child_session_id)
        if action != "removed":
            start_state_stream(child_session_id, session_role_map.get(child_session_id))
            ensure_child_lease(child_session_id, session_role_map.get(child_session_id))
        else:
            stop_state_poller(child_session_id)
            stop_child_lease(child_session_id)
            stop_state_stream(child_session_id)

    autopair_ctx: Optional[AutopairContext] = None
    autopair_ready = threading.Event()
    autopair_managed_sessions: Set[str] = set()
    controller_lease_stop: Optional[threading.Event] = None
    autopair_refresh_event = threading.Event()
    autopair_refresh_reasons: Deque[str] = deque(maxlen=16)
    autopair_refresh_lock = threading.Lock()

    def apply_autopair_context(ctx: AutopairContext, *, force: bool = False) -> None:
        nonlocal autopair_ctx, renewer, pairing_thread, controller_lease_stop, autopair_managed_sessions
        previous_ctx = autopair_ctx
        previous_sessions = set(autopair_managed_sessions)
        autopair_ctx = ctx
        autopair_ready.set()
        autopair_managed_sessions = set(ctx.session_roles.keys())
        removed_sessions = previous_sessions - autopair_managed_sessions
        for session_id in removed_sessions:
            cleanup_session(session_id)
            session_role_map.pop(session_id, None)

        mcp_client.set_default_controller_token(None)
        mcp_client.set_session_token(ctx.controller_session_id, ctx.controller_token)
        session_role_map.update(ctx.session_roles)
        diff_queue.put(("info", f"session roles: {session_role_map}"))

        if manager_client:
            new_children = set(ctx.child_sessions.values())
            previous_children = (
                set(previous_ctx.child_sessions.values()) if previous_ctx else set()
            )
            for session_id in new_children:
                start_state_stream(session_id, session_role_map.get(session_id))
                ensure_child_lease(session_id, session_role_map.get(session_id), force=True)
            for session_id in sorted(new_children - previous_children):
                try:
                    snapshot = manager_client.fetch_state_snapshot(session_id)
                except ManagerRequestError as exc:
                    diff_queue.put(
                        ("warn", f"snapshot fetch failed for {session_id}: {exc}")
                    )
                else:
                    if snapshot:
                        diff_queue.put(
                            (
                                "diff",
                                {
                                    "session_id": session_id,
                                    "payload": snapshot,
                                    "received_at": time.time(),
                                },
                            )
                        )

            controller_changed = (
                not previous_ctx
                or previous_ctx.controller_session_id != ctx.controller_session_id
            )
            if controller_changed and controller_lease_stop:
                controller_lease_stop.set()
                controller_lease_stop = None
            if controller_changed and renewer:
                renewer.join(timeout=1.0)
                renewer = None

            if not controller_lease_stop:
                controller_lease_stop = threading.Event()

            if not renewer:
                renewer = LeaseRenewer(
                    manager_client,
                    ctx.controller_session_id,
                    args.lease_ttl,
                    diff_queue,
                    controller_lease_stop,
                    _update_token,
                    reason=args.lease_reason,
                )
                renewer.start()

            if args.private_beach_id:
                if controller_changed and pairing_thread:
                    pairing_thread.stop()
                    pairing_thread.join(timeout=1.0)
                    pairing_thread = None
                if not pairing_thread:
                    pairing_thread = PairingSubscriber(
                        manager_client,
                        ctx.controller_session_id,
                        args.private_beach_id,
                        diff_queue,
                        stop_event,
                        handle_pairing_event,
                    )
                    pairing_thread.start()

        if ctx.prompt_pack:
            instructions = ctx.prompt_pack.get("instructions")
            if isinstance(instructions, str) and instructions.strip():
                diff_queue.put(("info", "agent prompt synchronized from onboarding"))

    def run_autopair_attempt(force: bool = False) -> bool:
        if not (args.auto_pair and manager_client and args.private_beach_id):
            return False
        if autopair_ready.is_set() and not force:
            return False
        ctx = autopair_sessions(args, manager_client, diff_queue)
        if ctx:
            apply_autopair_context(ctx, force=force)
            return True
        return False

    if not run_autopair_attempt():
        if args.auto_pair and manager_client and args.private_beach_id:
            diff_queue.put(
                (
                    "warn",
                    "autopair pending; waiting for tiles to attach before taking control",
                )
            )

            def autopair_retry_loop() -> None:
                delay = max(args.discovery_interval, 1.0)
                while not stop_event.wait(delay):
                    if run_autopair_attempt():
                        break

            threading.Thread(target=autopair_retry_loop, daemon=True).start()
        else:
            diff_queue.put(("info", f"session roles: {session_role_map}"))

    autopair_refresh_thread: Optional[threading.Thread] = None

    if args.auto_pair and manager_client and args.private_beach_id:

        def autopair_refresh_loop() -> None:
            backoff = 2.0
            while not stop_event.is_set():
                autopair_refresh_event.wait()
                autopair_refresh_event.clear()
                if stop_event.is_set():
                    break
                with autopair_refresh_lock:
                    reasons = list(autopair_refresh_reasons)
                    autopair_refresh_reasons.clear()
                reason_msg = reasons[-1] if reasons else "unknown trigger"
                diff_queue.put(
                    ("warn", f"autopair refresh triggered ({reason_msg})")
                )
                if run_autopair_attempt(force=True):
                    diff_queue.put(("info", "autopair refresh complete"))
                    backoff = 2.0
                else:
                    diff_queue.put(
                        ("warn", "autopair refresh failed; retrying shortly")
                    )
                    if stop_event.wait(backoff):
                        break
                    autopair_refresh_event.set()
                    backoff = min(backoff * 2, 30.0)

        autopair_refresh_thread = threading.Thread(
            target=autopair_refresh_loop, daemon=True, name="autopair-refresh"
        )
        autopair_refresh_thread.start()

    def run_app(stdscr: Optional["curses._CursesWindow"]) -> None:
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
            prompt_pack=getattr(autopair_ctx, "prompt_pack", None) if autopair_ctx else None,
            mcp_bridges=getattr(autopair_ctx, "mcp_bridges", None) if autopair_ctx else None,
            headless=args.headless,
        )
        try:
            app.log("agent ready", level="info")
            app.run()
        finally:
            pass

    try:
        if args.headless:
            run_app(None)
        else:
            curses.wrapper(run_app)
    finally:
        stop_event.set()
        for subscriber in list(state_subscribers.values()):
            subscriber.stop()
            subscriber.join(timeout=1.0)
        for poller in state_pollers.values():
            poller.stop()
            poller.join(timeout=1.0)
        for session_id in list(child_lease_renewers.keys()):
            stop_child_lease(session_id)
        if renewer:
            if controller_lease_stop:
                controller_lease_stop.set()
            renewer.join(timeout=1.0)
        if pairing_thread:
            pairing_thread.stop()
            pairing_thread.join(timeout=1.0)
        if autopair_refresh_thread:
            autopair_refresh_event.set()
            autopair_refresh_thread.join(timeout=1.0)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
