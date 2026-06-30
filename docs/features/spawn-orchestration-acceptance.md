# `/spawn` Orchestration Acceptance Criteria

Status: live persistent-pane orchestration is passing on the GLM/Vercel path; the exact GPT-5.5 Orc gate remains blocked by missing PFTerminal OpenAI auth; known limitation: native Codex Troll cannot dispatch to Claude-backed Orc (see Known Limitation section).

Current live evidence:

- PASS: Fresh tmux E2E with Vercel Fast Nazgul, Vercel Fast Troll, and two
  Vercel Fast native Codex Orcs produced a real website workflow. Burzum split
  work between Snaga and Ghash, reviewed both outputs, found a real
  `.step`/`.step-card` bug, forced targeted rework, then reported back to the
  Nazgul with files, server checks, and an inspection URL.
- PASS: After the Ctrl+C regression fix, live tmux test `pft_interrupt`
  survived process-level Ctrl+C during a running native turn. Before the fix,
  the same test killed the entire tmux session.
- PASS: Code-level regression for stale native Orc status is fixed. The app
  server now carries `task_complete.last_agent_message` through the
  `TurnCompleted` notification, and the TUI updates spawn status when thread
  notifications are enqueued rather than only when a child pane is actively
  replayed. Focused tests pass:
  `cargo test -p codex-tui native_spawn_turn --no-fail-fast`,
  `cargo test -p codex-tui enqueued_native_spawn_turn_completion_updates_status_before_replay --no-fail-fast`,
  and
  `cargo test -p codex-app-server --lib test_handle_turn_complete_includes_last_agent_message_item --no-fail-fast`.
- PASS: Live tmux proof of the status-rollup fix passed against the rebuilt
  binary in `pft_acceptance_after`. `/spawn` search matched `Spawn status` and
  `Send task to Snaga`; a direct Snaga task moved from `running` to `done` in
  `/spawn status`, and the row displayed the completed result preview from the
  worker's final answer.
- PASS: Live tmux run `pft_orch_live2` passed the persistent-pane routing gate
  after the task-context fix. Sauron sent a task to Burzum through `/spawn
  status`; Burzum saw the listed Snaga/Ghash Orc panes, reused them via
  `send_input`, waited for both, verified the files on disk, and reported back
  to the Nazgul. `/spawn status` then showed Burzum `done` with report preview,
  Snaga `done` with result preview, and Ghash searchable as the second Orc.
- PASS: Live tmux run `pft_orch_live3` passed a second real workflow with a
  different task class: read-only code review. Burzum received a routing review
  task, reused the existing persistent Snaga/Ghash Orc panes, assigned Snaga to
  correctness/code-quality review and Ghash to latency/UX review, waited for
  both, rejected one out-of-scope Ghash finding, sent a targeted rework, waited
  again, accepted the rework, independently spot-verified high-impact findings
  against source, then delivered a consolidated report upward. This is the
  qualitative orchestration bar: distinct worker assignments, supervisor review,
  rework, wait, verification, and final report.
- PASS: Live tmux interrupt sample in `pft_orch_live2` interrupted Snaga's active
  long-running turn from Snaga's pane. `/spawn status` showed Snaga
  `interrupted` while Burzum remained `done` and the TUI stayed alive. The
  background shell was then cleaned up with `/stop`.
- FIXED AFTER LIVE FAILURE: Live tmux run `pft_orch_live1` exposed that Burzum
  could still call native `spawn_agent` and create replacement worker IDs even
  when persistent Snaga/Ghash panes already existed. The send-task path now
  prepends a Troll task context listing existing Orc names/thread ids and
  instructing Trolls to reuse those panes before calling `spawn_agent`. The
  regression is covered by
  `troll_spawn_task_submission_names_existing_orc_panes`.
- FIXED AFTER LIVE REVIEW: `pft_orch_live3` exposed several small but material
  routing defects. Direct user sends to native spawn panes now emit an immediate
  "Task sent" confirmation. Troll task wrapping now remains present even when a
  Troll has no Orc panes yet, so first tasks do not lose role/supervision
  framing. Dispatch parser errors from inactive Claude panes are recorded into
  that source pane's transcript instead of being dropped. Implicit native Orc
  parent selection now fails closed when no Troll exists, while explicit Claude
  Troll parent selection still uses Codex Main as the backend parent thread and
  preserves the Claude Troll as the logical parent.
- PASS: Rebuilt-binary tmux smoke `pft_orch_confirm` verified the direct-send
  UX after the patch. A fresh `pfterminal --yolo` run created a demo crew,
  selected `Send task to Snaga [orc]`, submitted a tiny task, immediately showed
  `Task sent to Snaga [orc]. The pane will run it as a normal turn.` in Main,
  and `/spawn status` showed Snaga `running` with the exact task preview. Main
  stayed clean; the worker task was attached to Snaga.
- FIXED AFTER LIVE FAILURE: Switching from a running Troll Claude pane back to
  an idle Nazgul Claude pane no longer leaves the Troll's live `Claude running`
  panel visible on the Nazgul screen. Pane switching now suspends the global
  external-pane status display unless the selected pane itself is running. The
  regression is covered by
  `idle_claude_pane_selection_clears_previous_live_status_panel`.
- OBSERVED PROCESS RISK: In `pft_orch_live1`, Burzum issued a false rework over
  the valid word "truly". Snaga correctly pushed back, Burzum verified the
  mistake, preserved the file, and reported the process risk. This proves
  pushback/review auditability, but also shows model judgment can still waste a
  rework cycle.
- BLOCKED: The exact required scenario uses two Codex Orcs on GPT-5.5 xhigh.
  The local PFTerminal home does not currently have an OpenAI account auth file,
  and `pfterminal login status` returns `Not logged in`, so GPT-5.5 native Orc
  acceptance remains unproven in this environment. To unblock this exact gate,
  complete `pfterminal login --device-auth` or use `/providers` ->
  `Provider: OpenAI Codex Account`, then rerun the live tmux workflow with a
  GPT-5.5 xhigh Troll/Orc configuration.
- FAIL CASE TO PREVENT: In live use, Burzum received a website task but the
  operator could not see a meaningful Troll management transcript, and both
  Orcs appeared to receive overlapping or identical implementation instructions.
  That is not orchestration. It is hidden host dispatch plus worker spam.

This document defines what "done" means for `/spawn` orchestration. It exists
because the current implementation can create named panes, but live use exposed
core orchestration failures:

- The Troll pane did not show its own management work clearly.
- Two workers received overlapping or identical dispatches when their tasks
  should have been split.
- The user could not tell which pane emitted a dispatch or where it landed.
- Pane output and interrupt behavior were not reliably isolated by screen.

Passing unit tests, creating panes, or seeing a worker run one command is not
enough. `/spawn` is done only when the user can supervise a hierarchy from the
terminal and trust that each screen, task, and interrupt belongs to the entity
shown on that screen.

## Core Product Contract

`/spawn` creates a persistent, inspectable work hierarchy:

```text
Sauron (human)
  -> Nazgul (root pane / orchestrator)
      -> Trolls (supervisors)
          -> Orcs (executors)
```

Every entity has its own screen state. When the user switches panes, the screen
must show that entity's transcript, running status, queued work, and final
reports. Background work may update status metadata, but it must not dump raw
events into the visible pane.

## P0 Acceptance Gates

### 1. Pane-local transcript isolation

Acceptance:

- When viewing the Nazgul pane, the transcript contains Nazgul messages and
  explicit status summaries, not raw worker stream spam.
- When viewing a Troll pane, the transcript contains that Troll's management
  dialogue, dispatch decisions, worker reviews, and final reports.
- When viewing an Orc pane, the transcript contains that Orc's assigned task,
  tool activity, result, and evidence.
- Switching between panes restores the correct transcript for each pane.
- Background completions from inactive panes do not append messages to the
  active pane.

Failure examples:

- `Claude pane X is not running` repeating in a different pane.
- A worker's tool log appearing in the Nazgul pane without being summarized or
  selected.
- A Troll report appearing only in the currently active pane instead of the
  Troll's own pane.

### 2. Pane-local interrupt semantics

Acceptance:

- `Ctrl+C` or `Esc` interrupts only the visible active pane/turn.
- Interrupting the Nazgul pane must not stop Troll or Orc work.
- Interrupting a Troll pane must not stop the Nazgul pane or unrelated Orcs.
- Interrupting an Orc pane must not stop sibling Orcs or the Troll.
- Repeated interrupt on an idle pane is a no-op or a quiet state clear, not a
  repeated error stream.

Failure examples:

- Pressing cancel in one pane emits errors for another pane.
- Pressing cancel after a Claude pane is idle repeats "pane is not running".
- A stale active-pane id causes a native Codex worker interrupt to route to a
  Claude pane.

### 3. Supervisor action is visible

Acceptance:

- A Troll receiving a task must produce visible supervisory work in its own
  pane before and after dispatch.
- A Troll must have a persistent transcript entry for every management step;
  it is not acceptable for only the target Orc panes to show work.
- The Troll transcript must show:
  - task interpretation;
  - worker assignment plan;
  - which worker gets which task;
  - why the tasks are split that way;
  - review of each worker result;
  - rework request when a worker result is shallow or off-spec;
  - final report upward to the Nazgul.
- A Troll must not silently cause worker tasks to appear with no visible
  management step.

Failure example:

- The user sees only the initial task in the Troll pane while worker tasks
  appear elsewhere.
- The Troll says it delegated, but the host silently injected tasks into Orc
  panes with no visible Troll-side dispatch record.

### 4. Dispatch provenance is explicit

Acceptance:

- Every dispatched task records and displays:
  - source pane id and title;
  - source role and nickname;
  - target pane id and title;
  - target role and nickname;
  - parent/supervisor relationship at dispatch time;
  - task id;
  - timestamp;
  - task preview.
- `/spawn status` shows the latest dispatch provenance for each running or
  completed worker.
- Opening a worker pane shows who assigned the task.
- Opening a supervisor pane shows every task it sent and whether each target
  accepted, started, completed, failed, or was interrupted.

Failure examples:

- A worker receives a task and the user cannot tell whether it came from the
  Nazgul, Troll, or host parser.
- Two workers receive similar tasks and there is no task id/provenance to audit.

### 5. Distinct worker assignment

Acceptance:

- When a Troll delegates to multiple workers, each worker receives a distinct,
  non-overlapping task unless the Troll explicitly states that duplicate review
  is intended.
- The Troll pane must display the proposed per-worker split before dispatch:
  `Snaga -> owned files/tasks` and `Ghash -> owned files/tasks`, or equivalent.
- The UI must make task differences visible before dispatch.
- The host must detect exact duplicate worker dispatches from the same source
  turn and require confirmation or mark them as duplicate review.
- If the host cannot prove the two dispatches are distinct, it must block the
  second dispatch and surface the duplicate-risk warning in the Troll pane.
- A live test must prove two workers receive different work units for a real
  task.

Failure example:

- Snaga and Ghash both receive the same file ownership or identical assignment
  when the Troll claimed to split work.
- Both Orc panes receive a generic "build the website" prompt with no explicit
  ownership boundary.

### 6. Hierarchy is host-side truth, not prompt theater

Acceptance:

- The parent graph is stored in host state, not inferred only from model text.
- The host rejects invalid hierarchy changes:
  - Nazgul worker spawn;
  - Troll under Orc;
  - Orc under Nazgul when a Troll parent is required;
  - depth greater than the configured v0 hierarchy.
- The host can answer:
  - who supervises this worker;
  - which workers report to this supervisor;
  - which root pane owns the hierarchy;
  - which tasks are active under each supervisor.

Failure example:

- A Nazgul pane claims it has workers, but `/spawn status` disagrees.

### 7. Dispatch is a real host action

Acceptance:

- A model does not "send" work by merely saying it sent work.
- Dispatch requires a parseable host action that produces a durable task record.
- The dispatch parser must be robust against models accidentally emitting fake
  tool calls.
- The final visible assistant text must not expose raw dispatch control blocks.
- If dispatch parsing fails, the source pane must show a clear failure and no
  target pane should receive partial work.

Failure examples:

- Claude treats an XML-like dispatch block as a fake tool call; the contract must
  use fenced `pfterminal-send-task` blocks and strip them from visible text.
- A supervisor says "I dispatched to both workers" but no host task exists.
- A hidden parser sends work but the supervisor transcript does not show what
  happened.

### 8. Status rollup is reliable

Acceptance:

- Orc completion updates the supervising Troll status.
- Troll completion updates the Nazgul status.
- `/spawn status` shows running, done, interrupted, failed, and waiting states.
- Final reports include artifact links or evidence paths when available.
- Status survives pane switching and transcript replay.

Failure examples:

- A worker finishes but the Troll view still shows no result.
- A Troll finishes but the Nazgul view has no visible report or status update.

### 9. Auditability

Acceptance:

- Every turn has an audit artifact or rollout path linked from the relevant
  pane/status view.
- Dispatch records are durable enough to reconstruct:
  - original human task;
  - Troll plan;
  - worker task split;
  - worker outputs;
  - Troll review;
  - final Nazgul-facing report.
- The audit view must distinguish host-dispatched tasks from model prose.

Failure example:

- The user sees a worker prompt but cannot determine why it was sent or from
  which supervisor turn it came.

## Required Live E2E Scenario

This exact scenario must pass before marking `/spawn` done:

1. Start a fresh `pfterminal`.
2. Bind a Claude Code GLM 5.2 Fast Vercel pane as the Nazgul root.
3. Spawn one Troll on Claude Code GLM 5.2 Fast Vercel.
4. Spawn two Codex Orcs on GPT-5.5 xhigh under that Troll.
5. In the Nazgul pane, request:

   ```text
   Build a landing/marketing website for PFTerminal that Sauron can inspect
   locally in a browser on an open port. Delegate the implementation to Snaga
   and Ghash as you see fit. Manage the work distribution.
   ```

6. The Troll must visibly plan and dispatch different work to each worker.
7. Before or immediately after dispatch, the Troll pane must show a task split
   table or equivalent explicit mapping with one row per Orc.
8. The two worker panes must show different assigned tasks and different file
   or responsibility ownership. For the website scenario, examples of passing
   splits include content/components vs. styling/verification, or frontend
   implementation vs. review/build/server validation.
9. Each worker must produce concrete evidence: changed files, build output,
   server status, URL, or review findings.
10. The Troll must inspect both outputs and either request rework or produce a
   final report.
11. The Troll final report must name Snaga and Ghash separately, summarize what
    each did, cite evidence from each, and state whether rework was required.
12. The Nazgul pane must receive and display the Troll's final report.
13. The user must be able to switch among Nazgul, Troll, Snaga, and Ghash
    without seeing unrelated transcript spam.
14. Interrupting one pane during the scenario must not interrupt unrelated
    panes.

Passing result:

- `npm run build` passes for the website.
- A local server is running and reachable by URL.
- `/spawn status` shows the hierarchy and final statuses.
- Each pane transcript tells that pane's part of the story.
- The final Nazgul report contains the URL and evidence.

Failing result:

- Both Orc prompts are identical or materially overlapping without an explicit
  duplicate-review reason.
- The Troll transcript does not show the dispatch plan and review loop.
- The user cannot tell whether a task was sent by Burzum, by the Nazgul, or by
  hidden host-side parsing.
- A pane receives work while the visible source pane has no durable dispatch
  record for that work.

## Non-Goals For This Gate

- Perfect long-term persistence across application restart.
- Full marketplace role/plugin system.
- Additional roles beyond Nazgul, Troll, and Orc.
- Replacing native Codex subagents with a custom worker runtime.

## Known Limitation: Native Codex Troll Cannot Dispatch To Claude-Backed Orc

- A native Codex-harness Troll (e.g. GPT-5.5 on the OpenAI provider) supervises native Codex Orcs via the V1 multi-agent tools `send_input` / `followup_task` / `wait_agent` / `list_agents`. Its built-in role prompt (`core/src/agent/builtins/troll.toml`) instructs it to use those tools, not text dispatch blocks.
- The dispatch-block parser `extract_spawn_task_dispatches` (`tui/src/spawn_orchestration.rs:2090`) and the dispatcher `dispatch_spawn_task_blocks` (`tui/src/spawn_orchestration.rs:503`) are invoked ONLY from the Claude pane turn path (`tui/src/claude_panes.rs:5248` streaming progress and `:5304` turn finished). There is no call site in the native Codex turn-completion path.
- `send_input` resolves its target via `parse_agent_id_target` (`core/src/tools/handlers/multi_agents.rs:47`), which only accepts a `ThreadId`. It cannot target a Claude pane id.
- Net effect: a native Codex Troll cannot send work to a Claude-backed Orc pane. Any `<pfterminal_send_task>` block a native Troll emits is silently dropped (no parser in its turn path), and native `send_input` rejects a pane id as an invalid ThreadId.
- The reverse direction works: a Claude-backed Troll CAN dispatch to a native Codex Orc via `dispatch_spawn_task_blocks` → `SpawnTaskTarget::Native` → `AppEvent::SubmitSpawnAgentTask` (`tui/src/spawn_orchestration.rs:527`), which calls `app_server.turn_start` on the native Orc thread (`tui/src/app/event_dispatch.rs:2072`).
- Scope decision pending: closing this gap requires (1) injecting `write_spawn_dispatch_contract` into native Troll task context when Claude Orc children exist, and (2) a native-turn-finished hook in `tui/src/app/event_dispatch.rs` that calls `extract_spawn_task_dispatches` on the native Troll's assistant text. This is feature-scope work awaiting Nazgul sign-off on the prompt-change risk for native models.

## Implementation Implications

The current implementation should be considered incomplete until it has:

- pane-local transcript storage for every Claude and native worker pane;
- pane-local interrupt routing;
- durable task records with source and target metadata;
- duplicate-dispatch detection;
- supervisor-visible dispatch history;
- status rollup from worker to supervisor to root;
- a live E2E transcript proving the required scenario.
