"""Tests for ball tracking and loss detection in the Pong agent."""

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

SessionState = PONG_AGENT.SessionState
BALL_LOSS_GRACE_SECONDS = PONG_AGENT.BALL_LOSS_GRACE_SECONDS


class BallTrackingTests(unittest.TestCase):
    def test_ball_exit_requires_sustained_absence(self) -> None:
        session = SessionState("s", side="lhs")
        now = 10.0
        lines_with_ball = [
            "     ",
            "  ●  ",
            "     ",
            "     ",
            "     ",
        ]
        session.apply_terminal_frame(lines_with_ball, None, sequence=1, now=now)
        self.assertIsNotNone(session.ball_position)

        # One missing sample inside the grace window should not mark an exit.
        session.apply_terminal_frame(
            ["     " for _ in lines_with_ball],
            None,
            sequence=2,
            now=now + BALL_LOSS_GRACE_SECONDS / 2,
        )
        self.assertIsNone(session.ball_exit)
        self.assertIsNotNone(session.ball_position)

        # After the grace period, a continued absence should trigger an exit.
        session.apply_terminal_frame(
            ["     " for _ in lines_with_ball],
            None,
            sequence=3,
            now=now + BALL_LOSS_GRACE_SECONDS + 1.0,
        )
        self.assertEqual(session.ball_exit, "miss")
        self.assertIsNone(session.ball_position)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
