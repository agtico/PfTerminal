#!/usr/bin/env python3
"""
PFTerminal spawn report-delivery verifier.

Implements the verification plan from
  PfTerminal/codex-rs/tui/tests/spawn_report_delivery_scope.md

It reads a completed `/spawn` run (all-Codex Nazgul -> Troll -> 2x Orc) from the
PFTerminal state DB + per-thread rollout JSONL transcripts and emits a
machine-readable report answering:

  Q1. Did every child->parent report trigger a REAL turn on the parent?
  Q2. Did managers actually ACT on reports (triage/dispatch/acknowledge)?
  Q3. Did it hold across the mid-turn race (reports to a busy parent)?

Usage:
  spawn-report-verify.py [--codex-home DIR] [--root THREAD_ID] [--out JSON] [--md MD]

Defaults: --codex-home = $PFTERMINAL_HOME or ~/.pfterminal
The run is identified either by --root (the Nazgul thread id) or, if omitted, by the
most recently updated thread whose agent_role is nazgul (or the primary user thread).

Exit code: 0 = green run (A1-A8 product pass AND Q1-Q3 pass), 1 = report-delivery
regression or product incomplete, 2 = could not identify a run.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
from collections import OrderedDict, defaultdict
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Optional

REPORT_PROMPT_PREFIX = "A child pane has reported back"
MULTI_REPORT_PROMPT_PREFIX = "Multiple child panes have reported back"
DISPATCH_TOOL_NAMES = {"spawn_agent", "send_input", "followup_task", "send_message"}
SEND_TASK_BLOCK_MARKER = "<pfterminal_send_task"
# A parent turn that follows a report is considered "acted on" if it ends with at
# least one of: a dispatch tool call, an exec/read tool call (audit), or an assistant
# message longer than a no-op acknowledgement.
ACT_TOOL_NAMES = DISPATCH_TOOL_NAMES | {
    "exec_command",
    "shell_command",
    "read_mcp_resource",
}
NOOP_ACK_MAX_LEN = (
    120  # an "acknowledged / will wait" style message shorter than this is a drop
)


@dataclass
class Turn:
    index: int
    turn_id: str
    started_at: float
    completed_at: Optional[float] = None
    last_agent_message: Optional[str] = None
    # tool call names made during this turn, in order
    tool_calls: list[str] = field(default_factory=list)
    # user/trigger prompts that started this turn (text)
    trigger_prompts: list[str] = field(default_factory=list)


@dataclass
class Pane:
    thread_id: str
    role: Optional[str]
    nickname: Optional[str]
    model: Optional[str]
    rollout_path: Path
    parent_thread_id: Optional[str]
    turns: list[Turn] = field(default_factory=list)


def codex_home_default() -> Path:
    return Path(
        os.environ.get("PFTERMINAL_HOME") or os.path.expanduser("~/.pfterminal")
    )


def open_state_db(codex_home: Path) -> sqlite3.Connection:
    # state_<n>.sqlite — pick the highest-numbered one that has the spawn tables.
    candidates = sorted(codex_home.glob("state_*.sqlite"), reverse=True)
    for db in candidates:
        try:
            con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
            cur = con.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name IN "
                "('threads','thread_spawn_edges')"
            )
            rows = {r[0] for r in cur.fetchall()}
            if {"threads", "thread_spawn_edges"} <= rows:
                return con
        except sqlite3.DatabaseError:
            continue
    raise SystemExit(
        f"could not find a state DB with threads+thread_spawn_edges in {codex_home}"
    )


def load_panes(con: sqlite3.Connection) -> dict[str, Pane]:
    con.row_factory = sqlite3.Row
    panes: dict[str, Pane] = {}
    for row in con.execute(
        "SELECT id, rollout_path, agent_role, agent_nickname, model FROM threads"
    ):
        panes[row["id"]] = Pane(
            thread_id=row["id"],
            role=row["agent_role"],
            nickname=row["agent_nickname"],
            model=row["model"],
            rollout_path=Path(row["rollout_path"]),
            parent_thread_id=None,
        )
    for row in con.execute(
        "SELECT parent_thread_id, child_thread_id FROM thread_spawn_edges"
    ):
        child = panes.get(row["child_thread_id"])
        if child is not None:
            child.parent_thread_id = row["parent_thread_id"]
    return panes


def parse_rollout(pane: Pane) -> None:
    """Populate pane.turns from its rollout JSONL."""
    path = pane.rollout_path
    if not path.is_file():
        return
    turns: dict[str, Turn] = OrderedDict()
    order: list[str] = []
    current_turn_id: Optional[str] = None
    with path.open("r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            typ = obj.get("type")
            payload = obj.get("payload") or {}
            ptype = payload.get("type") if isinstance(payload, dict) else None

            if typ == "event_msg" and ptype == "task_started":
                tid = payload.get("turn_id")
                if tid and tid not in turns:
                    turns[tid] = Turn(
                        index=len(turns),
                        turn_id=tid,
                        started_at=float(payload.get("started_at") or 0.0),
                    )
                    order.append(tid)
                current_turn_id = tid
            elif typ == "event_msg" and ptype == "task_complete":
                tid = payload.get("turn_id")
                turn = turns.get(tid)
                if turn is not None:
                    turn.completed_at = float(payload.get("completed_at") or 0.0)
                    lam = payload.get("last_agent_message")
                    if isinstance(lam, str):
                        turn.last_agent_message = lam
            elif typ == "event_msg" and ptype == "user_message":
                tid = current_turn_id
                if tid and tid in turns:
                    msg = payload.get("message")
                    if isinstance(msg, str):
                        turns[tid].trigger_prompts.append(msg)
            elif typ == "response_item" and ptype == "function_call":
                tid = current_turn_id
                if tid and tid in turns:
                    name = payload.get("name")
                    if isinstance(name, str):
                        turns[tid].tool_calls.append(name)
    pane.turns = [turns[t] for t in order]


def find_run_root(panes: dict[str, Pane], requested: Optional[str]) -> Optional[Pane]:
    if requested:
        return panes.get(requested)
    # Prefer a thread whose agent_role is nazgul; else the most recently updated user thread.
    nazguls = [p for p in panes.values() if (p.role or "").lower() == "nazgul"]
    if nazguls:
        return max(
            nazguls,
            key=lambda p: (
                p.rollout_path.stat().st_mtime if p.rollout_path.is_file() else 0
            ),
        )
    return None


def children_of(panes: dict[str, Pane], parent_id: str) -> list[Pane]:
    return [p for p in panes.values() if p.parent_thread_id == parent_id]


def first_report_turn_after(pane: Pane, when: float) -> Optional[Turn]:
    """First report-processing turn after `when`, falling back to the first candidate turn.

    A child can complete while its parent is already busy. In that race, the busy parent turn may
    have started inside the timestamp grace window but it is not the queued report-processing turn.
    Keep scanning for the report prompt so Q3 verifies the actual flush instead of the pre-existing
    busy turn.
    """
    first_candidate: Optional[Turn] = None
    for t in pane.turns:
        if t.started_at >= when - 1.0:
            if first_candidate is None:
                first_candidate = t
            if is_report_processing_turn(t):
                return t
    return first_candidate


def is_report_processing_turn(turn: Turn) -> bool:
    return any(
        p.startswith(REPORT_PROMPT_PREFIX) or p.startswith(MULTI_REPORT_PROMPT_PREFIX)
        for p in turn.trigger_prompts
    )


def turn_acted(turn: Turn) -> bool:
    if any(tc in ACT_TOOL_NAMES for tc in turn.tool_calls) or turn_has_dispatch(turn):
        return True
    lam = (turn.last_agent_message or "").strip()
    if lam and len(lam) > NOOP_ACK_MAX_LEN:
        return True
    return False


def turn_has_dispatch(turn: Turn) -> bool:
    if any(tc in DISPATCH_TOOL_NAMES for tc in turn.tool_calls):
        return True
    return SEND_TASK_BLOCK_MARKER in (turn.last_agent_message or "")


def analyze(panes: dict[str, Pane], root: Pane) -> dict:
    nazgul = root
    trolls = children_of(panes, nazgul.thread_id)
    troll_ids = {t.thread_id for t in trolls}
    # Orcs are children of Trolls. Only fall back to Nazgul's direct children if they are not
    # already counted as Trolls, so a pane is never double-listed as both Troll and Orc.
    orcs: list[Pane] = []
    for t in trolls:
        orcs.extend(children_of(panes, t.thread_id))
    for c in children_of(panes, nazgul.thread_id):
        if c.thread_id not in troll_ids:
            orcs.append(c)

    # ---- Q1: every child completion -> a parent report-processing turn ----
    q1 = {"pass": True, "cycles": []}
    for child in orcs + trolls:
        parent_id = child.parent_thread_id
        parent = panes.get(parent_id) if parent_id else None
        if parent is None:
            continue
        # The Troll's parent is the Nazgul; an Orc's parent is a Troll.
        for ct in child.turns:
            if ct.completed_at is None:
                continue
            parent_turn = first_report_turn_after(parent, ct.completed_at)
            # Allow a small grace: the report turn may start a hair before the
            # child's recorded completion if timestamps are coarse. Match by prompt.
            became_turn = parent_turn is not None and is_report_processing_turn(
                parent_turn
            )
            # Was the parent busy when the child completed? (mid-turn race)
            parent_busy_at_delivery = any(
                pt.started_at <= ct.completed_at
                and (pt.completed_at is None or pt.completed_at >= ct.completed_at)
                for pt in parent.turns
            )
            acted = parent_turn is not None and turn_acted(parent_turn)
            q1["cycles"].append(
                {
                    "child": _label(child),
                    "parent": _label(parent),
                    "child_turn_id": ct.turn_id,
                    "child_completed_at": ct.completed_at,
                    "parent_report_turn_id": parent_turn.turn_id
                    if parent_turn
                    else None,
                    "report_became_turn": became_turn,
                    "parent_busy_at_delivery": parent_busy_at_delivery,
                    "parent_acted": acted,
                }
            )
            if not became_turn:
                q1["pass"] = False

    # ---- Q2: managers act on reports (>=1 forced rework loop, Nazgul audits code) ----
    rework_loops = 0
    for parent in trolls + [nazgul]:
        for t in parent.turns:
            if is_report_processing_turn(t) and turn_has_dispatch(t):
                rework_loops += 1
    nazgul_audited = any(
        any(
            tc in {"exec_command", "shell_command", "read_mcp_resource"}
            for tc in t.tool_calls
        )
        for t in nazgul.turns
    )
    q2 = {
        "pass": rework_loops >= 1 and nazgul_audited,
        "rework_dispatches": rework_loops,
        "nazgul_audited_code": nazgul_audited,
    }

    # ---- Q3: mid-turn race — reports to a busy parent still get a turn ----
    race_cycles = [c for c in q1["cycles"] if c["parent_busy_at_delivery"]]
    q3 = {
        "pass": all(c["report_became_turn"] for c in race_cycles)
        if race_cycles
        else False,
        "race_cycle_count": len(race_cycles),
        "all_flushed": all(c["report_became_turn"] for c in race_cycles),
    }
    # A run must actually exercise the race to count as a real multi-turn test.
    if not race_cycles:
        q3["pass"] = False
        q3["note"] = "no mid-turn-race cycle observed; run was not multi-turn enough"

    # ---- product (A1-A8) presence check via rollouts is heuristic: we only verify the
    # run reached a Nazgul sign-off (ACCEPTANCE.md mentioned in a Nazgul turn) ----
    nazgul_signoff = any(
        "ACCEPTANCE" in (t.last_agent_message or "") for t in nazgul.turns
    ) or any("ACCEPTANCE" in p for t in nazgul.turns for p in t.trigger_prompts)
    total_turns = sum(len(p.turns) for p in [nazgul, *trolls, *orcs])
    product = {
        "pass": nazgul_signoff and total_turns >= 6,
        "total_turns": total_turns,
        "nazgul_signoff": nazgul_signoff,
        "nazgul": _label(nazgul),
        "trolls": [_label(t) for t in trolls],
        "orcs": [_label(o) for o in orcs],
    }

    green = product["pass"] and q1["pass"] and q2["pass"] and q3["pass"]
    return {
        "green": green,
        "product": product,
        "Q1_report_became_turn": q1,
        "Q2_manager_acted": q2,
        "Q3_mid_turn_race": q3,
    }


def _label(p: Pane) -> dict:
    return {
        "thread_id": p.thread_id,
        "role": p.role,
        "nickname": p.nickname,
        "model": p.model,
        "turns": len(p.turns),
    }


def render_md(report: dict) -> str:
    out = ["# Spawn Report-Delivery Verification Report\n"]
    out.append(f"**Overall: {'GREEN' if report['green'] else 'RED'}**\n")
    p = report["product"]
    out.append("## Run\n")
    out.append(f"- Nazgul: {_md_label(p['nazgul'])}")
    for t in p["trolls"]:
        out.append(f"- Troll: {_md_label(t)}")
    for o in p["orcs"]:
        out.append(f"- Orc: {_md_label(o)}")
    out.append(f"- Total turns: {p['total_turns']}")
    out.append(f"- Nazgul sign-off (ACCEPTANCE.md): {p['nazgul_signoff']}\n")

    q1 = report["Q1_report_became_turn"]
    out.append("## Q1 — Did every child→parent report trigger a real turn?")
    out.append(f"**Pass: {q1['pass']}**\n")
    out.append(
        "| child | parent | child turn | parent report turn | became turn | parent busy | acted |"
    )
    out.append("|---|---|---|---|---|---|---|")
    for c in q1["cycles"]:
        out.append(
            f"| {c['child']['nickname'] or c['child']['role']} "
            f"| {c['parent']['nickname'] or c['parent']['role']} "
            f"| `{c['child_turn_id'][-8:]}` "
            f"| `{(c['parent_report_turn_id'] or '-')[-8:]}` "
            f"| {'✅' if c['report_became_turn'] else '❌'} "
            f"| {'busy' if c['parent_busy_at_delivery'] else 'idle'} "
            f"| {'✅' if c['parent_acted'] else '❌'} |"
        )
    out.append("")

    q2 = report["Q2_manager_acted"]
    out.append("## Q2 — Did managers act on reports?")
    out.append(
        f"**Pass: {q2['pass']}** (rework dispatches={q2['rework_dispatches']}, "
        f"nazgul audited code={q2['nazgul_audited_code']})\n"
    )

    q3 = report["Q3_mid_turn_race"]
    out.append("## Q3 — Mid-turn race held?")
    out.append(
        f"**Pass: {q3['pass']}** (race cycles={q3['race_cycle_count']}, "
        f"all flushed={q3['all_flushed']})"
    )
    if q3.get("note"):
        out.append(f"_Note: {q3['note']}_")
    return "\n".join(out) + "\n"


def _md_label(d: dict) -> str:
    name = d.get("nickname") or d.get("role") or "pane"
    return f"`{name}` ({d.get('model') or '?'}, {d['turns']} turns, `{d['thread_id'][-8:]}`)"


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(
        description="PFTerminal spawn report-delivery verifier"
    )
    ap.add_argument("--codex-home", default=str(codex_home_default()))
    ap.add_argument("--root", help="Nazgul/root thread id of the run to analyze")
    ap.add_argument("--out", help="Write machine-readable JSON report to this path")
    ap.add_argument("--md", help="Write human-readable markdown report to this path")
    args = ap.parse_args()

    codex_home = Path(args.codex_home)
    if not codex_home.is_dir():
        print(f"codex-home not found: {codex_home}", file=sys.stderr)
        return 2
    try:
        con = open_state_db(codex_home)
    except SystemExit as e:
        print(str(e), file=sys.stderr)
        return 2
    panes = load_panes(con)
    for p in panes.values():
        parse_rollout(p)
    root = find_run_root(panes, args.root)
    if root is None:
        print(
            "could not identify a run (no nazgul-role thread and no --root given)",
            file=sys.stderr,
        )
        return 2
    report = analyze(panes, root)
    text = json.dumps(report, indent=2, default=str)
    if args.out:
        Path(args.out).write_text(text)
    else:
        print(text)
    if args.md:
        Path(args.md).write_text(render_md(report))
    return 0 if report["green"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
