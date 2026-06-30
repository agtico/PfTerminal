# Tool Call Runaway Remedy

## Complete

- [x] Audited the live `pfterminalworker` tmux failure.
- [x] Consulted local OpenCode, Hermes Agent, Kilo Code, and Cline source snapshots.
- [x] Identified the PFTerminal code path that turns malformed tool arguments into another model follow-up.
- [x] Defined the structural remedy before touching runtime code.

## To Do

- [x] Add a non-retriable malformed/truncated tool-call boundary in PFTerminal.
- [x] Stop persisting raw malformed oversized tool-call arguments into conversation history.
- [ ] Add bounded/chunked write mechanics for large generated files.
- [x] Add regression tests using the observed GLM `structured_write` failure shape.
- [ ] Re-run the StakeHub visual-mock task through GLM after the runtime fix.

## Problem Statement

The live `pfterminalworker` session is not blocked by tmux. It is stuck in a
PFTerminal tool-loop failure:

- The worker repeatedly says it will write
  `dashboard/shielded-navswap-flow-rail-mock.html`.
- It emits very large `structured_write` or `exec_command` tool calls.
- The tool arguments are truncated mid-string and fail JSON parsing with
  `failed to parse function arguments: EOF while parsing a string`.
- PFTerminal records that error as a normal tool result and asks the model for
  a follow-up.
- The model repeats the same large write attempt, generating another provider
  request and another malformed tool call.

Concrete rollout evidence from
`/home/postfiat/.pfterminal/sessions/2026/06/23/rollout-2026-06-23T21-22-52-019ef65d-5652-7252-934f-ba52cf91f2e6.jsonl`:

| Time UTC | Tool | Result |
| --- | --- | --- |
| `22:45` | `structured_write` | `EOF while parsing a string at line 1 column 12610` |
| `22:46` | `structured_write` | `EOF while parsing a string at line 1 column 12851` |
| `23:08` | `structured_write` | `EOF while parsing a string at line 1 column 12706` |
| `23:09` | `structured_write` | `EOF while parsing a string at line 1 column 13725` |
| `23:11` | `structured_write` | `EOF while parsing a string at line 1 column 12689` |
| `23:16` | `structured_write` | `EOF while parsing a string at line 1 column 12579` |

This is a harness failure. The runtime should not let malformed oversized tool
calls remain inside the normal retry loop.

## Current PFTerminal Mechanics

PFTerminal's Chat Completions adapter accumulates streamed tool-call argument
deltas into a string:

- `codex-rs/codex-api/src/endpoint/chat_completions.rs` records
  `finish_reason`, but it is currently unused.
- The same file appends `function.arguments` chunks into
  `tool_call.arguments`.
- At stream completion, it emits a `ResponseItem::FunctionCall` even if the
  tool argument was truncated by the provider or completion-token cap.

The tool handler path then parses that argument:

- `codex-rs/core/src/tools/handlers/mod.rs` maps JSON parse failure to
  `FunctionCallError::RespondToModel("failed to parse function arguments: ...")`.
- `codex-rs/core/src/stream_events_utils.rs` handles `RespondToModel` by
  recording a `FunctionCallOutput`, setting `needs_follow_up = true`, and
  continuing the model loop.
- The same error branch records the malformed response item, which means a huge
  broken tool call can be preserved into history and re-sent later.

`apply_patch` has a special failure counter that eventually switches to
structured edit/write. `structured_write` does not yet have an equivalent
non-retry boundary, so it can loop on malformed full-file writes.

## Alternative Agent Evidence

| Agent | Relevant mechanics | Lesson for PFTerminal |
| --- | --- | --- |
| OpenCode | Registers `edit`, `patch`, and `write` as separate tools. `edit` is exact `old_string` to `new_string`, while `patch` is optional rather than the only edit path. | Keep model-family-specific edit primitives. GLM-class models should not be forced into one strict grammar or one giant full-file JSON payload. |
| Cline | CLI exposes `--retries` / max consecutive mistakes. Its changelog explicitly calls out fixes for infinite retry loops when `write_to_file` or `replace_in_file` fails repeatedly. Tests require later write failures to say not to retry the full write. | Treat repeated invalid tool calls as a runtime state, not a prompt suggestion. After a small number of failures, force a different strategy or stop the run. |
| Kilo Code | Adds explicit tool-output truncation limits, defaults of 2000 lines and 50 KiB, saves full output separately, supports read offsets/limits, and truncates process stdout/stderr by byte budget. | Bound tool input/output in the harness so one large artifact does not poison the next provider request. |
| Hermes Agent | Detects provider stream parse errors, has GLM/Ollama truncation detection, drops broken recovery scaffolding from transcript tails, deduplicates identical tool calls, caps excess delegate calls, and surfaces when a large tool payload is being generated. | Classify malformed/truncated streams separately from ordinary tool failures and keep broken protocol scaffolding out of the next turn. |

## Remedy

### P0: Non-Retriable Malformed Tool-Call Boundary

PFTerminal should classify malformed oversized tool-call arguments as terminal
for the current turn when the failure shape indicates provider truncation or
invalid JSON framing.

Implementation mechanics:

- Track `finish_reason` in the Chat Completions stream state.
- If `finish_reason == "length"` and any tool call is pending, emit a provider
  stream/truncation error instead of a `ResponseItem::FunctionCall`.
- If a tool argument parse fails with `serde_json::error::Category::Eof`, treat
  it as `MalformedToolCallTruncated`, not as normal `RespondToModel`.
- For `MalformedToolCallTruncated`, stop the current turn and surface a concise
  user-visible error that names the tool and recommends chunked write or
  smaller edits.
- Release provider leases/cooldowns normally so the user can switch model,
  compact, or retry with a smaller plan.

### P0: Do Not Persist Raw Broken Payloads

When the malformed argument is non-retriable, do not record the raw
`FunctionCall` argument into conversation history. Store only a diagnostic:

- tool name;
- argument byte length;
- parse category;
- short safe excerpt;
- provider/model;
- `finish_reason` if present.

This prevents the next request from re-uploading a giant broken JSON string.

### P1: Bounded Large-Write Primitive

Full-file writes are valid for small files. They are the wrong primitive for a
large generated HTML mock under a provider output cap.

Add a bounded write path for third-party/chat providers:

- `structured_write` should have a conservative max content budget.
- Large writes should use a chunked protocol:
  `begin_write(path, mode, expected_size?)`, `append_write_chunk(write_id,
  index, content)`, and `finish_write(write_id, expected_sha256?)`.
- Each chunk must have a byte limit that safely fits inside one tool call.
- If a chunk fails JSON parsing or exceeds the budget, stop that write session
  instead of asking the model to retry the full artifact.
- For existing files, prefer `structured_edit` or an OpenCode-style
  exact-replace edit before full rewrite.

### P1: Repeated Failure Signature Guard

Track a per-turn signature:

```text
tool_name + parse_category + argument_prefix_hash + target_path
```

If the same signature fails twice, stop the turn even when the error is not
`EOF`. Cline's max-consecutive-mistake model is the right precedent: repeated
tool failure is a harness state, not an infinite invitation to retry.

### P2: Provider-Specific Edit Policy

Keep strict `apply_patch` for Codex-native/OpenAI profiles where it is
in-distribution. For GLM-class, OpenRouter, Baseten, and other generic Chat
Completions providers:

- prefer `structured_edit` for existing files;
- use bounded/chunked `structured_write` for new files or generated assets;
- keep strict `apply_patch` available only when the model profile demonstrates
  reliable grammar output;
- keep provider request leases and cache-aware telemetry, but do not treat them
  as a substitute for edit-tool correctness.

## Acceptance Tests

1. A streamed Chat Completions response ending with `finish_reason: "length"`
   while a tool call has pending arguments must not dispatch the tool.
2. A `structured_write` argument ending mid-string must produce a terminal
   malformed-tool-call event, not `needs_follow_up = true`.
3. The raw malformed argument body must not be persisted into conversation
   history.
4. Two identical malformed tool-call signatures in one turn must halt the turn.
5. A large file generation task must succeed through chunked write or fail once
   with actionable UI, never loop for repeated provider calls.

## Postmortem: 2026-06-23 StakeHub mock runaway

The remedy table above was written from the first six truncation events near
`22:45`. The same session kept running for ~2 hours and produced far more data,
which changes two mechanics in this plan.

Source rollout:
`/home/postfiat/.pfterminal/sessions/2026/06/23/rollout-2026-06-23T21-22-52-019ef65d-5652-7252-934f-ba52cf91f2e6.jsonl`

| Metric | Value |
| --- | --- |
| Persisted `structured_write` function calls in the runaway | 107 |
| `EOF while parsing a string` persisted parse failures | 108 |
| Other persisted parse failures in the same window | 1 `missing field` (`serde_json::error::Category::Data`) |
| Total persisted `function_call_output` parse failures | 109 |
| First → last persisted parse failure | `21:36:15.351Z` → `23:16:19.526Z` (~1 h 40 m) |
| Failed argument column range | 7062 → 15980 |
| `finish_reason: "length"` in persisted rollout evidence | none observed |
| Successful `structured_write`/edit during the mock task | 0 |
| Assistant text turns containing "writing now / chunk / small hunk" | 22 |

Observations that modify the remedy:

1. **`finish_reason` alone would not have caught this.** No `finish_reason:
   "length"` appears in persisted rollout evidence, so runtime classification
   cannot depend on persisted `finish_reason` alone. The reliable signal was the
   tool argument parse error itself: `serde_json::error::Category::Eof` for
   terminal truncation, plus one `missing field` classified as `Data`. P0 should
   treat `Eof` as terminal truncation and `Syntax`/`Data` as malformed tool
   arguments, especially when repeated or oversized; `Io` is a separate
   transport/runtime class, not a tool-malfunction boundary.

2. **The model migrates the oversized payload to other tools.** After
   `structured_write` kept failing, the model switched to `exec_command` and
   placed the same ~12–16 KB HTML inside `cmd` (a `cat > file <<'PY'` heredoc
   and a `python3 - <<'PY'` string literal). Those calls carry the same
   truncation risk in a different argument field. The byte budget in P1 must
   apply **to any tool argument**, not only to `structured_write`'s `content`.
   Otherwise P1 closes one door and the model walks through `exec_command`,
   `apply_patch`, or any future long-argument tool.

3. **The runaway includes non-progress text turns.** 22 model messages said
   "writing now" / described chunking, with no tool call emitted (or with a
   failing one). Those turns still spent a provider request and re-sent the
   now-~14 KB broken payload from history. The signature guard in P1 must also
   count **consecutive turns that produced no successful tool result**, not
   only identical parse-failure signatures, so the loop halts even when the
   model narrates instead of calling.

4. **Broken payloads persisted and grew.** Late `function_call` entries carry
   >12 KB of escaped HTML in `arguments` and were re-uploaded each turn. This
   confirms P0 ("do not persist raw broken payloads") and shows the cost: the
   poisoned payload inflated every subsequent request, lowering the effective
   output budget and making the *next* write more likely to truncate — a
   feedback loop, not just a flat retry.

### Additions to P0 / P1 from this postmortem

- **P0 (trigger):** classify as non-retriable on
  `serde_json::error::Category::Eof` during tool-argument parsing, OR
  `finish_reason: "length"` with a pending tool call. Treat `Syntax`/`Data`
  as malformed tool arguments when repeated, oversized, or otherwise clearly
  produced by broken tool-call framing. `Io` is a separate transport/runtime
  class, not a tool-malfunction boundary.
- **P1 (budget scope):** the per-call byte budget is a **tool-argument
  budget**, applied to every tool, not just `structured_write.content`.
  `exec_command.cmd`, `apply_patch`, and any long-string argument are bounded
  the same way. A call whose argument exceeds the budget is rejected before
  dispatch with the same non-retriable diagnostic.
- **P1 (non-progress guard):** alongside the `tool_name + parse_category +
  argument_prefix_hash + target_path` signature, track a **consecutive
  no-successful-tool-result** counter. Halt the turn at the same small bound
  (e.g. 2) when the model emits text-only turns or repeat failures.

### Revised acceptance tests (delta)

- (replaces #2) A tool argument that fails JSON parsing with
  `serde_json::error::Category::Eof` must produce a terminal truncation event,
  not `needs_follow_up = true`; `Syntax`/`Data` malformed tool arguments should
  hit the same boundary when repeated or oversized.
- (new) An oversized argument delivered via `exec_command.cmd` (or any
  non-`structured_write` long argument) must honor the same byte budget and
  reject before dispatch, not just `structured_write.content`.
- (new) The turn must halt after 2 consecutive assistant turns that produce no
  successful tool result, even when no parse signature repeats exactly.
- (clarifies #4) "Two identical malformed tool-call signatures" still halts,
  but is now an *instance* of the no-success guard, not the only stop rule.

## Review Against Reference Implementations

This section re-checks the remedy against the actual stored source of each agent
it cites, so the plan does not inherit a misremembered best practice. Sources:
`agent-harness-study/cline`, `agent-harness-study/hermes-agent`,
`agent-harness-study/kilocode`, and `opencode-current`.

### Cline: two distinct mechanisms, not one

The "Cline" lesson in the table conflates two separate, complementary loops
that Cline keeps deliberately apart. The remedy's signature guard maps to the
second one but is written as if it were the first.

1. **`consecutiveMistakeCount` — per-handler failure counter.**
   `TaskState.consecutiveMistakeCount` (`apps/vscode/src/core/task/TaskState.ts`)
   is incremented by each tool handler on a diff/param error and reset to 0
   only after `saveChanges()` succeeds
   (`WriteToFileToolHandler.ts`, see the explicit comment "Do NOT reset ...
   here - it should only be reset after successful completion"). The loop gate
   is in `core/task/index.ts:~2825`: when the count reaches
   `maxConsecutiveMistakes` (default **3**, `shared/storage/state-keys.ts:265`)
   it either fails the task (YOLO) or asks the user (`mistake_limit_reached`).
   A regression test (`WriteToFileToolHandler.consecutiveMistakeCount.test.ts`)
   pins the exact bug class — the counter being reset at operation *start*
   instead of after success — which is the same pathology PFTerminal is
   exposing (no reset-on-success boundary for `structured_write`).

2. **`consecutiveIdenticalToolCount` — identical-args loop detector.**
   `core/task/loop-detection.ts` computes a canonical signature
   (`toolCallSignature`, strips metadata keys, sorted) and counts identical
   consecutive calls: soft=**3** (inject warning), hard=**5** (escalate by
   forcing `consecutiveMistakeCount` to the max). This is the mechanism the
   remedy's P1 "signature guard" is actually modeling.

**Gap:** the remedy should state both mechanisms explicitly and mirror Cline's
**reset-on-success** rule (counter only clears after a real, validated tool
success) and its **progressive, context-window-aware error messaging**
(`WriteToFileToolHandler.ts:~126`: the write-error text changes at count `>= 2`
and includes `contextUsagePercent`). The postmortem's "no-successful-result
counter" is the right instinct but should be reset *only on success*, matching
Cline, not on any tool dispatch.

### Hermes: stop-misreport, role-sequence repair, cache-safe IDs

Hermes is under-described relative to its source, and three load-bearing
mechanics are absent from the remedy.

1. **`finish_reason` is untrustworthy in *both* directions.** The postmortem
   already notes the PFTerminal failure emitted no `length`. Hermes handles the
   inverse in `run_agent.py:_should_treat_stop_as_truncated`: Ollama-hosted GLM
   reports `finish_reason:"stop"` for a stream that was actually truncated, and
   Hermes treats that as truncation via a content heuristic (no natural ending).
   Lesson for the remedy: do not key on `finish_reason` at all as a *gate*;
   treat the malformed/incomplete tool argument as authoritative, with
   `finish_reason` (length OR stop-misreport) only as corroboration. This
   reinforces the postmortem's category-specific boundary: `Eof` is terminal
   truncation, repeated or oversized `Syntax`/`Data` are malformed tool
   arguments, and `Io` stays in the transport/runtime class.

2. **Dropping a broken payload requires role-sequence repair.**
   `_drop_trailing_empty_response_scaffolding` (`run_agent.py:~1524`) does more
   than delete the bad `FunctionCall`: it rewinds past the dangling tool-result
   messages **and** the `assistant(tool_calls)` that produced them, restoring
   the user/assistant/tool alternation invariant. The stated reason is that a
   trailing orphan `tool` message makes the next user turn land as
   `...tool, user, user`, a protocol-invalid sequence providers reject *silently
   with empty content* — which spawns a **different** infinite loop
   (empty-retry). **The remedy's P0 "do not persist raw broken payloads" is
   necessary but incomplete**: PFTerminal must also repair the message-sequence
   role invariant after the drop, or it trades the parse-loop for an
   empty-response loop. Add this as an explicit step in P0.

3. **Deterministic call IDs to protect prompt caching.**
   `_deterministic_call_id` derives tool call IDs from (name, arguments, index)
   so a re-issued (but valid) call does not invalidate OpenAI's prompt cache —
   random UUIDs would make every request prefix unique. When the remedy
   *replaces* a malformed call with a diagnostic, the replacement ID must be
   stable/derived, not random, or every remediation costs a full cache miss on
   a history that is already oversized. Note this trade-off in P0.

Hermes also deduplicates at two granularities the remedy only covers at one:
within a single turn (`_deduplicate_tool_calls`, identical `(name, args)`
pairs) and across turns (the loop detector). The within-turn dedup is cheap and
worth copying because the model frequently re-emits the same oversized call
twice in one response.

### OpenCode / Kilocode: output is bounded, input is not

The Kilocode claim is verified verbatim: `packages/opencode/src/tool/truncate.ts`
exports `MAX_LINES = 2000` and `MAX_BYTES = 50 * 1024`, writes the full text to
`TRUNCATION_DIR`, and returns a preview + hint. OpenCode also registers
`edit` / `write` / `apply_patch` as separate tools, and `edit` does exact
`old_string`→`new_string` with fuzzy correction sourced from Cline/Gemini.

**Gap:** both agents bound tool **output** (`truncate.output` in
`tool/tool.ts`), but neither imposes an **input-side argument byte budget** on
`write`/`edit`/`apply_patch`. They rely on the model + the provider's output
limit. OpenCode even models a failed parse as a first-class `invalid` tool that
just feeds the error back to the model (`tool/invalid.ts`) — the exact
RespondToModel loop PFTerminal is trying to break.

This is the strongest validation of the postmortem's "the budget must apply to
tool *arguments*, not only `structured_write.content`": the reference
implementations are *vulnerable to the same PFTerminal failure* because they
only cap output. PFTerminal adding an input-side argument budget is a genuine
improvement over the references, not a duplication of them — and it should be
framed that way rather than as "aligning with Kilo Code."

### The chunked-write proposal is unprecedented (weakest item)

P1 proposes a multi-call chunked protocol (`begin_write` /
`append_write_chunk` / `finish_write`). **No reference implementation does
this.** Cline streams content into a live editor UI (`handlePartialBlock`) but
still requires the full write/diff in one response; OpenCode/Kilocode use a
single write with output truncation on the *result*; Hermes uses output
truncation + transcript repair. Chunked writes also add new failure modes
(partial writes, orphaned sessions, index-skew) that none of the references
have solved.

Recommendation: demote chunked writes from P1 to a documented **fallback of
last resort**, and lead with the two patterns every reference uses:
(a) prefer exact `old_string`→`new_string` edits (OpenCode `edit` / Cline
`replace_in_file`) so the payload is small by construction, and (b) for a file
that *must* be generated whole, refuse once with an actionable UI rather than
retry. The single most effective intervention for the observed runaway is the
non-retriable boundary + reset-on-success counter; chunking is optional and
should not gate the fix.

### Summary of doc changes implied by this review

- Split the Cline lesson into `consecutiveMistakeCount` (per-handler,
  reset-on-success, default 3, user-escalation) vs `consecutiveIdenticalToolCount`
  (signature loop detector, soft 3 / hard 5). State the reset-on-success rule.
- Add **progressive, context-window-aware error messaging** (Cline) to the
  remediation output, not a single static diagnostic.
- Strengthen P0 trigger: ignore `finish_reason` as a gate; treat `Eof`
  (and Hermes stop-misreport) as authoritative truncation, treat repeated or
  oversized `Syntax`/`Data` as malformed tool arguments, and keep `Io` as a
  separate transport/runtime class.
- Add **role-sequence repair** to P0: after dropping a malformed call, rewind
  dangling tool-results + the issuing `assistant(tool_calls)` to preserve
  user/assistant/tool alternation (else empty-retry loop).
- Add **stable/derived call IDs** on the replacement so remediation does not
  break prompt caching on an already-oversized history.
- Add **within-turn dedup** of identical `(tool, args)` pairs (Hermes).
- Reframe the **argument byte budget** as a PFTerminal improvement over the
  references (they cap output only), not alignment with them.
- Demote **chunked writes** to a last-resort fallback; lead with exact-edit
  preference and fail-once-with-actionable-UI.

## Non-Goal

This plan is not a prompt-only workaround and not a request to tell GLM to
"write smaller." The runtime must enforce the boundary because the model cannot
recover from a tool call that was truncated before it became valid JSON.
