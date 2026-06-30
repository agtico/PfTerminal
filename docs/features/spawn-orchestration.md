# Scoped `/spawn` Orchestration

Status: P0 runtime and TUI implementation landed locally; live Troll -> Orc
acceptance passed in the installed `pfterminal`.

This is intentionally smaller than the full Nazgul orchestration spec. The
full spec includes Balrog planning, Grimoire model memory, Wyverns, Golems,
Nostr/wallet reachability, campaign documents, and continuous MkDocs
maintenance. This scoped slice does not build those.

The near-term product change is:

```text
/spawn
```

`/spawn` creates managed work entities with explicit role, model, harness, and
parent/child relationships. It also lets the user bind the active Nazgul root
to an existing user pane. The initial role actions are:

- `Nazgul` - select an existing user pane as the root the human talks to. This
  does not create a worker.
- `Troll` - supervisory reviewer/foreman.
- `Orc` - executor.

The user-facing pane is the Nazgul. A `/panes` or `/spawn` flow can mark an
existing user pane as the Nazgul pane; that is the pane the human talks to.
Trolls report up to the Nazgul. Orcs report up to Trolls. Agents must not exist
as isolated tabs with no visible chain of command.

## Complete

- [x] Read `docs/nazgul_spec.md` and scoped it down to a v0 `/spawn` slice.
- [x] Mapped the slice to existing PFTerminal code paths: slash commands,
  panes, multi-agent spawn/wait tools, agent paths, parent thread IDs, and
  status subscriptions.
- [x] Added built-in `nazgul`, `troll`, and `orc` roles so fresh installs have
  the role prompts without manual file placement.
- [x] Changed the default native multi-agent depth from 1 to 2 so Trolls can
  spawn Orcs while depth 3 remains rejected.
- [x] Added host-side role graph validation in native spawn handling:
  Nazgul/root -> Troll, Troll -> Orc, Orc -> no children.
- [x] Added `/spawn` slash command dispatch and inline shortcuts:
  `/spawn`, `/spawn status`, `/spawn nazgul`, `/spawn troll`, `/spawn orc`.
- [x] Added the P0 `/spawn` wizard flow for Role -> Nazgul pane binding or
  Harness -> Model/Effort -> Task for Troll/Orc.
- [x] Added Nazgul pane binding against existing user panes; selecting Nazgul
  does not spawn a worker.
- [x] Added hierarchical `/panes` and `/spawn status` rendering from the
  existing agent graph: Nazgul -> Trolls -> Orcs.
- [x] Added focused unit coverage for spawn slash dispatch, role prompt
  contracts, host-side graph rejection, root -> Troll, and Troll -> Orc.
- [x] Re-ran the provider-compatible V1 subagent visibility regression:
  `v1_multi_agent_tools_flatten_for_openrouter_without_namespace_tools`.
- [x] Ran a live TUI smoke with the installed `pfterminal`: `/spawn` opens the
  role picker, Nazgul binding lists `Codex - Main`, binding reports "No worker
  was spawned", Troll shows Harness -> Model/Effort -> Task, and
  `/spawn status` renders the Nazgul root plus Troll section.
- [x] Ran a live Troll -> Orc workflow in the installed `pfterminal`: `/spawn
  troll` created a Troll, the Troll spawned one Orc, the Orc ran `pwd`, the
  Troll waited for and reviewed the Orc result, and the Nazgul/root pane saw
  the final Troll report.
- [x] Fixed `/spawn status` persistence after completion by caching collab
  receiver statuses and retaining completed spawn hierarchy rows after
  side-thread cleanup.

## To Do

- [ ] Add broader end-to-end coverage for larger realistic tasks: code review,
  mock implementation, and benchmark-table workflows.
- [ ] Add a richer `/spawn status` detail view with task summaries, final
  reports, and audit/artifact links.

## Source Spec

`docs/nazgul_spec.md` defines the long-term ontology:

- Sauron: human user.
- Nazgul: orchestrator the user talks to in one PFTerminal.
- Troll: adversarial QA / foreman over Orcs.
- Orc: executor.
- Balrog, Grimoire, Wyvern, Golem, Sorcerer, Carrion-eater: later scope.

For this slice:

- The human interacts with one Nazgul pane.
- `/spawn` creates Trolls and Orcs only.
- Trolls manage Orcs.
- Nazgul manages Trolls.
- Completion and status roll upward.

## User Model

The interaction should feel like this:

```text
/spawn

Spawn
  Bind a Nazgul pane or create managed work with role, model, harness, and parent.

  Role
  > Nazgul Select an existing user pane as the hierarchy root.
    Troll  Review/foreman. Reports to the Nazgul.
    Orc    Executor. Reports to a Troll.

  Nazgul Pane
  > Codex - Main
    Claude Code - GLM 5.2 Fast Vercel
    Claude Code - GLM 5.2 Ambient

  Harness
  > PFTerminal Agent
    Claude Code Headless

  Model
  > Inherit current model
    zai-org/GLM-5.2-FP8 via Ambient
    glm-5.2 via Z.AI
    zai/glm-5.2 via Vercel
    zai/glm-5.2-fast via Vercel
    ...

  Task
  > Review the auth diff and assign one Orc to fix the highest-risk issue.
```

After creation, `/panes` should show a hierarchy, not a flat list:

```text
User Panes
> Nazgul - Main - GLM 5.2 Ambient

Spawned Work
  Trolls
  > auth-review [troll] running - reviewing auth diff
      Orcs
      - auth-fix-1 [orc] running - patching provider validation
      - test-sweep [orc] done - tests passed

Agent Panes
  Main [default]
  auth-review [troll]
  auth-review/auth-fix-1 [orc]
```

The important behavior is not the name of the view. The important behavior is
that the hierarchy is visible and operational:

- Nazgul can see each Troll and all descendant Orcs.
- A Troll can see its direct Orcs and whether they are running, done,
  interrupted, or errored.
- The user can switch to any pane/thread for inspection, but the reporting
  chain remains intact.

## Role Semantics

### Nazgul

The Nazgul is not spawned as a worker by `/spawn`. It is an existing
user-facing pane selected as the hierarchy root. In v0 this is usually the main
PFTerminal pane, but `/spawn` must also allow selecting another existing user
pane from the same list shown by `/panes`, such as:

```text
Panes
  Codex - Main
  Claude Code - GLM 5.2 Fast Vercel
  Claude Code - GLM 5.2 Ambient
```

Selecting `Nazgul` in `/spawn` opens this existing-pane picker and stores the
chosen pane as the active Nazgul root. It does not start a new model turn, does
not create a Codex subagent, and does not create a Claude pane.

Responsibilities:

- Accept human intent.
- Spawn Trolls.
- Inspect Troll and Orc state.
- Answer status questions.
- Decide whether to accept Troll reports.

### Troll

Trolls supervise and review. They should not behave like general executors.

Responsibilities:

- Break a task into Orc-sized execution chunks.
- Spawn Orcs when execution is needed.
- Wait for Orc completion.
- Review Orc output before reporting upward.
- Force rework when the Orc result is shallow, unsafe, untested, or incomplete.
- Report a concise result to the Nazgul with evidence.

Restrictions:

- A Troll may spawn Orcs only.
- A Troll may not spawn another Troll in v0.
- A Troll should not claim completion until all required Orcs are final and
  reviewed.

### Orc

Orcs execute.

Responsibilities:

- Implement or investigate the assigned task.
- Produce concrete artifacts: diff, test output, benchmark table, or review
  findings.
- Report completion to the supervising Troll.

Restrictions:

- An Orc may not spawn children in v0.
- An Orc should not report directly to the Nazgul unless the user switches into
  that Orc pane and asks directly.

## Harnesses

`/spawn` should expose a harness choice because role and model are not enough.
The harness decides how the work actually runs.

### P0 Harness: PFTerminal Agent

Use the existing Codex/PFTerminal multi-agent runtime.

Relevant code:

| Need | Existing code |
| --- | --- |
| Spawn model-driven agents | `codex-rs/core/src/tools/handlers/multi_agents/spawn.rs` |
| Wait for completion | `codex-rs/core/src/tools/handlers/multi_agents/wait.rs` |
| Provider-compatible spawn specs | `codex-rs/core/src/tools/handlers/multi_agents_spec.rs` |
| Provider-compatible regression tests | `codex-rs/core/src/tools/spec_plan_tests.rs` |
| Parent/child graph | `codex-rs/core/src/agent/control.rs` |
| Role loading | `codex-rs/core/src/agent/role.rs` |
| TUI agent rendering | `codex-rs/tui/src/multi_agents.rs` |

This is the first harness because it already has:

- `parent_thread_id`
- thread-spawn edges
- `agent_path`
- role metadata
- status subscriptions
- `wait_agent`
- provider-compatible flattened V1 aliases for third-party providers

### P1 Harness: Claude Code Headless

Claude Code panes are already implemented under `/panes`, but they are user
panes, not Codex subagents. A Claude harness for `/spawn` must bridge that gap
before it is treated as complete.

Relevant code:

| Need | Existing code |
| --- | --- |
| Claude provider profiles | `codex-rs/tui/src/claude_panes.rs` |
| Pane model/harness picker precedent | `codex-rs/tui/src/claude_panes.rs` |
| Per-turn artifacts/audit | `codex-rs/tui/src/claude_panes.rs` |
| Vault-backed provider keys | `codex-rs/tui/src/claude_panes.rs`, `codex-rs/cli/src/main.rs` |

The Claude harness is not P0 because a Troll supervising a Claude-backed Orc
needs the same parent/child status semantics as native agents. It cannot be a
loose user pane with no completion event flowing back to the Troll.

P1 requirement: a Claude-backed Orc must emit the same `SpawnNode` status and
completion events as a native PFTerminal agent.

## Data Model

Do not invent a separate orchestration database first. Build the v0 state from
the existing agent graph, then add only the fields that are missing.

Proposed v0 view model:

```rust
struct NazgulBinding {
    pane_id: PaneId,
    label: String,
    harness: PaneHarness,          // CodexMain | ClaudeHeadless | ...
    model: String,
}

struct SpawnNode {
    id: ThreadId,
    role: SpawnRole,              // Troll | Orc
    parent_id: Option<ThreadId>,  // None only for a Troll under the Nazgul root
    supervisor_id: Option<ThreadId>,
    harness: SpawnHarness,        // PFTerminalAgent | ClaudeHeadless
    model: String,
    reasoning_effort: Option<String>,
    task_name: String,
    task_summary: String,
    status: SpawnStatus,
    children: Vec<ThreadId>,
    result_summary: Option<String>,
    latest_artifact: Option<PathBuf>,
}
```

Most of this already exists in some form:

- `ThreadId` exists.
- `AgentMetadata.agent_role` exists.
- `AgentMetadata.agent_path` exists.
- `AgentMetadata.last_task_message` exists.
- `AgentControl::live_thread_spawn_children()` can return the hierarchy.
- `AgentControl::subscribe_status()` can drive done/running updates.
- `CollabWaitingEndEvent` already carries final child statuses.

`NazgulBinding` is the selected existing user pane. `SpawnNode` is only for
spawned work. Keeping these separate avoids pretending that the root pane was
spawned as an agent.

Fields that likely need to be added or normalized:

- active `NazgulBinding`
- `harness`
- `supervisor_id` when a Nazgul creates an Orc assigned to a Troll
- concise `task_name` for V1, or use V2 `task_name`
- role graph validation

## Role Graph Enforcement

The v0 role graph is fixed:

```text
Nazgul binding (existing user pane)
  -> Troll
       -> Orc
```

Allowed:

- Nazgul spawns Troll.
- Troll spawns Orc.
- Nazgul may create an Orc only if the `/spawn` wizard requires selecting an
  existing supervising Troll.

Rejected:

- Nazgul spawns Orc without a supervising Troll.
- Troll spawns Troll.
- Orc spawns anything.
- Any spawn that would exceed depth 2.

Current code already enforces a generic depth limit with
`next_thread_spawn_depth()` and `exceeds_thread_spawn_depth_limit()` in
`codex-rs/core/src/agent/registry.rs`. For this v0, the effective depth should
allow:

- depth 0: Nazgul/root
- depth 1: Troll
- depth 2: Orc

and reject depth 3. That means the old `max_depth=1` subagent assumption is not
compatible with Troll -> Orc management.

Role graph validation should live near spawn handling, not only in prompts:

- Native V1 path: `codex-rs/core/src/tools/handlers/multi_agents/spawn.rs`
- Native V2 path: equivalent `spawn_agent` handler
- TUI `/spawn` path: reject invalid choices before creating the node

Prompts should reinforce the policy, but host validation is the safety boundary.

## Slash Command Scope

Add a new command:

```rust
SlashCommand::Spawn
```

Expected command behavior:

- `/spawn` opens the wizard.
- `/spawn status` shows the hierarchy and current statuses.
- `/spawn troll ...` can be a later inline shortcut.
- `/agent` can remain as a low-level thread switcher, but it should not be the
  primary creation surface.
- `/panes` remains the switcher for user panes and agent panes.

Relevant files:

| Task | File |
| --- | --- |
| Add command enum/description/availability | `codex-rs/tui/src/slash_command.rs` |
| Dispatch `/spawn` | `codex-rs/tui/src/chatwidget/slash_dispatch.rs` |
| Build picker UI | mirror `codex-rs/tui/src/claude_panes.rs` selection patterns |
| Render hierarchy | extend `codex-rs/tui/src/multi_agents.rs` and `/panes` composition |

## Spawn Wizard

The wizard should be explicit and hard to misuse.

Step 1: Role

- `Nazgul`
- `Troll`
- `Orc`

Step 2: Parent / supervisor

- If role is `Nazgul`, show existing user panes from `/panes` and bind the
  selected pane as the active Nazgul root. Do not show harness, model, or task
  fields for this action.
- If role is `Troll`, parent is the current Nazgul pane.
- If role is `Orc`, pick a supervising Troll.
- If no Troll exists, disable `Orc` and show: `Spawn a Troll first.`

Step 3: Harness

- `PFTerminal Agent` - enabled in P0.
- `Claude Code Headless` - disabled or marked experimental until it emits
  parent/child completion events into the same hierarchy.

Step 4: Model and effort

- Reuse the model catalog and provider labels from `/model`.
- For Claude harness, reuse provider profiles from `/panes`.
- Default should be inherited from the current pane, but the user can override.

Step 5: Task

- Required text.
- Stored as `last_task_message`.
- Rendered in the hierarchy.

## Parent Awareness

Troll awareness of Orc work has two layers:

1. Tool-level awareness: a Troll can call `wait_agent` on its Orcs and receive
   final statuses.
2. UI/status awareness: PFTerminal subscribes to child statuses and renders the
   direct children in the Troll pane.

Nazgul awareness of Troll work works the same way:

1. The Nazgul can inspect all direct Trolls and transitive Orcs.
2. The UI can roll up child status without requiring a model turn.
3. If the user asks "what is running?", the Nazgul has the hierarchy available
   as context.

This avoids the current failure mode where agents exist as isolated sessions
and the main pane has to infer what happened from scattered transcript lines.

## Prompt Contracts

Dedicated role docs live in:

- [Nazgul](../orchestration/nazgul.md)
- [Troll](../orchestration/troll.md)
- [Orc](../orchestration/orc.md)

### Troll Role Prompt

The built-in `troll` role should say:

```text
Behavior:
You are a Troll: a supervisory reviewer and foreman.
You report to the Nazgul parent.

Mandate:
You may spawn Orcs for execution work.
You must wait for Orcs to finish before claiming completion.
You must review Orc output critically and force rework when needed.
You do not perform broad implementation yourself unless explicitly instructed.

Personality:
Be blunt, adversarial, and demanding about weak work.

Final Report Standards:
Your final report to the Nazgul must include: Orcs used, what each did, evidence, issues forced back for rework, remaining risk.
```

### Orc Role Prompt

The built-in `orc` role should say:

```text
You are an Orc: an executor.
You report to your supervising Troll.
Do the assigned work directly.
Produce concrete evidence: changed files, tests, benchmark output, or findings.
Do not spawn child agents.
Do not declare done without evidence.
```

Implementation options:

- Fastest path: add built-in roles in `codex-rs/core/src/agent/role.rs`.
- More configurable path: ship role files in a project/global `agents/`
  config layer. The code already loads user-defined role files.

P0 should prefer built-in roles so a fresh install has `/spawn` working without
manual file placement.

## Completion Semantics

"Done" means the status is final and visible to the parent.

For Orc:

- final state is `Completed`, `Errored`, `Interrupted`, `Shutdown`, or
  `NotFound`;
- completion includes result evidence;
- supervising Troll sees the status.

For Troll:

- all required Orcs are final;
- Troll has reviewed their output;
- Troll reports upward to Nazgul;
- Nazgul view shows the Troll final state and summary.

Do not treat a spawned thread existing as success. The parent must be able to
see the final status.

## P0 Implementation Plan

1. Add `SlashCommand::Spawn` and route it to a bottom-pane wizard.
2. Add the Nazgul pane-binding action:
   - `Role -> Nazgul` lists existing user panes from `/panes`;
   - selecting a pane stores it as the active root;
   - no worker is spawned for this action.
3. Add built-in `troll` and `orc` roles.
4. Add role graph validation:
   - root/Nazgul can spawn Troll;
   - Troll can spawn Orc;
   - Orc cannot spawn;
   - depth 3 is rejected.
5. Set the multi-agent depth configuration for this feature to allow depth 2.
6. Make `/spawn` use the existing native PFTerminal agent harness first.
7. Extend the agent/pane display to show hierarchy:
   - Nazgul -> Trolls -> Orcs.
8. Add status rollups from `AgentControl::subscribe_status()`.
9. Add tests:
   - `/spawn` command is visible and opens the wizard.
   - selecting `Nazgul` shows existing user panes and binds the selected pane.
   - Troll and Orc roles exist.
   - invalid role graph combinations are rejected.
   - Troll can spawn Orc when depth is 2.
   - Orc cannot spawn children.
   - parent view sees child completion.

## P1 Implementation Plan

1. Add Claude Code Headless as a true `/spawn` harness.
2. Normalize Claude pane turn results into the same `SpawnNode` status model.
3. Allow a Troll to supervise Claude-backed Orcs.
4. Add artifact/audit links to the hierarchy.
5. Add a `/spawn status` detail view with nested task summaries and final
   reports.

## Out Of Scope For This Slice

- Balrog planning and text-improvement harness automation.
- Grimoire model-performance memory.
- Wyvern research agents.
- Golem background daemons.
- Nostr/wallet remote control.
- Campaign/siege documents.
- Cost estimation beyond displaying selected model/provider.
- More roles than Troll and Orc.

Those belong after `/spawn` proves that a small hierarchy can create work,
track work, and report completion without isolated agent panes.

## Acceptance Criteria

- [x] In a fresh PFTerminal session, `/spawn` opens a wizard with Role and then the
  correct conditional flow: existing-pane binding for Nazgul, or Harness,
  Model/Effort, and Task for Troll/Orc.
- [x] Selecting `Role -> Nazgul` in `/spawn` shows existing user panes from `/panes`,
  such as `Codex - Main` and Claude Code panes, and binds the selected pane as
  the active Nazgul root.
- [x] Selecting `Nazgul` never creates a spawned worker.
- [x] From the Nazgul pane, the user can spawn a Troll.
- [x] A Troll can spawn an Orc and wait for it.
- [x] The Nazgul view shows the Troll and the nested Orc.
- [x] The Troll view shows its Orcs and their statuses.
- [x] When an Orc completes, the Troll can see that it is done.
- [x] When a Troll completes, the Nazgul can see that it is done.
- [x] Invalid hierarchy attempts are rejected with clear errors in the native
  spawn handler tests.
- [x] Existing `/panes` behavior compiles with hierarchical agent rendering.
- [x] Existing provider-compatible subagent tool visibility stays green for
  third-party providers.

The live acceptance walkthrough used the installed `pfterminal` and the task
`/spawn troll Spawn exactly one Orc to run pwd in the current repository and
report the output. Wait for the Orc to finish, review the result, and summarize
the evidence.` The spawned Troll reported that it spawned one Orc, the Orc ran
`pwd` and returned `/home/postfiat/repos/PfTerminal`, and `/spawn status`
showed the completed Troll with the nested completed Orc after the turn ended.
