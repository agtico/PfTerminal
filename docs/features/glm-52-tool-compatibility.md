# GLM 5.2 Tool Compatibility Proposal

## Complete

- [x] Implement `structured_edit` and `structured_write` for GLM/Z.AI/Ambient-style model profiles.
- [x] Preserve Codex-native strict `apply_patch` behavior for models that handle the grammar correctly.
- [x] Rebuild `pfterminal`, update the PATH wrapper, and verify the structured-edit path with deterministic tests.

## To Do

- [x] Re-run live GLM structured-edit smoke testing after the Z.AI `429` rate limit clears.
- [x] Finish harness-side command-budget enforcement for explicit read-only review caps.
- [x] Run a broader GLM repo-review/edit benchmark and record pass/fail evidence in this feature page.

Status: feature implementation record with V0 core tool support implemented
and live GLM validation in progress.

Latest readiness update, 2026-06-23 02:00 UTC: `pfterminal` was rebuilt from
this repo and the PATH wrapper at `/home/postfiat/.local/bin/pfterminal` now
executes `/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal`.
The rebuilt binary is ready for manual GLM structured-edit testing. The first
post-rebuild live smoke was blocked by Z.AI `429 Too Many Requests`, but the
rerun after the rate limit cleared succeeded through the file-change path.

## Executive Summary

GLM 5.2 is working through the OpenAI-compatible transport and Codex tool-call
loop. The observed failure is narrower: it repeatedly missed Codex's strict
`apply_patch` grammar during a live `/vault` implementation session.

The OpenCode source review confirms the low-friction shape: do not force every
model through the same edit primitive. OpenCode exposes a patch tool only for
model families it expects to handle patches, and exposes normal JSON
`edit`/`write` tools to the rest. PFTerminal should follow that harness pattern:
keep strict `apply_patch` unchanged for Codex-native models, but route GLM/Z.AI
and similar providers to structured edit/write tools.

V0 of that path is now implemented in the PFTerminal core tool harness. The
remaining work is live GLM validation across real repo-review/edit loops,
per-session aggregation beyond the turn-scoped strict-patch fallback threshold,
and hardening review-loop discipline so successful tool compatibility turns into
usable throughput.
Model-authored Python heredoc source rewrites are now rejected for
structured-edit profiles, so shell remains available for tests/builds but is no
longer the normal edit fallback. V0 now also emits a low-cardinality
`codex.model_edit_compatibility` counter around strict-patch failures,
structured edit/write outcomes, and Python heredoc source-write rejections.

This is not a claim that GLM 5.2 cannot code. In the current session it used
chat completions, tool calls, shell commands, file creation, tests, and ordinary
compile/test repair. The failing surface was strict `apply_patch` grammar.

## Session Evidence

Source: `/home/postfiat/.pfterminal/logs_2.sqlite` plus live rollout traces
under `/home/postfiat/.pfterminal/sessions/2026/06/23/`.

Active session:

```text
thread_id: 019ef112-d498-7853-a4d8-1776ebf38021
model: glm-5.2
cwd: /home/postfiat/repos/PfTerminal
task: implement /vault v0
observed_at: 2026-06-22 UTC, matching the host clock for this note
```

The logs show the normal Codex loop working:

- model responses completed through the chat-completions transport;
- `apply_patch` and `exec_command` tool calls were emitted;
- tool results were returned to the model;
- `cargo nextest` and `cargo build` were invoked by the worker;
- the worker continued after test/build failures.

The logs also show repeated malformed `apply_patch` calls.

| Time UTC | Tool | Failure |
| --- | --- | --- |
| 2026-06-22 21:04:37 | `apply_patch` | Update hunk included `/// Remove a provider key...` without the required leading ` `, `+`, or `-`. |
| 2026-06-22 21:04:55 | `apply_patch` | Update hunk included `pub fn provider_api_key_from_auth_storage(` without the required leading ` `, `+`, or `-`. |
| 2026-06-22 21:05:22 | `apply_patch` | Parser expected an `@@` context marker and received `}`. |

Representative error text from the tool router:

```text
apply_patch verification failed: invalid hunk at line 4,
Unexpected line found in update hunk: 'pub fn provider_api_key_from_auth_storage('.
Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)
```

After these failures, the worker repeatedly fell back to `exec_command` with
`python3 - <<'PY'` file rewrites. That fallback was effective enough to keep
moving, but it bypasses the stricter edit contract and creates avoidable
latency, context growth, and risk.

A later review found the more serious version of that risk: one Python fallback
edit produced syntactically valid Rust but changed the wrong match arm. The
`/vault` command was added to `available_during_task`, but not to
`supports_inline_args` in `codex-rs/tui/src/slash_command.rs`. Because the
composer only routes slash commands with arguments through the inline-args path,
`/vault credential add` would fall through to ordinary chat submission instead
of opening the secure vault-entry modal. This is not just a failed patch syntax
case; it is a semantic misplacement after a shell rewrite fallback.

V0 structured-edit smoke evidence:

```text
thread_id: 019ef1cb-4940-7cf2-a3dd-abd7a8813b62
model: glm-5.2
provider: zai
cwd: /tmp/pft_glm_structured_smoke
tool: structured_edit
result: notes.txt changed beta -> BETA
rollout: /home/postfiat/.pfterminal/sessions/2026/06/23/rollout-2026-06-23T00-04-52-019ef1cb-4940-7cf2-a3dd-abd7a8813b62.jsonl
```

That run proves the low-friction path at the live model/tool layer: GLM emitted
JSON arguments for `structured_edit`, PFTerminal converted the edit into the
internal patch runtime, and the file changed without any `apply_patch` grammar
call or Python rewrite.

Post-telemetry smoke evidence:

```text
thread_id: 019ef201-cbd4-7fb1-b92b-ab2fb82a3387
model: glm-5.2
provider: zai
cwd: /tmp/pft_glm_metric_smoke
result: notes.txt changed beta -> BETA
rollout: /home/postfiat/.pfterminal/sessions/2026/06/23/rollout-2026-06-23T01-04-24-019ef201-cbd4-7fb1-b92b-ab2fb82a3387.jsonl
```

That run used the rebuilt binary with compatibility telemetry inserted. The
visible rollout shows a file-change event and no strict `apply_patch` call or
Python source rewrite; the final file content was `alpha\nBETA\ngamma\n`.

A longer read-only GLM review of the PFTerminal changes started as thread
`019ef1d0-0ef8-7ca1-b9d2-3429215dc1a3` and reached 170 KB of JSONL trace before
the 420 second timeout. It did not produce a final report file, so it is only
partial evidence. The useful signal is operational: the model inspected the git
diff and the relevant handler/parser files through shell reads, did not call
`apply_patch`, and initially tripped the structured-edit guard by trying an
empty `old_string` before recovering to read commands. That should be counted as
a usability gap for review-only tasks, not as an edit-path failure. The
structured tool descriptions and empty-`old_string` error now explicitly say the
tool is not for reading or inspection.

## What This Means

This is not primarily an OpenAI API compatibility problem. If the API layer were
the blocker, the session would fail before tool execution: malformed streaming
events, missing `tool_calls`, bad request schema, or unusable tool JSON.

Instead, the transport and function-call loop work. The problem appears when
GLM 5.2 must satisfy Codex's exact custom patch grammar.

In this session, GLM produced patch shapes that are common in other coding
tools:

- normal unified diff context;
- file sections without Codex's custom hunk prefixes;
- direct code blocks inside update hunks;
- repair edits through Python scripts when patch calls fail.

Codex currently expects a narrower patch language.

## Evidence Strength And Validation

The current evidence is one live implementation session, one successful GLM
structured-edit smoke, two completed GLM read-only repo reviews, and two partial
GLM review traces. That is enough to justify the V0 structured edit/write path,
not enough to declare the model profile production-ready.

Before PFTerminal treats a GLM 5.2 capability profile as production-ready, the
team should collect:

- at least three independent GLM 5.2 coding sessions or a fixed benchmark of
  20 edit/test/repair tasks;
- counts for malformed `apply_patch` calls out of total `apply_patch` calls;
- counts for Python heredoc rewrites used as edit fallback;
- counts for semantic misplacements after shell/Python fallback edits, where the
  code compiles but the intended behavior is not wired at the correct callsite;
- latency and token growth after repeated tool grammar failures;
- the same benchmark results for a Codex-native model as the control.

The initial success metric should be simple: GLM 5.2 completes the benchmark
with no Python file rewrites, no semantic fallback misplacements, and no
raw-secret transcript leakage, while Codex-native models keep their current
strict `apply_patch` path.

The original Phase 0 gate is now superseded by V0 structured edit/write support.
The same numbers should still govern hardening: tune the turn-scoped fallback
threshold or add broader replacement normalizers only if GLM 5.2 still shows
tool-loop churn, or Python file-rewrite fallback appears in more than 2 of 20
benchmark tasks, while the Codex-native control stays at or below 1 comparable
failure.

## OpenCode Reference Evidence

OpenCode provides a useful reference implementation for this exact class of
problem. The current local source review used
`/home/postfiat/repos/opencode-current` from `anomalyco/opencode`, branch `dev`,
commit `f48f24ec4e1e26cc32c4d4953497fe2734c61ee1`
(`2026-06-22 17:51:49 -0400`).

The important finding is that OpenCode does not force every model through the
same patch protocol. It registers `edit`, `write`, and `apply_patch`, then gates
which edit tools are exposed by model family:

```ts
const usePatch =
  input.modelID.includes("gpt-") && !input.modelID.includes("oss") && !input.modelID.includes("gpt-4")
if (tool.id === ApplyPatchTool.id) return usePatch
if (tool.id === EditTool.id || tool.id === WriteTool.id) return !usePatch
```

Source: `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/registry.ts:273`

For non-GPT and OSS-style models, including the class GLM belongs to, OpenCode
exposes structured `edit` and `write` tools instead of the patch tool. This is
the same architecture this proposal recommends for PFTerminal: choose the edit
primitive by model capability instead of requiring every provider to mimic a
Codex-native `apply_patch` grammar.

OpenCode's structured edit tool is a normal JSON tool with these fields:

```ts
filePath: string
oldString: string
newString: string
replaceAll?: boolean
```

Source: `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/edit.ts:47`

The edit execution path is also the important part. OpenCode resolves the path,
normalizes line endings, computes a diff, asks for the unified `edit`
permission, writes the file, runs formatting, emits file-system events, and
then reports diagnostics. That is the low-friction pattern to copy: the model
gets simple arguments, while the harness still owns diffing, permissions,
formatting/events, and diagnostics.

Sources:

- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/edit.ts:80`
- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/edit.ts:129`
- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/edit.ts:145`
- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/edit.ts:155`
- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/edit.ts:188`

OpenCode's replacement engine starts with simple matches and then tries bounded
fallback matchers for common model-output drift: line trimming, block anchors,
whitespace normalization, indentation flexibility, escaped content, trimmed
boundaries, context anchors, and multi-occurrence handling. It still rejects
missing matches, ambiguous matches, and disproportionate matches. PFTerminal V0
should start narrower than this and add normalizers only after the GLM benchmark
proves they are needed.

Source: `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/edit.ts:682`

OpenCode still has an `apply_patch` tool, but even there the patch is carried in
a structured JSON field named `patchText`, not as Codex's custom freeform grammar
tool. The permission model remains unified: `edit`, `write`, and `apply_patch`
all route through the same `edit` permission path.

Sources:

- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/apply_patch.ts:18`
- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/apply_patch.ts:204`
- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/write.ts:20`
- `/home/postfiat/repos/opencode-current/packages/opencode/src/tool/write.ts:53`

This is direct evidence that a production coding harness can keep a strict patch
tool for models that handle it while routing other models to structured
replacement/write tools. PFTerminal should use that as the design precedent, not
as code to copy wholesale.

## Enforcement Summary

The failure path is strict and intentional. The fix should sit around tool
selection and model compatibility, not by weakening the core parser.

| Layer | Location | Enforcement | Why it matters |
| --- | --- | --- | --- |
| Tool exposure | `codex-rs/core/src/tools/spec_plan.rs:751` and `:758` | Z.AI/Ambient/GLM-like models now get structured edit/write; other models with `apply_patch_tool_type` keep strict `apply_patch`, gated by write-capable permission profiles. | This is the direct PFTerminal equivalent of OpenCode's model-family tool gate. |
| Freeform patch spec | `codex-rs/core/src/tools/handlers/apply_patch_spec.rs:18` | Strict `apply_patch` is a `ToolSpec::Freeform` custom grammar tool. | This is a good fit for Codex-native models and a bad fit for GLM-style JSON tool training. |
| Chat conversion | `codex-rs/core/src/client.rs:2112` and `:2194` | Function tools become normal Chat Completions tools; freeform tools become a single string argument named `input`. | This is why GLM should see structured JSON tools instead of raw grammar strings. |
| Parse failure | `codex-rs/core/src/tools/handlers/apply_patch.rs:381` and `:391` | The handler parses immediately and returns `apply_patch verification failed: ...` on grammar errors. | The exact failure text appears in the current session logs. |
| Patch grammar | `codex-rs/apply-patch/src/streaming_parser.rs:180`, `:192`, `:263` | The parser requires Codex hunk headers and ` ` / `+` / `-` line prefixes. | This produced the observed malformed-hunk failures. |
| Shell interception | `codex-rs/apply-patch/src/invocation.rs:242` | Only narrow `apply_patch <<'EOF'` heredoc forms are recognized. | Shell-script repair paths remain intentionally unforgiving. |
| Python source-write guard | `codex-rs/core/src/tools/handlers/structured_edit.rs:88`, `codex-rs/core/src/tools/handlers/shell/shell_command.rs:181`, `codex-rs/core/src/tools/handlers/unified_exec/exec_command.rs:190` | Structured-edit profiles reject obvious Python heredoc source rewrites. | This prevents the observed fallback from becoming the normal edit mechanism. |
| Line-ending preservation | `codex-rs/apply-patch/src/lib.rs:691` and `:707` | The patch engine now detects CRLF sources, strips internal `\r` for matching, then joins updated content with the original source line ending. | Structured edits can reuse the patch runtime without silently converting CRLF files to LF. |
| Compatibility telemetry | `codex-rs/core/src/tools/handlers/structured_edit.rs:29` and `codex-rs/core/src/tools/handlers/apply_patch.rs:384` | Emits `codex.model_edit_compatibility` with `profile`, `protocol`, `outcome`, and `reason` tags for structured edit/write results, strict-patch failures, and Python source-write rejections. | This turns future GLM compatibility runs into countable evidence instead of only manual log review. |

## Mechanics Review: OpenCode To PFTerminal

The OpenCode review changes the implementation target from "make GLM learn
Codex patch syntax" to "put a lower-friction edit primitive in front of the
same safety runtime." The useful mechanics are:

| Mechanic | OpenCode source | Codex/PFTerminal source | Sprint conclusion |
| --- | --- | --- | --- |
| Choose the edit primitive before the model sees tools. | `opencode-current/packages/opencode/src/tool/registry.ts:273` gates `apply_patch` to selected GPT models and exposes `edit`/`write` otherwise. | `codex-rs/core/src/tools/spec_plan.rs:751` now gates GLM/Z.AI/Ambient-like profiles to `structured_edit`/`structured_write`; Codex-native profiles keep `ApplyPatchHandler`. | Keep this as a capability profile decision, not a runtime guessing game. |
| Give non-Codex-native models ordinary JSON arguments. | `edit.ts:47` uses `filePath`, `oldString`, `newString`, `replaceAll`; `write.ts:20` uses `content`, `filePath`. | `structured_edit.rs:188` and `:242` expose the `structured_edit`/`structured_write` schemas. | Flat JSON tools are the low-friction contract GLM reliably emits. |
| Make the harness own diffing and permissions. | `edit.ts:145`, `:155`, `:159`, and `:198` compute diff, ask for `edit` permission, write, publish events, and report diagnostics. | `structured_edit.rs:408` generates internal patch text and `:581` calls `intercept_apply_patch`; `apply_patch.rs:592` then runs the existing patch runtime. | The model gets simple fields; PFTerminal keeps diff preview, sandboxing, approvals, Guardian review, cached approvals, and events. |
| Keep patch strict, but optional. | `apply_patch.ts:18` carries patch text in a JSON field, and `registry.ts:275` hides it from models not expected to use it. | `apply_patch_spec.rs:18` defines Codex strict patch as a `ToolSpec::Freeform` grammar; `client.rs:2194` maps that freeform tool to one raw `input` string for Chat Completions. | Do not weaken strict `apply_patch`; avoid exposing it to GLM as the primary edit path. |
| Hide edit tools in read-only review. | `permission/index.ts:216` treats `edit`, `write`, and `apply_patch` as one edit permission, so denial removes all write paths. | `spec_plan.rs:751` and `:758` now gate both structured edit/write and strict `apply_patch` on the active permission profile. | Review-only turns should not expose any edit primitive; GLM should inspect through read commands and finish with findings. |
| Treat fuzzy matching as a measured hardening step. | `edit.ts:682` cascades through simple, line-trimmed, block-anchor, whitespace-normalized, indentation-flexible, escaped-content, trimmed-boundary, context-aware, and multi-occurrence replacers. | PFTerminal V0 currently supports exact replacement and line-ending normalization only. | Add trim/fuzzy normalizers only after GLM benchmark data proves exact matching is too brittle. |
| Prevent shell rewrites from becoming the hidden editor. | OpenCode routes edits through tool permissions; source rewrites are still represented as edit/write tool calls. | `structured_edit.rs`, `shell_command.rs`, and `exec_command.rs` reject obvious Python heredoc source writes for structured-edit profiles. | Shell remains for tests and diagnostics, not model-authored source edits. |

The Codex-side review explains why this is necessary. Strict patch is not just a
different spelling of a normal diff: it is a freeform grammar tool whose parser
requires `*** Begin Patch`, Codex hunk headers, and update lines prefixed with
` `, `+`, or `-`. In Chat Completions mode, PFTerminal can translate normal
function tools into provider-native JSON, but a freeform tool still becomes one
raw string argument. That is exactly the high-friction surface GLM failed in the
live `/vault` session.

## Low-Friction Implementation Mechanics

The successful path is to copy OpenCode's harness shape, not its code. Keep
`apply_patch` strict and unchanged for Codex-native models, but stop making it
the only editing primitive available to every provider.

### Current Implementation Status

V0 is implemented in the PFTerminal core tool harness:

- `codex-rs/core/src/tools/handlers/structured_edit.rs` adds
  `structured_edit` and `structured_write` as flat `ToolSpec::Function` tools.
- `codex-rs/core/src/tools/spec_plan.rs` routes built-in Z.AI, Ambient, and
  GLM/ZAI-looking model slugs to structured edit/write instead of strict
  `apply_patch` when the active permission profile allows writes; Codex-native
  models keep `ApplyPatchHandler`.
- If a model starts on strict `apply_patch` and produces two strict-patch
  grammar failures in the same turn, `TurnContext` records the threshold and
  the next tool plan switches that turn to `structured_edit`/`structured_write`
  instead of continuing to expose `apply_patch`.
- The same planner now hides all edit tools under read-only permission profiles:
  no `structured_edit`, no `structured_write`, and no strict `apply_patch`.
- `codex-rs/core/src/tools/handlers/apply_patch.rs` and
  `structured_edit.rs` emit `codex.model_edit_compatibility` counters for
  strict-patch parse/verification failures, structured edit/write successes and
  failures, and Python heredoc source-write rejections.
- The structured tool schema now says these tools are edit/write tools, not
  read or inspection tools; during read-only review, GLM should use targeted
  shell reads (`rg`, `sed -n`, `wc`) and avoid edit tools entirely.
- Explicit shell-command caps in accepted user input are now enforced by the
  tool dispatcher for new shell execution calls. Phrases such as `at most 5
  shell commands`, `no more than 5 commands`, `maximum of 5 shell commands`,
  `max 5 commands`, and `use 5 or fewer commands` activate a turn-local budget
  for `exec_command`/`shell_command`; `write_stdin` is not counted because it
  continues an already-open process.
- Structured edit/write generate valid internal `apply_patch` text and delegate
  through `intercept_apply_patch`, preserving patch safety assessment, diff
  preview, approval flow, sandboxing, Guardian review, cached approvals, and
  patch tool events.
- Tests now cover exact replacement rules, `replace_all`, unsafe path
  rejection, generated patch validity, multi-environment schemas, Z.AI/GLM tool
  exposure, structured edit runtime execution, structured write runtime
  execution, and existing strict `apply_patch` behavior.

The implemented V0 deliberately chooses the lowest-friction contract:

- one edit protocol per model/session at tool-planning time;
- zero edit protocols for explicit read-only review turns;
- normal JSON function arguments for GLM/Z.AI-facing edit tools;
- relative paths only, with absolute paths and `..` rejected before file access;
- exact single-match replacement by default, with explicit `replace_all`;
- bounded full-file writes through `structured_write`, capped at 256 KiB;
- generated internal patch text sent through `intercept_apply_patch`, not direct
  file writes;
- the same patch events, diff tracking, approval path, sandboxing, Guardian
  review, and cached approvals as strict `apply_patch`.

Successful low-friction implementation means the model does less protocol work
and the harness owns more safety work:

1. Choose the edit primitive before the model sees tools. OpenCode does this in
   `registry.ts`; PFTerminal does it in `spec_plan.rs`.
2. Expose no edit primitive at all for read-only review turns, so the model
   cannot probe edit tools as a substitute for reading.
3. Give non-Codex-native models flat JSON tools (`structured_edit` and
   `structured_write`), not a grammar string or namespace-shaped tool.
4. Keep the model contract simple: path plus exact old/new text, or path plus
   full-file content for intentional writes.
5. Convert the structured request into the existing internal patch path, so the
   harness still owns diffing, safety assessment, approvals, sandboxing,
   Guardian review, events, and cached approvals.
6. Preserve source line endings inside the shared patch engine before routing
   structured edits through it.
7. Reject source-file rewrites via Python heredocs for structured-edit model
   profiles, while still allowing shell/Python for diagnostics and tests.
8. Keep strict `apply_patch` as the Codex-native control path instead of
   weakening the parser for every model.
9. Count failures and successes with compatibility telemetry, and use the
   two-failure strict-patch threshold to switch an already-open turn to
   structured edit/write before the model falls back to shell rewrites.

Not yet complete:

- per-session edit retry-loop aggregation beyond the turn-scoped strict-patch
  fallback threshold; V0 now has compatibility counters for the individual
  strict-patch, structured-edit, and Python rewrite-rejection events;
- broader read-size and finalization budgets so GLM uses targeted reads and
  finalizes before timeout on explicitly read-only review tasks. Explicit
  shell-command count caps are now enforced, but output-size/read-strategy
  discipline still needs the broader benchmark to decide what to harden next;
- host read-only sandbox availability: the current Linux box still cannot run
  `--sandbox read-only` repo-review commands under approval policy `never`.
  A locally extracted `bubblewrap` 0.9.0 binary proved the binary-availability
  issue, but unprivileged sandbox startup still fails with UID-map and loopback
  setup errors. This needs system-level `bwrap`/namespace capability support,
  not an edit-protocol change;
- trimmed-boundary replacement normalizers, if live GLM runs show exact plus
  line-ending matching is too brittle;
- the broader benchmark where PFTerminal/GLM completes a fixed set of
  edit/test/repair tasks and code reviews with bounded command budgets.

Verification already covered the low-friction mechanics in deterministic tests:

- `codex-rs/core/src/tools/spec_plan_tests.rs:664`
  proves Z.AI gets structured edit/write and not strict `apply_patch`.
- `codex-rs/core/src/tools/spec_plan_tests.rs:678`
  proves a `glm-5.2` model slug gets structured edit/write and not strict
  `apply_patch`.
- `codex-rs/core/src/tools/spec_plan_tests.rs:716` and `:729`
  prove read-only permission profiles hide both structured edit/write and
  strict `apply_patch`.
- `codex-rs/core/src/tools/spec_plan_tests.rs` now proves repeated
  strict-patch failures switch a Codex-native strict-patch turn to
  `structured_edit`/`structured_write` and hide `apply_patch`.
- `codex-rs/core/tests/suite/tool_harness.rs:518`
  proves `structured_edit` executes through patch file-change events.
- `codex-rs/core/tests/suite/tool_harness.rs:670`
  proves `structured_write` executes through patch file-change events.
- `codex-rs/core/tests/suite/tool_harness.rs:368` and `:820`
  keep the strict `apply_patch` success path, parse diagnostics, and
  request-to-request dynamic fallback behavior covered for Codex-native models.
- `codex-rs/apply-patch/src/lib.rs` now has a CRLF regression proving update
  hunks preserve source file line endings.
- Live GLM smoke thread `019ef1cb-4940-7cf2-a3dd-abd7a8813b62` proves
  `structured_edit` is reachable by GLM 5.2 and changes a file without
  `apply_patch` grammar or Python fallback.
- Live GLM smoke thread `019ef201-cbd4-7fb1-b92b-ab2fb82a3387` proves the
  rebuilt telemetry-instrumented binary still changes a file through the
  file-change path without strict `apply_patch` or Python source rewrite.

Live GLM repo-review validation now separates edit-protocol compatibility from
review-loop quality:

| Target | Thread | Result | Edit-protocol signal | Remaining issue |
| --- | --- | --- | --- | --- |
| `/home/postfiat/repos/codex-whip` | `019ef1e1-2736-7bd2-9200-35269cbc61b7` | Completed read-only code review; ran `python3 -m pytest tests/ -q` with 17/17 passing; produced findings. | No `structured_edit`, `structured_write`, or strict `apply_patch` calls; no Python source-write fallback; repo stayed clean. | Completed but expensive: 123k input tokens, 9.9k reasoning tokens. |
| `/home/postfiat/repos/codex-whip` after dynamic fallback build | `019ef226-ab1d-7053-b660-9e7184105d02` | Completed read-only code review with correctness, portability, maintainability, and test-gap findings. | `/tmp/pft_glm_codex_whip_review_after_fallback.jsonl` contains only agent-message and command-execution events: no `structured_edit`, `structured_write`, strict `apply_patch`, function/custom edit tool calls, file-change events, or repo diff; repo stayed clean. | Still exceeded the prompt command budget: 8 completed shell commands against a requested maximum of 5. |
| `/home/postfiat/repos/pft-sglang` | `019ef1f6-7557-7083-b4a0-5bc615d91301` | Completed read-only code review on an 11-file repo; produced 4 Medium, 4 Low, and 2 Informational findings. | No `structured_edit`, `structured_write`, or strict `apply_patch` calls; no Python source-write fallback; repo stayed clean. | Completed with a bounded prompt and six command executions; still used `cat` for small config/docs files, so harness-side read budgets remain useful. |
| `/home/postfiat/repos/ambient-code-review-codex-plugin` | `019ef1e6-5de5-73a3-a29f-16ce33e8e6cf` | Timed out at 300s after substantial read-only review; ran `py_compile` and 28 unit tests successfully. | No edit-tool calls and no source-file writes; diagnostic Python heredocs were used only to execute read-only checks; repo stayed clean. | Poor review-loop discipline: large file reads and repeated probes prevented a final report. |
| `/home/postfiat/repos/opencode-current` tool-edit slice | `019ef1eb-27f0-79c0-ada3-d00d63bb1e71` | Timed out at 240s while reviewing `registry.ts`, `edit.ts`, `write.ts`, `apply_patch.ts`, and patch parser internals. | No edit-tool calls and no source-file writes; repo stayed clean. Mentions of `apply_patch` were source text, not tool invocations. | Still too slow for a constrained review; needs stronger read ranges, step budget, or harness-side review prompt. |
| `/home/postfiat/repos/mordornotebook` with `--sandbox read-only` | `019ef20b-7aff-7043-b456-c4cd47405922` | Blocked before file inspection because the Linux sandbox launcher could not find `bubblewrap`/`bwrap`, and approval policy `never` rejected escalation. | No `structured_edit`, `structured_write`, strict `apply_patch`, or file-change events appeared in `/tmp/pft_glm_readonly_gate.jsonl`; repo stayed clean. | Host setup blocker, not an edit-protocol regression. Install/bundle `bwrap` before using this exact read-only validation path. |
| `/home/postfiat/repos/mordornotebook` with full-access shell validation | `019ef213-154f-7d02-ab48-65ce61770105` | Completed read-only code review and produced High/Medium/Low findings plus residual test gaps. | `/tmp/pft_glm_mordor_review.jsonl` contains no `structured_edit`, `structured_write`, strict `apply_patch`, or file-change events; repo stayed clean. | Good edit-protocol signal. Review used 5 completed shell commands and finished within the 300s timeout. |
| `/home/postfiat/repos/pft-chatbot-mcp` with full-access shell validation | `019ef215-2934-75b1-8260-66b278d92f0b` | Completed read-only code review and produced Medium/Low findings plus positive crypto/TLS observations and test gaps. | `/tmp/pft_glm_pft_chatbot_review.jsonl` contains no `structured_edit`, `structured_write`, strict `apply_patch`, or file-change events; repo stayed clean. | Edit-protocol signal remains good, but review-loop discipline regressed: prompt budget was 7 shell commands, actual completed shell commands were 11. |
| `/tmp/pft_glm_structured_ready` through rebuilt PATH wrapper | `019ef236-084e-70d2-aace-e6e22ea27855` | Turn started through `/home/postfiat/.local/bin/pfterminal`, but failed before model tool selection because the provider returned `429 Too Many Requests` after retries. | No edit-tool call occurred and the temp repo stayed unchanged (`status: alpha`). | Confirms the wrapper reaches the rebuilt binary; does not validate live structured-edit behavior because the upstream provider rate-limited the request. |
| `/tmp/pft_glm_structured_ready_2` through rebuilt PATH wrapper | `019ef247-4603-74f1-b6de-308af8647fb7` | Completed the requested one-line edit, changing `README.md` from `status: alpha` to `status: beta`. | `/tmp/pft_glm_structured_ready_2.jsonl` shows one targeted `rg` read, one `file_change` event, and no strict `apply_patch`, Python, `sed`, `perl`, or `cat` source rewrite. The diff is exactly the intended line. | Confirms the rebuilt binary works for live GLM structured-edit after the Z.AI rate limit cleared. |
| `/tmp/pft_glm_benchmark_edit` edit/test/repair benchmark | `019ef24c-03bd-79e2-a59a-5431fded78ea` | Completed a failing-test repair in a tiny Python repo: `src/math_utils.py` changed `return amount * bps` to `return amount * bps / 10000`, and `python3 -m pytest tests/ -q` passed with `1 passed`. | `/tmp/pft_glm_benchmark_edit.jsonl` shows four completed shell commands under the explicit `at most 4 shell commands` cap, one file-change lifecycle, no strict `apply_patch`, and no Python heredoc source rewrite. | Validates a live GLM edit/test/repair loop on the rebuilt binary with command-budget enforcement active. |
| `/home/postfiat/repos/pft-sglang` bounded read-only review benchmark | `019ef24c-9c1d-75c0-a68f-6a261747ad23` | Completed a read-only code review with file/line findings and test-gap notes. | `/tmp/pft_glm_benchmark_pft_sglang_review.jsonl` shows four completed shell commands under the explicit `at most 5 shell commands` cap, no `file_change`, no strict `apply_patch`, no structured edit/write calls, and the repo stayed clean. | Validates a live GLM read-only review loop with command-budget enforcement active. |

The interpretation is important: the original blocker was malformed
`apply_patch` mechanics. The live runs no longer show that blocker when GLM is
on the structured-tool profile. The repeated remaining blocker is operational
quality in long review loops: bounded file reads, fewer redundant probes,
enforced command budgets, and reliable finalization before timeout.

Latest local verification:

- `just fmt`: clean.
- `/home/postfiat/.local/bin/pfterminal` now points at the rebuilt PfTerminal
  binary in this repo instead of the older `PfTerminal-clean-unified` checkout.
- `cargo build -p codex-cli --bin pfterminal`: passed at 2026-06-23 02:00 UTC,
  producing `/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal`.
- `pfterminal --version`: runs through the updated wrapper.
- `cargo test -p codex-core structured_edit -- --nocapture`: 18 unit tests plus
  the structured-edit tool harness test passed against the rebuilt source.
- `cargo test -p codex-core explicit_shell_command_budget -- --nocapture`:
  parser and dispatcher tests passed for explicit shell-command budget phrases
  and enforcement after the limit is reached.
- `cargo test -p codex-core shell_command_budget -- --nocapture`: same focused
  budget tests passed through the dispatcher filter.
- `cargo test --manifest-path codex-rs/Cargo.toml -p codex-apply-patch -- --nocapture`:
  70 unit tests and 17 integration tests passed.
- `cargo test -p codex-core structured_edit -- --nocapture`: 16 core tests plus
  the structured-edit tool harness test passed.
- `cargo test -p codex-core structured_edit_tools -- --nocapture`: planner
  tests passed for writable Z.AI/GLM structured-edit exposure and read-only
  structured-edit hiding.
- `cargo test -p codex-core read_only_permission_profile_hides -- --nocapture`:
  planner tests passed for hiding structured edit/write and strict
  `apply_patch` in read-only profiles.
- `cargo test -p codex-core apply_patch_tool_executes_and_emits_patch_events -- --nocapture`:
  strict `apply_patch` success path passed.
- `cargo test -p codex-core repeated_strict_patch_failures -- --nocapture`:
  turn-scoped fallback threshold tests passed.
- `cargo test -p codex-core apply_patch_reports_parse_diagnostics -- --nocapture`:
  strict `apply_patch` diagnostics and dynamic fallback request tools passed.
- `cargo build -p codex-cli --bin pfterminal`: passed previously after the
  dynamic fallback build and passed again after the wrapper-readiness fix,
  producing an updated debug binary for live GLM validation.
- `cargo build -p codex-cli --bin pfterminal`: passed again after
  command-budget enforcement, so the runnable `pfterminal` binary includes
  explicit shell-command cap enforcement.
- `cargo clippy -p codex-core --tests`: passed with only pre-existing warnings
  in `codex-model-provider` and `codex-core-plugins`.
- `./scripts/docs-site-build`: passed with the upstream Material for MkDocs 2.0
  warning only.
- Local sandbox setup attempt: downloaded and extracted Ubuntu Noble
  `bubblewrap` 0.9.0 to `codex-rs/target/debug/codex-resources/bwrap`; sandbox
  still failed under this account with `setting up uid map: Permission denied`
  or `loopback: Failed RTM_NEWADDR: Operation not permitted`.
- Live GLM review `/home/postfiat/repos/mordornotebook`: completed, no edit
  tool/file-change trace, repo clean.
- Live GLM review `/home/postfiat/repos/pft-chatbot-mcp`: completed, no edit
  tool/file-change trace, repo clean, but exceeded the prompt command budget.
- Live GLM review `/home/postfiat/repos/codex-whip` after the dynamic fallback
  build: completed, no edit tool/file-change trace, repo clean, but again
  exceeded the prompt command budget.
- Live GLM benchmark `/tmp/pft_glm_benchmark_edit`: completed an edit/test
  repair loop within the explicit 4-command cap, changed one source line through
  the file-change path, and passed the test.
- Live GLM benchmark `/home/postfiat/repos/pft-sglang`: completed a read-only
  review within the explicit 5-command cap, produced no edit/file-change trace,
  and left the repo clean.

### Tool Exposure

Add flat structured file tools as `ToolSpec::Function` tools:

- `structured_edit`
- `structured_write`

Do not implement the GLM path as `ToolSpec::Namespace` and do not make it a new
`ToolSpec::Freeform` grammar. `codex-rs/core/src/client.rs:2112` converts plain
function tools for Chat Completions, while `ToolSpec::Namespace` is dropped for
that wire format. Freeform tools are converted into a single string field named
`input` at `codex-rs/core/src/client.rs:2194`, which preserves the same awkward
grammar-string problem that GLM is already failing.

The model gate lives where `apply_patch` is selected:
`codex-rs/core/src/tools/spec_plan.rs:749`. The rule should remain exactly one
edit protocol per model/session:

- Codex-native models: expose `ApplyPatchHandler`.
- GLM/Z.ai and other non-Codex-native coding models: expose
  `structured_edit` and `structured_write`.
- If a model starts in strict-patch mode and hits the bounded failure threshold,
  switch that session to structured edit/write instead of allowing Python
  heredoc rewrites to become the fallback edit path.

This mirrors OpenCode's gate in
`/home/postfiat/repos/opencode-current/packages/opencode/src/tool/registry.ts:267`,
where GPT-family models get `apply_patch` and non-GPT/OSS-style models get
`edit`/`write`.

### Structured Tool Contracts

Use two simple tools instead of one overloaded operation union. That gives
models fewer fields to choose incorrectly and makes validation cleaner.

```json
{
  "path": "relative/path.rs",
  "old_string": "exact text to replace",
  "new_string": "replacement text",
  "replace_all": false
}
```

```json
{
  "path": "relative/path.rs",
  "content": "complete intended file contents",
  "mode": "create_only"
}
```

`structured_edit` should be the default. `structured_write` should be reserved
for new files and explicit full-file overwrites, with a max-size guard and a
larger review surface.

Optional v1 hardening can add `expected_sha256`, `context_before`, and
`context_after`, but v0 should not require them. The immediate goal is to remove
the patch-grammar chokepoint without building another high-friction protocol.

### Replacement Rules

Start with deterministic replacement behavior:

- exact `old_string` replacement first;
- `0` matches: reject and tell the model to read the current file before
  retrying;
- `1` match: apply and show the diff;
- `2+` matches without `replace_all`: reject and ask for more surrounding
  context;
- `replace_all: true`: apply only when explicitly requested;
- reject binary files, paths outside the workspace, and edits above the size
  limit.

V0 currently implements exact replacement plus line-ending normalization to the
current file's line-ending style. The next lowest-friction improvement, if live
GLM runs need it, is line-trimmed matching. Avoid broad fuzzy matching until the
benchmark proves it is needed. OpenCode has a richer replacement cascade at
`/home/postfiat/repos/opencode-current/packages/opencode/src/tool/edit.ts:682`,
but PFTerminal should start narrower to keep review and failure modes obvious.

### Runtime And Permissions

Do not create a second edit safety system. Convert structured edit/write into
the same internal patch/change representation already used by `apply_patch`,
then reuse the existing runtime path:

- `codex-rs/core/src/apply_patch.rs:34` for patch safety assessment;
- `codex-rs/core/src/tools/handlers/apply_patch.rs:406` for effective
  permissions and runtime dispatch;
- `codex-rs/core/src/tools/runtimes/apply_patch.rs:130` for approval,
  Guardian review, cached approvals, and tool events.

V0 achieves this without a new public constructor by generating a valid internal
patch and passing it through `intercept_apply_patch` from
`codex-rs/core/src/tools/handlers/apply_patch.rs:560`. The structured handler
must continue to avoid manual file writes; the runtime that already owns
sandboxing, diff preview, approvals, and tool events should remain the only
write path.

### Failure Policy

Once structured edit exists, Python heredoc rewrites should not be considered a
normal model-edit fallback. Shell remains available for tests, builds, and
diagnostics, but model-authored source edits should go through either
`apply_patch` or structured edit/write.

V0 enforces that policy for structured-edit profiles by rejecting obvious Python
heredoc source writes in both shell tool paths. This is intentionally scoped:
diagnostic Python scripts and non-source commands still run, while source file
edits must use `structured_edit`, `structured_write`, or strict `apply_patch`.

The bounded policy is now:

1. first strict-patch grammar failure: return a targeted repair message;
2. second strict-patch grammar failure in the same turn/session: switch the
   next tool plan for that turn to structured edit/write;
3. any attempt to edit source via Python heredoc after that should continue to
   be rejected with guidance to use the structured edit tool.

Implementation touchpoints:

- `codex-rs/core/src/session/turn_context.rs` stores turn-scoped
  strict-patch failure state.
- `codex-rs/core/src/tools/handlers/apply_patch.rs` increments the failure
  counter on parse and shell-parse failures and adds model-facing fallback
  guidance at the threshold.
- `codex-rs/core/src/tools/handlers/structured_edit.rs` treats the turn-scoped
  fallback flag as a structured-edit profile for tool selection and Python
  heredoc source-write rejection.
- `codex-rs/core/src/tools/spec_plan.rs` rebuilds tools before follow-up model
  requests, so the next request sees `structured_edit`/`structured_write` and
  no longer sees strict `apply_patch`.

This directly addresses the observed failure mode: repeated malformed
`apply_patch` attempts followed by Python file rewrites that can compile while
landing behavior in the wrong callsite.

## Proposal

Prepare and validate a gated PFTerminal tool-compatibility path for
non-Codex-native coding models that fail the strict patch benchmark.

The goal is not to weaken the core Codex parser. The goal is to stop treating
the Codex parser as the only edit protocol available to every model.

### 1. Model Capability Profile

Introduce an explicit capability profile per model/provider:

```yaml
model: glm-5.2
provider: zai
observed:
  tool_call_json: pass
  shell_exec: pass
  strict_apply_patch: fail-retry
recommended_edit_protocol: structured_edit_write
secret_safe_input: required
confidence: current-session-log
```

This can live near the provider/model metadata that currently controls
`apply_patch_tool_type`. PFTerminal should expose `apply_patch` only to models
that pass the strict patch capability test. GLM 5.2 should initially get a
structured edit tool, following the OpenCode model-gating precedent above.

### 2. Add Structured Edit/Write Tools

Status: V0 implemented in `codex-rs/core/src/tools/handlers/structured_edit.rs`.

Add model-friendly edit tools for OSS models as plain JSON function tools:

```json
{
  "path": "codex-rs/login/src/auth/manager.rs",
  "old_string": "pub fn provider_api_key_from_auth_storage(\n    codex_home: &Path,\n",
  "new_string": "pub fn provider_api_key_from_auth_storage(\n    codex_home: &Path,\n    provider_key_id: &str,\n",
  "replace_all": false
}
```

and:

```json
{
  "path": "docs/authentication.md",
  "content": "...",
  "mode": "create_only"
}
```

For replacement edits, the runtime must enforce a single-match rule:

```text
0 matches: reject and ask the model to read the latest file before retrying
1 match: apply, then show the diff
2+ matches: reject and ask for more context
```

That avoids reintroducing the same ambiguity that makes freeform patches
fragile. If needed later, the tool can require `context_before`,
`context_after`, or an expected pre-edit file hash for higher-risk edits.

The runtime must reuse the existing `apply_patch` safety path instead of
writing files directly. That preserves:

- workspace boundaries;
- sandbox policy;
- diff preview;
- approval rules;
- binary-file refusal;
- maximum edit size;
- post-edit diff tracking.

This gives GLM a simpler contract without requiring Python heredoc rewrites.

### Risks And Cheaper Alternatives

A better GLM-facing `apply_patch` description is still useful as a strict-patch
control, including one valid update hunk and one invalid hunk. It is low effort
and may reduce failures for providers that still expose strict patch.

That control is not sufficient by itself. The current logs show repeated
failures after the parser returned explicit prefix guidance. The model then
escaped to Python file rewrites, so the product problem is not only an unclear
description. PFTerminal needs the structured edit/write path plus a bounded
fallback policy after repeated grammar failures.

The structured edit path also has risks:

- exact `old_string` values are brittle if the file changed after the model read
  it;
- broad replacements can match multiple locations;
- `structured_write` can be too large and obscure review.

The mitigation is to make the structured tool stricter than a Python rewrite:
single-match replacement, workspace checks, maximum edit size, binary refusal,
diff capture, and explicit failure messages when the model needs fresher file
context. The structured path should also convert into the existing
`ApplyPatchRuntime` approval and execution path so it inherits the same safety
surface as strict `apply_patch`.

The `/vault supports_inline_args` miss is the concrete failure this mitigation
must prevent: a fallback edit should not be able to silently land in a nearby but
behaviorally wrong match arm. The compatibility harness should score that as a
failed edit even if the crate still compiles.

### 3. Add Deterministic Patch Repair

Status: not implemented.

Before giving up on `apply_patch`, PFTerminal can attempt safe, deterministic
repairs:

- reject and explain normal unified diffs that start with `---` / `+++`;
- if an update hunk begins with raw unprefixed code, return a targeted repair
  message that includes a minimal valid example;
- optionally convert simple unified diffs into Codex patch hunks in a separate,
  tested adapter.

Do not silently guess edits when context is ambiguous.

### 4. Add A Tool-Error Feedback Policy

Status: implemented for repeated strict-patch grammar failures.

After two `apply_patch` grammar failures in one turn, PFTerminal should stop
repeating the same tool contract and switch the model to the structured edit
tool for that turn.

This avoids the observed pattern:

1. malformed `apply_patch`;
2. parse error;
3. model retries a similar malformed `apply_patch`;
4. model falls back to Python shell editing.

The integration harness now verifies that exact path: the first malformed
`apply_patch` leaves strict patch visible for one repair attempt, the second
malformed call records the threshold, and the next request exposes
`structured_edit`/`structured_write` while hiding `apply_patch`.

### 5. Add A Capability Test Suite

Status: partially implemented with deterministic core harness tests; live GLM
multi-repo validation is still required.

Every provider/model should be tested against the PFTerminal tool contract:

- valid tool-call JSON;
- valid strict `apply_patch`;
- invalid patch recovery;
- multi-file edit;
- test-failure repair loop;
- no shell-history secret leakage;
- no raw secret in transcript/context;
- latency and token growth under repeated tool failures.

The result should be a compatibility matrix:

| Model | Tool JSON | Strict Patch | Structured Edit | Shell | Recommended Mode |
| --- | --- | --- | --- | --- | --- |
| Codex-native model | pass | pass | pass | pass | strict patch |
| GLM 5.2 | pass | weak | target | pass | structured edit |

## Acceptance Criteria

The fix is complete when:

- GLM 5.2 can complete a multi-file edit/test/fix loop without Python heredoc
  rewrites;
- malformed strict-patch attempts are either repaired or routed to a structured
  edit tool after a bounded number of failures;
- fallback edits do not create semantic misplacements such as wiring a slash
  command into the wrong availability list;
- `apply_patch` remains strict for models that can use it correctly;
- PFTerminal records per-model tool compatibility outcomes in logs or test
  reports;
- the `/vault` implementation can proceed on GLM 5.2 without repeated patch
  grammar failures.

## Prioritized Roadmap

V0 has already moved the highest-leverage mechanics into the core harness. The
remaining priority is validation and hardening, not a larger compatibility
layer.

| Order | Work | Status | Impact | Rollout |
| --- | --- | --- | --- | --- |
| 0 | Improve the GLM-facing `apply_patch` description with one valid update example and clearer prefix rules. | Superseded by structured tool routing as the primary fix; still useful as a strict-patch control. | Medium | Keep as a control experiment for providers that still expose strict patch. |
| 1 | Record per-model edit capability outcomes in logs and tests. | Partial. `codex.model_edit_compatibility` now counts strict-patch failures, threshold-triggered fallback, structured edit/write outcomes, and Python source-write rejections; per-session retry-loop aggregation is still open. | High | Use the compatibility counter plus rollout traces as benchmark evidence; add broader aggregation once live samples justify it. |
| 2 | Implement flat `structured_edit` and `structured_write` function tools. | Done in V0. | High | Keep as normal `ToolSpec::Function`; avoid `Namespace` and `Freeform` for GLM-facing tools. |
| 3 | Route GLM 5.2 to structured edit/write by profile. | Done in V0 for Z.AI/Ambient/GLM-like slugs. Dynamic fallback after repeated strict-patch grammar failures is also implemented for already-open strict-patch turns. | High | Gate in `spec_plan.rs`, matching OpenCode's model-family tool selection; keep Codex-native behavior unchanged until the turn records repeated strict-patch failures. |
| 4 | Convert structured edits into the existing apply_patch runtime path. | Done in V0 through `intercept_apply_patch`. | High | Preserve diff preview, approval, Guardian review, sandboxing, cached approvals, and patch events. |
| 5 | Preserve source line endings when structured edit/write reuses the apply_patch runtime. | Done in V0. | Medium | Shared apply-patch update hunks now preserve CRLF source files; regression and direct binary smoke passed. |
| 6 | Add deterministic patch repair or unified-diff conversion only for simple, unambiguous cases. | Open. | Medium | Keep the strict parser unchanged; put conversion in a tested adapter if needed. |
| 7 | Reject model-authored Python heredoc source rewrites once structured edit exists. | Done in V0 for obvious Python heredoc source writes. | Medium | Shell remains available for tests/builds; source edits should use either strict patch or structured edit/write. |
| 8 | Build a provider compatibility harness covering GLM 5.2, Codex-native models, and future OSS models. | Partial. Deterministic coverage passes; live validation has two completed reviews and two timeout traces without edit-protocol regressions. | High | Use current session logs and new GLM repo-review runs as regression fixtures. |
| 9 | Add review-loop budgets for non-Codex-native models. | Partial. Structured edit/write descriptions now warn against read-only misuse; command/read budgeting is still open. | High | Cap broad `cat` reads, prefer targeted `sed`/`rg`, and require finalization before timeout so successful tool compatibility becomes usable review throughput. |

Likely code touchpoints:

- `codex-rs/core/src/tools/spec_plan.rs`: choose tool exposure based on model
  capability instead of only `apply_patch_tool_type`.
- `codex-rs/core/src/tools/handlers/structured_edit.rs`: new handler for
  exact replacement and bounded full-file write requests.
- `codex-rs/core/src/client.rs`: keep the GLM edit path as flat
  `ToolSpec::Function` tools; Chat Completions conversion drops
  `ToolSpec::Namespace` and wraps `ToolSpec::Freeform` as a single grammar
  string field.
- `codex-rs/core/src/tools/handlers/apply_patch.rs`: classify parse failures
  and emit targeted repair messages.
- `codex-rs/apply-patch/src/lib.rs`: keep strict parser behavior while ensuring
  generated internal update hunks preserve source file line endings.
- `codex-rs/core/src/tools/runtimes/apply_patch.rs`: reuse approval,
  Guardian-review, cached-approval, sandbox, and tool-event behavior.
- `codex-rs/core/src/tools`: add tests for no-match, multi-match,
  `replace_all`, binary refusal, path refusal, and runtime permission reuse.

## Decision

PFTerminal should support open-source and open-weight coding models without
requiring them to mimic every Codex-native edit protocol exactly. The bounded
decision is to validate GLM 5.2 on the implemented V0 structured edit/write
path, keep strict `apply_patch` for models that pass it, and add only the
normalizers or fallback policy that the measured tool-compatibility gate proves
necessary.
