# Claude Code Pane Completion Spec

Status: Ambient provider complete; Z.AI, Baseten, OpenRouter, and Claude Plan
profiles remain experimental until their workflow suites pass.

## Hard Completion Bar

Claude panes are not done until the actual `/panes` user experience can do all
of this without operator debugging:

- [x] Create a mock website in a disposable directory and verify the resulting
  files.
- [x] Run a concrete NumPy vs Pandas benchmark and output a readable result
  table.
- [x] Conduct a substantive code review on a local PFTerminal-owned repo.
- [x] Expose turn-by-turn progress and audit records so a long run is never a
  silent black box.

These are product acceptance tests. A helper command, one-turn smoke response,
or raw artifact path is not enough.

This is the completion spec for wrapped Claude Code panes in PFTerminal. The
Ambient profile now passes the required workflow suite and the actual `/panes`
TUI path. The other provider profiles are still intentionally labeled
experimental until they pass the same workflow suite.

Passing smoke tests is not sufficient. "Done" means the interactive `/panes`
workflow can perform real work repeatedly, with visible progress and useful
turn-by-turn auditability.

## Definition Of Done

Claude panes are complete for a provider profile only when all four workflows
below pass through the same pane backend used by `/panes`, and the actual TUI
can create that pane, route a turn, and expose artifact/audit paths.

- [x] **Mock website creation:** create a small mock website in a disposable
  fixture directory, including HTML/CSS/JS or the repo-native equivalent, then
  verify the files exist and contain the expected content.
- [x] **Benchmark task:** run a discrete NumPy vs Pandas benchmark, capture the
  benchmark command, and render a result table with timings and a short
  interpretation.
- [x] **Code review task:** conduct a code review on one PFTerminal-owned repo
  from a Claude pane and return findings with file references. This must pass
  in a fresh pane and in a resumed pane.
- [x] **Turn auditability:** every turn must expose enough status to know what
  Claude is doing while it runs, what tools it used, what it produced, and why
  it stopped. A 15-minute silent run is a failure.

## Current Evidence

Verified on June 25, 2026 with the Ambient profile:

- Workflow report:
  `/home/postfiat/.pfterminal/panes/workflow-reports/claude-pane-workflow-suite-1782399985.json`
- Summary: `Claude pane workflow suite: 4 passed, 4 checked`.
- Mock website audit:
  `/home/postfiat/.pfterminal/panes/claude-00f079fc-f1b5-4829-87b0-e254732e980d/turn-0001.audit.json`
- NumPy/Pandas benchmark audit:
  `/home/postfiat/.pfterminal/panes/claude-71832356-e5b6-4568-8839-285865700735/turn-0001.audit.json`
- Code review resumed-turn audit:
  `/home/postfiat/.pfterminal/panes/claude-aa5e5aa9-afb9-42f8-8fe6-6999cf39e120/turn-0002.audit.json`
- Auditability multi-turn audit:
  `/home/postfiat/.pfterminal/panes/claude-6a9192e8-4a10-4c0e-88bb-ea7d9f6397b5/turn-0003.audit.json`
- Actual TUI `/panes` smoke:
  `/home/postfiat/.pfterminal/panes/claude-83ff79a4-b960-49c8-b8bf-8dc949ef18f2/turn-0001.audit.json`
  showed `status=success`, `usage_status=untrusted`, immediate artifact/audit
  paths, and the visible marker `PFT_TUI_USAGE_OK`.

Important implementation guardrail: the Ambient bridge now enforces a local
3-tool-call budget per Claude pane turn. The failed runs showed that larger
tool histories could push GLM-backed Claude Code into API retry loops during
code review. After three tool calls, the bridge removes tools from the upstream
request and instructs Claude to produce a final answer using gathered evidence.

## Repeated Failures This Spec Must Prevent

The following failures already happened and are now explicit release blockers:

- I treated a narrow smoke runner as proof of product readiness.
- I called the work complete while `pfterminal-ci` was still running.
- I shipped a pane path that could return one trivial response but failed on
  substantive Claude Code work.
- I did not test the same workflows across the intended provider profiles before
  presenting the implementation as complete.
- A real resumed code-review prompt timed out after one user test.
- The timeout was labeled as a provider error even though the product problem
  was the pane runner's inability to handle long Claude work transparently.
- The audit rendered `input_tokens=0` and `output_tokens=0` as if that were
  trustworthy usage data.
- The UI exposed artifact paths but did not provide enough live progress,
  tool-step visibility, or turn summaries for the user to understand what was
  happening.
- Tests proved plumbing, not the actual user workflows.
- The completion report overstated success by counting shallow pass/fail checks
  while the real `/panes` UX still failed a basic code-review prompt.

The corrective principle is simple: do not claim parity with Claude Code until
PFTerminal can demonstrate comparable multi-turn work through its own pane UI.

## Product Requirements

### Pane UX

- `/panes` must show user panes and agent panes separately.
- Selecting a Claude pane must make new user prompts route to Claude until the
  user switches away.
- The active footer/status line must clearly show that the active pane is
  Claude, including provider/model when space allows.
- Creating a pane must fail early with a clear missing-credential message when
  the required vault key is absent.
- Provider profiles that have not passed the workflow suite must be labeled
  experimental or unavailable.

### Long-Running Turn Transparency

During a Claude turn, PFTerminal must show bounded live progress. At minimum:

- elapsed time;
- current phase: starting Claude, waiting for provider, tool call, tool result,
  assistant response, audit write, or timeout handling;
- last tool name and a bounded preview of its input/result;
- artifact path and audit path as soon as they are known;
- whether the run is first turn or resumed session.

The UI must not sit for minutes with only a generic spinner.

### Audit Record

Every turn must write and surface an audit record with:

- pane id and pane title;
- provider profile and model;
- Claude session id;
- turn number;
- command mode: new session or resume;
- max-turn setting and wall-clock timeout setting;
- start time, last-progress time, end time, and duration;
- artifact path and audit path;
- stream-json parse status;
- tool-call count, tool names, and bounded previews;
- usage values when known;
- explicit `usage_status`: `reported`, `missing`, `unknown`, or `untrusted`;
- terminal reason;
- final result status: success, max-turn pause, timeout pause, provider error,
  permission/tool error, parse failure, or user interrupt.

If usage is missing or zero because the provider did not report it, the UI must
say that. It must not imply that a real Claude turn consumed zero tokens.

### Timeout And Resume Policy

A timeout is not automatically a provider error.

The pane backend must distinguish:

- provider returned an error;
- Claude hit max turns but has a resumable session;
- PFTerminal wall-clock timeout fired while Claude may still be working;
- Claude produced partial useful output;
- no useful output was produced.

For max-turn or timeout pauses, the pane must remain resumable and the UI must
offer a clear continue action. If the turn cannot be resumed safely, the UI must
say so and point to the audit record.

### Secret Handling

- Provider keys must remain vault-owned.
- Raw provider keys must not be written to settings files, command arguments,
  artifacts, audit files, chat history, logs, or docs.
- Tests must check artifact and audit files for absence of the active provider
  secret.

## Mandatory Workflow Tests

These tests must run through the same code path as `/panes`. A helper may drive
the pane programmatically, but it must use the pane registry, command planner,
Claude invocation, artifact writer, audit writer, and result renderer.

For each workflow, run a live Claude Code TUI baseline first when practical, then
run the matching PFTerminal `/panes` workflow. The result does not need to be
byte-identical, but it must be comparable in capability: it should perform the
same class of work, expose useful progress, and leave inspectable artifacts.

Record the comparison for every workflow:

- live Claude Code command or prompt;
- PFTerminal `/panes` provider profile;
- PFTerminal prompt;
- pass/fail result;
- artifact path;
- audit path;
- observed gap, if any.

### 1. Mock Website Creation

Fixture:

- create a temporary empty directory;
- create a Claude pane rooted in that directory;
- prompt Claude to build a mock website for a simple product page;
- require at least `index.html` and one styling/script asset unless the chosen
  stack has a repo-native equivalent.

Pass criteria:

- files are created in the fixture directory;
- content includes the requested product name and at least one interactive or
  styled element;
- audit shows tool use and final success;
- no timeout or max-turn pause.

### 2. NumPy vs Pandas Benchmark

Fixture:

- create a temporary Python script or notebook-equivalent in a disposable
  directory;
- benchmark one concrete task, such as filtering and aggregating one million
  rows of numeric data;
- run both NumPy and Pandas implementations.

Pass criteria:

- Claude runs the benchmark command;
- output includes a markdown table with at least implementation, mean time,
  fastest run, and notes;
- artifact captures the command output;
- audit shows the command/tool used;
- if Python dependencies are missing, the pane reports a clear environment
  failure rather than hanging.

### 3. Code Review

Fixture:

- run against one of the local repos, for example
  `/home/postfiat/repos/PfTerminal` or `/home/postfiat/repos/StakeHub`;
- use a realistic prompt: "do a code review of this implementation" with a
  file or subsystem target;
- run once in a fresh pane and once in a resumed pane.

Pass criteria:

- review completes without timeout;
- output includes concrete findings or "no findings" with file references;
- audit shows file-reading/tool activity;
- resumed-pane review does not lose context;
- the result is visible in the TUI, not only in a JSONL file.

### 4. Turn-By-Turn Auditability

Fixture:

- run a multi-turn Claude pane session with at least three turns;
- include one long-running turn and one failure-path turn.

Pass criteria:

- every turn has a visible summary in the UI;
- every turn has a valid audit JSON file;
- audit entries can be opened from the UI path shown to the user;
- long-running turns update progress at least every 30 seconds;
- failed turns show status and next action, not just "provider error."

## Provider Matrix

Each supported provider must run the workflow suite or be explicitly marked as
experimental.

| Provider pane | Required credential | Required evidence |
| --- | --- | --- |
| Claude Code - GLM 5.2 Ambient | `provider/ambient_api_key` | All four workflows pass through `/panes`. |
| Claude Code - GLM 5.2 Z.AI | `provider/zai_api_key` | All four workflows pass or profile is marked experimental. |
| Claude Code - GLM 5.2 Baseten | `provider/baseten_api_key` | All four workflows pass or profile is marked experimental. |
| Claude Code - OpenRouter | `provider/openrouter_api_key` | All four workflows pass or profile is marked experimental. |
| Claude Code - Claude Plan | Claude native auth | All four workflows pass or profile is marked unavailable. |

## Required Commands

Before completion can be claimed, run and record fresh output for:

```bash
cargo test -p codex-tui claude_panes --no-fail-fast
cargo test -p codex-cli claude_pane_smoke_parses_provider_list
cargo clippy -p codex-tui -p codex-cli --tests
cargo build -p codex-cli --bin pfterminal
.venv-docs/bin/mkdocs build --strict
```

Live workflow runner required before release:

```bash
pfterminal claude-pane-workflow-suite \
  --providers ambient,zai,baseten,openrouter,claude-plan \
  --workflows mock-website,numpy-pandas-benchmark,code-review,auditability \
  --cwd /home/postfiat/repos/PfTerminal
```

The runner must write:

- machine-readable report under `$CODEX_HOME/panes/workflow-reports/`;
- human-readable summary with pass/fail per provider and workflow;
- paths to every artifact and audit file;
- exact command lines used for benchmarks and builds.

## CI Gate

Do not call the feature complete while GitHub CI is still running.

Completion requires:

- latest pushed commit equals local `HEAD`;
- worktree is clean;
- Codespell passes;
- cargo-deny passes;
- `pfterminal-ci` passes or the specific failure is understood, documented,
  and unrelated to this work;
- no failing check is dismissed as "still running."

## Release Rule

The Claude pane integration is not done until a user can open `/panes` and run
the four mandatory workflows without debugging Claude, providers, artifacts,
or PFTerminal internals.

Any claim of completion must include:

- commit hash;
- CI status;
- workflow-suite report path;
- provider matrix result;
- mock website artifact path;
- NumPy vs Pandas benchmark table path;
- code-review output path;
- auditability report path;
- known limitations that remain visible in the UI.
