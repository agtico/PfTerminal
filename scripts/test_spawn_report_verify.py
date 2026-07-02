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
    last_agent_message: str | None = None,
) -> verify.Turn:
    return verify.Turn(
        index=index,
        turn_id=f"turn-{index}",
        started_at=started_at,
        completed_at=completed_at,
        trigger_prompts=trigger_prompts or [],
        last_agent_message=last_agent_message,
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

    def test_wrapped_spawn_context_report_prompt_becomes_report_turn(self) -> None:
        report_prompt = (
            "<pfterminal_spawn_troll_task_context>\n"
            "Recent child reports delivered to this pane:\n"
            "- Snaga [orc]; status=done; result=Wrote artifact.\n"
            "</pfterminal_spawn_troll_task_context>\n\n"
            "Task from Sauron/Nazgul:\n"
            "A child pane has reported back. Review the child report below and act "
            "on it immediately.\n\n"
            "Snaga [orc]; status=done; result=Wrote artifact."
        )
        cycle = self.analyze_one_child_cycle(
            [
                make_turn(
                    1,
                    started_at=10.1,
                    completed_at=20.0,
                    trigger_prompts=[report_prompt],
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

    def test_busy_parent_turn_does_not_mask_later_report_turn(self) -> None:
        cycle = self.analyze_one_child_cycle(
            [
                make_turn(
                    1,
                    started_at=9.5,
                    completed_at=20.0,
                    trigger_prompts=["Continue the prior implementation task."],
                ),
                make_turn(
                    2,
                    started_at=20.1,
                    completed_at=21.0,
                    trigger_prompts=[
                        "A child pane has reported back. Review the child report below."
                    ],
                ),
            ]
        )
        self.assertTrue(cycle["parent_busy_at_delivery"])
        self.assertTrue(cycle["report_became_turn"])
        self.assertEqual("turn-2", cycle["parent_report_turn_id"])

    def test_wrapped_report_prompt_counts_for_busy_parent_race(self) -> None:
        wrapped_report_prompt = (
            "<pfterminal_spawn_troll_task_context>\n"
            "Recent child reports delivered to this pane:\n"
            "- Snaga [orc]; status=done; result=Updated artifact.\n"
            "</pfterminal_spawn_troll_task_context>\n\n"
            "Task from Sauron/Nazgul:\n"
            "A child pane has reported back. Review the child report below and act "
            "on it immediately.\n\n"
            "Snaga [orc]; status=done; result=Updated artifact."
        )
        root = make_pane(
            "nazgul-thread",
            "nazgul",
            turns=[
                make_turn(
                    1,
                    started_at=9.5,
                    completed_at=20.0,
                    trigger_prompts=[wrapped_report_prompt],
                )
            ],
        )
        child = make_pane(
            "troll-thread",
            "troll",
            parent_thread_id=root.thread_id,
            turns=[make_turn(0, started_at=1.0, completed_at=10.0)],
        )

        report = verify.analyze({root.thread_id: root, child.thread_id: child}, root)

        cycle = report["Q1_report_became_turn"]["cycles"][0]
        self.assertTrue(cycle["parent_busy_at_delivery"])
        self.assertTrue(cycle["report_became_turn"])
        self.assertTrue(report["Q3_mid_turn_race"]["pass"])

    def test_host_dispatch_block_counts_as_rework_dispatch(self) -> None:
        root = make_pane(
            "nazgul-thread",
            "nazgul",
            turns=[
                make_turn(
                    0,
                    started_at=1.0,
                    completed_at=2.0,
                    trigger_prompts=[
                        "A child pane has reported back. Review the child report below."
                    ],
                    last_agent_message=(
                        '<pfterminal_send_task target="Burzum">\n'
                        "Rework the artifact.\n"
                        "</pfterminal_send_task>"
                    ),
                )
            ],
        )
        troll = make_pane(
            "troll-thread",
            "troll",
            parent_thread_id=root.thread_id,
            turns=[make_turn(1, started_at=0.0, completed_at=0.5)],
        )
        report = verify.analyze({root.thread_id: root, troll.thread_id: troll}, root)
        self.assertEqual(1, report["Q2_manager_acted"]["rework_dispatches"])

    def test_q1_fails_with_note_for_zero_turn_spawn_tree(self) -> None:
        root = make_pane("nazgul-thread", "nazgul")
        troll = make_pane("troll-thread", "troll", parent_thread_id=root.thread_id)
        native_orc = make_pane(
            "native-orc-thread",
            "orc",
            parent_thread_id=troll.thread_id,
        )
        claude_orc = make_pane(
            "claude-orc-thread",
            "orc",
            parent_thread_id=troll.thread_id,
        )

        report = verify.analyze(
            {
                root.thread_id: root,
                troll.thread_id: troll,
                native_orc.thread_id: native_orc,
                claude_orc.thread_id: claude_orc,
            },
            root,
        )

        q1 = report["Q1_report_became_turn"]
        self.assertFalse(q1["pass"])
        self.assertEqual([], q1["cycles"])
        self.assertIn("no child completion/report cycles observed", q1["note"])
        self.assertFalse(report["green"])


if __name__ == "__main__":
    unittest.main()
