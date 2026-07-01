#!/usr/bin/env python3
"""Self-checks for spawn-report-verify.py cycle classification."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
VERIFY_PATH = SCRIPT_DIR / "spawn-report-verify.py"

spec = importlib.util.spec_from_file_location("spawn_report_verify", VERIFY_PATH)
if spec is None or spec.loader is None:
    raise RuntimeError(f"could not load verifier module from {VERIFY_PATH}")
verify = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = verify
spec.loader.exec_module(verify)


def make_turn(
    index: int,
    started_at: float,
    completed_at: float | None = None,
    trigger_prompts: list[str] | None = None,
) -> verify.Turn:
    return verify.Turn(
        index=index,
        turn_id=f"turn-{index}",
        started_at=started_at,
        completed_at=completed_at,
        trigger_prompts=trigger_prompts or [],
    )


def make_pane(
    thread_id: str,
    role: str,
    parent_thread_id: str | None = None,
    turns: list[verify.Turn] | None = None,
) -> verify.Pane:
    return verify.Pane(
        thread_id=thread_id,
        role=role,
        nickname=role,
        model="test-model",
        rollout_path=Path("/dev/null"),
        parent_thread_id=parent_thread_id,
        turns=turns or [],
    )


class SpawnReportVerifyTests(unittest.TestCase):
    def analyze_one_child_cycle(self, parent_turns: list[verify.Turn]) -> dict:
        root = make_pane("nazgul-thread", "nazgul", turns=parent_turns)
        child = make_pane(
            "troll-thread",
            "troll",
            parent_thread_id=root.thread_id,
            turns=[make_turn(0, started_at=1.0, completed_at=10.0)],
        )
        report = verify.analyze({root.thread_id: root, child.thread_id: child}, root)
        cycles = report["Q1_report_became_turn"]["cycles"]
        self.assertEqual(1, len(cycles))
        return cycles[0]

    def test_no_following_parent_turn_does_not_become_report_turn(self) -> None:
        cycle = self.analyze_one_child_cycle(
            [
                make_turn(
                    1,
                    started_at=1.0,
                    completed_at=2.0,
                    trigger_prompts=[
                        "A child pane has reported back. Review an earlier child report."
                    ],
                )
            ]
        )
        self.assertFalse(cycle["report_became_turn"])
        self.assertIsNone(cycle["parent_report_turn_id"])
        self.assertFalse(cycle["parent_acted"])

    def test_following_report_prompt_becomes_report_turn(self) -> None:
        cycle = self.analyze_one_child_cycle(
            [
                make_turn(
                    1,
                    started_at=10.1,
                    completed_at=20.0,
                    trigger_prompts=[
                        "A child pane has reported back. Review the child report below."
                    ],
                )
            ]
        )
        self.assertTrue(cycle["report_became_turn"])
        self.assertEqual("turn-1", cycle["parent_report_turn_id"])

    def test_following_non_report_prompt_does_not_become_report_turn(self) -> None:
        cycle = self.analyze_one_child_cycle(
            [
                make_turn(
                    1,
                    started_at=10.1,
                    completed_at=20.0,
                    trigger_prompts=["Continue the prior implementation task."],
                )
            ]
        )
        self.assertFalse(cycle["report_became_turn"])
        self.assertEqual("turn-1", cycle["parent_report_turn_id"])


if __name__ == "__main__":
    unittest.main()
