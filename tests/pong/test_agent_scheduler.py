"""Unit tests for the demo Pong agent command scheduler."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = (
    Path(__file__).resolve().parents[2]
    / "apps/private-beach/demo/pong/agent/main.py"
)
SPEC = importlib.util.spec_from_file_location("pong_agent_main", MODULE_PATH)
if SPEC is None or SPEC.loader is None:  # pragma: no cover - import guard
    raise RuntimeError("unable to load pong agent module for testing")
PONG_AGENT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PONG_AGENT
SPEC.loader.exec_module(PONG_AGENT)

CommandScheduler = PONG_AGENT.CommandScheduler
RunState = PONG_AGENT.CommandScheduler.RunState
CommandDispatchResult = PONG_AGENT.CommandDispatchResult
SessionState = PONG_AGENT.SessionState
normalize_transport_status = PONG_AGENT.normalize_transport_status


class CommandSchedulerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.logs = []

        def _log(message: str, level: str = "info") -> None:
            self.logs.append((level, message))

        self.scheduler = CommandScheduler(
            log_func=_log,
            enabled=True,
            per_session_rate=2,
            readiness_timeout=2.0,
            wait_log_interval=0.01,
        )
        self.lhs = SessionState("lhs-session", side="lhs")
        self.rhs = SessionState("rhs-session", side="rhs")

    def _mark_ready(
        self,
        session: SessionState,
        now: float,
        *,
        lease: bool = True,
        transport: bool = True,
    ) -> None:
        session.lines = ["#" * 5 for _ in range(10)]
        session.last_update = now
        session.lease_active = lease
        session.transport_ready = transport
        session.transport_status = "fast_path" if transport else "pending"

    def test_waits_until_both_players_ready(self) -> None:
        now = 10.0
        self._mark_ready(self.lhs, now)
        ready = self.scheduler.update_player_readiness([self.lhs], now)
        self.assertFalse(ready)
        self.assertFalse(self.scheduler.allow_command(self.lhs, now))

        self._mark_ready(self.rhs, now)
        ready = self.scheduler.update_player_readiness([self.lhs, self.rhs], now)
        self.assertTrue(ready)
        self.assertTrue(self.scheduler.allow_command(self.lhs, now + 0.01))
        self.assertEqual(self.scheduler.state, RunState.RUNNING)

    def test_rate_limit_caps_commands_per_session(self) -> None:
        now = 20.0
        for session in (self.lhs, self.rhs):
            self._mark_ready(session, now)
        self.scheduler.update_player_readiness([self.lhs, self.rhs], now)

        self.assertTrue(self.scheduler.allow_command(self.lhs, now))
        self.assertTrue(self.scheduler.allow_command(self.lhs, now + 0.01))
        self.assertFalse(self.scheduler.allow_command(self.lhs, now + 0.02))
        # After one second passes the budget resets.
        self.assertTrue(self.scheduler.allow_command(self.lhs, now + 1.5))

    def test_handle_result_applies_backoff(self) -> None:
        now = 30.0
        result = CommandDispatchResult(False, "throttled", status_code=429)
        self.scheduler.handle_result(self.lhs, result, now)
        delay = self.lhs.action_backoff_until - now
        self.assertGreaterEqual(delay, 1.5)
        self.assertGreater(self.lhs.action_failures, 0)

        # Success should clear the backoff tracking.
        success = CommandDispatchResult(True, "sent")
        self.scheduler.handle_result(self.lhs, success, now + 5.0)
        self.assertEqual(self.lhs.action_failures, 0)
        self.assertEqual(self.lhs.action_backoff_until, 0.0)

    def test_requires_transport_and_lease_signals(self) -> None:
        now = 40.0
        self._mark_ready(self.lhs, now, lease=False)
        self._mark_ready(self.rhs, now, transport=False)
        ready = self.scheduler.update_player_readiness([self.lhs, self.rhs], now)
        self.assertFalse(ready)
        summary_logs = "\n".join(message for _, message in self.logs)
        self.assertIn("lease", summary_logs)
        self.assertIn("transport", summary_logs)

    def test_normalizes_transport_status_variants(self) -> None:
        self.assertEqual(normalize_transport_status("FastPath"), "fast_path")
        self.assertEqual(normalize_transport_status("httpfallback"), "http_fallback")
        self.assertEqual(normalize_transport_status("HTTP"), "http_poller")
        self.assertEqual(normalize_transport_status("pb-controller"), "fast_path")
        self.assertEqual(normalize_transport_status(None), "pending")
        self.assertEqual(self.scheduler.state, RunState.WAITING)

    def test_pause_and_resume_on_throttle(self) -> None:
        now = 50.0
        for session in (self.lhs, self.rhs):
            self._mark_ready(session, now)
        self.scheduler.update_player_readiness([self.lhs, self.rhs], now)
        self.assertEqual(self.scheduler.state, RunState.RUNNING)

        throttle = CommandDispatchResult(False, "throttled", status_code=429)
        self.scheduler.handle_result(self.lhs, throttle, now)
        self.assertEqual(self.scheduler.state, RunState.PAUSED)
        self.assertFalse(self.scheduler.allow_command(self.lhs, now + 0.1))

        later = now + 2.0
        success = CommandDispatchResult(True, "sent")
        self.scheduler.handle_result(self.lhs, success, later)
        self.assertEqual(self.scheduler.state, RunState.RUNNING)
        self.assertTrue(self.scheduler.allow_command(self.lhs, later + 0.5))


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
