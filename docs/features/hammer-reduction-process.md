# Hammer Reduction Process

## Complete

- [x] Captured PFTerminal/Codex session evidence showing repeated high-input requests after small user nudges.
- [x] Confirmed PFTerminal is a Codex fork and inherits Codex prompt assembly, retry, compaction, token display, and edit-tool behavior unless changed deliberately.
- [x] Downloaded and reviewed local OpenCode, Hermes Agent, Kilo Code, and Cline source snapshots.
- [x] Compared provider retry handling, context compaction, edit primitives, request-size controls, and loop guards across the studied harnesses.
- [x] Confirmed the current edit-tool split: strict `apply_patch` remains available for Codex-native profiles, while structured edit/write exists for selected profiles.
- [x] Implemented shared provider request state in SQLite, keyed by provider/model/key fingerprint.
- [x] Implemented local provider cooldown after `429`, including `Retry-After` and reset-header parsing.
- [x] Implemented cross-process request leases so concurrent PFTerminal workers share active-request state.
- [x] Implemented request-byte/input-token preflight telemetry and third-party hammer-risk warnings before dispatch.
- [x] Implemented hard identical-tool-call loop guard and verified GLM-class structured edit/write routing.
- [x] Rebuilt `pfterminal` and ran a mechanical fake-provider benchmark proving immediate post-`429` retries are blocked locally.
- [x] Verified live GLM prompt caching on Z.AI and Ambient using real provider calls, not mocks.
- [x] Implemented cache-aware provider telemetry so large third-party requests do not produce user-facing warnings when the previous live request reported a healthy provider cache hit.

## To Do

- [x] P0 - Add request preflight telemetry and a shared `provider_request_state` store. Measure estimated input tokens, cached input tokens, serialized request bytes, provider/model/key fingerprint, cooldown state, and lease state before dispatch.
- [x] P0 - Enforce a provider/model/key cooldown circuit breaker after `429`. Parse provider reset headers when present; otherwise use local exponential cooldown such as 30s, 60s, 120s, capped at 5m.
- [x] P0 - Add a cross-process request lease keyed by provider/model/key so concurrent PFTerminal agents do not send large requests through the same credential.
- [x] P1 - Add third-party-provider preflight warnings and request-size visibility before requests reach provider rate, byte, or context limits. V0 deliberately does not launch an extra LLM compaction call while a provider cooldown is active.
- [x] P1 - Add hard loop guards for repeated identical tool calls, repeated failed edit attempts, and repeated immediate provider calls after a provider-side `429`.

Status: implementation and mechanical benchmark verified on 2026-06-23. Evidence snapshot date: 2026-06-23.

## Executive Summary

- User-facing problem: after a provider `429`, a user can type a tiny follow-up such as "continue" or "wat" and PFTerminal can send another full live-context request. In the observed thread, these follow-ups still carried about 35k-37k input tokens.
- PFTerminal is a Codex fork. It inherits Codex behavior where each turn rebuilds model input from live conversation history until compaction cuts it down.
- The observed failure was not mainly an internal automatic `429` retry loop. Generic provider config has `retry_429: false`, and the `429` surfaced at attempt 0.
- The missing control is a local dispatch gate: preflight sizing, provider cooldown, and a cross-process lease shared by all PFTerminal workers using the same provider/model/key.
- GLM prompt caching is automatic at the provider layer. PFTerminal must preserve stable prefixes and read provider-reported `usage.prompt_tokens_details.cached_tokens`; it should not invent a client-side response cache.
- Prompt caching can reduce repeated computation and cost, but it does not make the serialized request body small. The rollout recorded cumulative input of `888,271`, including `624,384` cached input.
- V0 should stop hammering by default: if cooldown or lease is active, do not send; show wait time and offer wait, compact, switch provider/model, or start a fresh thread.
- P1 should reduce future request size with earlier third-party-provider compaction, old tool-output pruning, request-byte gates, and loop guards.
- Provider profiles should differ. Codex-native models can keep strict `apply_patch`; GLM-class and generic gateway profiles should prefer structured edit/write and tighter context controls.

## Observed Facts

### Session Snapshot

| Field | Value |
| --- | --- |
| `thread_id` | `019ef259-b3ac-7601-86fd-a3cd6ae9bc56` |
| `provider` | `ambient` |
| `model` | `zai-org/GLM-5.2-FP8` |
| `cwd` | `/home/postfiat/repos` |
| `rollout` | `/home/postfiat/.pfterminal/sessions/2026/06/23/rollout-2026-06-23T02-40-25-019ef259-b3ac-7601-86fd-a3cd6ae9bc56.jsonl` |

### Token Accounting

Raw UI line:

```text
Token usage: total=270,204 input=263,887 (+ 624,384 cached) output=6,317
```

| Metric | Value | Meaning |
| --- | ---: | --- |
| UI `input` | `263,887` | Non-cached input tokens. |
| UI `+ cached` | `624,384` | Cached input tokens reported separately. |
| UI `output` | `6,317` | Output tokens. |
| UI `total` | `270,204` | `263,887 + 6,317`; cached input is excluded from this displayed total. |
| Rollout cumulative input | `888,271` | `263,887 + 624,384`; the rollout still records the large cumulative input shape. |

### Request And Provider Behavior

| Fact | Evidence |
| --- | --- |
| Small follow-ups still sent large inputs | Later turns sent about `36,414`, `37,211`, and `37,492` input tokens. |
| The rollout file itself was not huge | The JSONL trace was about 216 KB, so the issue was repeated context accumulation, not one massive stored artifact. |
| Provider request bodies were already large | Local request logs showed recent serialized request bodies around 110-160 KB. |
| Z.AI and Ambient both returned `429` | Both providers rejected initial attempts. The logs did not show `Retry-After` headers. |
| The runtime did not sit in an internal `429` retry loop | Generic provider `429` handling surfaced an error at attempt 0 because provider retry config has `retry_429: false`. |

## Interpretation

### Root Cause

PFTerminal currently lacks a shared provider dispatch gate above the inherited Codex turn loop. Codex rebuilds input from live history each turn, and auto-compaction is tied to token-window pressure, not provider `429`s or repeated tiny follow-ups.

The result is structural: every manual follow-up can become a new full-context request. If several PFTerminal processes run in tmux or separate shells, they also lack shared cooldown and active-request state for the same provider credential.

A paid provider plan does not remove per-minute request, token, burst, or concurrency limits. Repeated full-context calls can exhaust a burst bucket even when daily or monthly quota is healthy.

### Codex Mechanics Inherited By PFTerminal

| Area | Current inherited behavior | Source |
| --- | --- | --- |
| Prompt assembly | Each turn builds model input from cloned live history through `for_prompt(...)`. | `codex-rs/core/src/session/turn.rs` |
| Retry policy | Generic provider config sets `retry_429: false`; transport and 5xx errors are retryable, while 429 generally is not. | `codex-rs/model-provider-info/src/lib.rs`, `codex-rs/codex-client/src/retry.rs` |
| Stream retry | Dropped streams can be retried separately from initial HTTP 429 handling. | `codex-rs/core/src/responses_retry.rs` |
| Error mapping | Non-OpenAI 429 maps to a retry-limit style provider error unless it matches OpenAI usage-limit payloads. | `codex-rs/codex-api/src/api_bridge.rs` |
| Auto compaction | Compaction is tied to token-window pressure, not provider 429s or repeated small follow-ups. | `codex-rs/core/src/session/turn.rs` |
| Token display | UI total uses non-cached input plus output, while cached input is recorded separately. | `codex-rs/protocol/src/protocol.rs` |
| Edit tool choice | PFTerminal has structured edit/write for selected profiles; Codex-native models can keep strict `apply_patch`. | `codex-rs/core/src/tools/spec_plan.rs` |

### GLM Prompt Cache Mechanics

Z.AI documents [context caching](https://docs.z.ai/guides/capabilities/cache) as automatic: repeated or highly similar input content is identified by the provider, and cache use is reported in `usage.prompt_tokens_details.cached_tokens`. The coding-plan endpoint must be `https://api.z.ai/api/coding/paas/v4`, not the general API endpoint.

PFTerminal therefore should not add a fake local LLM-response cache. The low-friction implementation is:

- keep the inherited conversation prefix as stable as possible between turns;
- parse and preserve provider `cached_tokens` telemetry from Chat Completions responses;
- warn only when a large third-party request follows a known large cache miss;
- stay quiet when the previous provider response shows a healthy cache hit;
- keep cooldown and leases for actual provider `429`s, because caching reduces repeated computation but does not guarantee provider worker availability.

### Reference Harness Lessons

| Harness | Useful behavior | Lesson for PFTerminal |
| --- | --- | --- |
| PFTerminal/Codex | Sends full live history until compaction thresholds are reached. Prompt caching helps cost but does not remove large request bodies. | Add preflight, cooldown, and cross-process rate state around the inherited loop. |
| OpenCode | Shows context usage, reserves usable context, compacts before overflow, prunes tool outputs, and gates `apply_patch` by model family. | Use structured edit/write for non-Codex-native models and expose context/retry state before dispatch. |
| Hermes Agent | Compresses around 50 percent of context, targets a 20 percent summary ratio, protects first and recent turns, prunes old tool output, uses jittered backoff, and caps concurrency. | Compact earlier for third-party providers and keep intermediate tool chatter out of prompt context. |
| Kilo Code | Productizes auto compaction, threshold percent, pruning, keep/buffer settings, request-byte pruning, bounded retry, and doom-loop detection. | Make hammer-reduction controls explicit settings and add byte gates, not only token-window gates. |
| Cline | CLI exposes `basic`, `agentic`, and `off` compaction; VS Code path uses safety buffers, provider-specific output caps, retry reset parsing, and repeated-tool notices. | Keep a cheap non-LLM compaction fallback and tune provider-specific output headroom. |

## Proposed Implementation

Implement the request path in dependency order:

1. Measure and record request state.
2. Block on active cooldown.
3. Acquire a cross-process lease.
4. Compact or prune when thresholds require it.
5. Dispatch the provider request.
6. Update state, release lease, and apply cooldown on provider errors.

### 1. Request State And Preflight

Add a durable local store keyed by provider, model, and credential fingerprint. Do not store raw API keys.

```text
provider_request_state(
  provider_id,
  model,
  key_fingerprint,
  cooldown_until,
  lease_owner,
  lease_until,
  last_status,
  last_request_id,
  last_input_tokens,
  last_cached_input_tokens,
  last_request_bytes,
  updated_at
)
```

Before dispatch, compute and display:

- last user message size;
- estimated input tokens;
- cached input tokens from the previous response, when known;
- serialized request bytes after provider-specific formatting;
- provider/model/key fingerprint;
- active cooldown state;
- active lease holder, if any;
- whether compaction is recommended or required.

If the last user message is tiny but the live context is still tens of thousands of tokens, the UI should make that obvious before another provider call goes out.

### 2. Provider Cooldown Circuit Breaker

On `429`, parse `Retry-After`, `retry-after-ms`, `x-ratelimit-reset`, and equivalent provider reset headers. If no usable header exists, set local cooldown with exponential backoff such as 30s, 60s, 120s, capped at 5m.

While cooldown is active, PFTerminal should not send by default. It should show:

- provider/model/key affected;
- cooldown expiry and remaining wait;
- last request token and byte size;
- options: wait, compact, switch provider/model, start a fresh thread, or explicitly override with a logged force action.

### 3. Cross-Process Request Lease

Multiple PFTerminal workers can run from tmux or separate shells. V0 should allow one active request per provider/model/key. The lease should be durable, have a TTL, and be released when the stream finishes or fails.

If another process holds the lease, the UI should offer to wait, switch provider/model, or cancel. It should not silently send a second large request through the same credential.

### 4. Compaction And Pruning Profiles

For third-party providers, default to earlier compaction than the Codex-native path:

- compact around 50-60 percent of usable model context;
- leave a provider-specific safety buffer below advertised context length;
- protect the initial task and instructions;
- protect recent turns by token budget, not only message count;
- prune or summarize older tool outputs before asking an LLM to compact;
- store raw old outputs outside prompt context with file path and hash;
- use request-byte pruning when serialized body size crosses a configured limit;
- cap compaction attempts per turn;
- keep a basic non-LLM compaction fallback.

This should be a provider profile setting, not a product judgment about which model is better.

### 5. Tool Selection And Loop Guards

Use model-appropriate edit tools:

- OpenAI/Codex-native profiles: keep strict `apply_patch` available.
- GLM-class, OpenRouter, and generic gateway profiles: prefer structured edit/write unless the model is known to handle strict patch grammar reliably.

Add hard guards for:

- repeated identical tool calls;
- repeated failed edit attempts against the same file and same target text;
- repeated read-only shell commands when the user set a review cap;
- repeated immediate provider calls after a provider-side `429`.

When a guard trips, stop and report the specific loop. Do not ask the provider to reason through another full context copy.

## Provider Best Practices

| Provider class | Practice |
| --- | --- |
| OpenAI/Codex-native | Keep strict `apply_patch` available for models trained to use it. Still track serialized bytes, non-cached input, and cached input so prompt caching does not hide oversized turns. |
| Z.AI/GLM-5.2 class | Prefer structured edit/write. Do not immediately retry `429` without `Retry-After`; if no header exists, apply local cooldown. Use smaller smoke-test prompts and compact before large context turns. |
| Ambient GLM | Treat as GLM-class for edit tools and third-party-provider cooldowns. Share provider/model/key cooldown and lease state across tmux workers. |
| OpenRouter and generic OpenAI-compatible gateways | Assume rate-limit headers may be missing or provider-specific. Strip unsupported tool types, cap context, and set local exponential cooldown on 429. |
| Fast inference providers such as Cerebras/Groq | Cap max output tokens because some providers reserve quota based on requested maximum output, not only actual output. Preserve burst headroom. |
| Local providers such as Ollama/LM Studio | They avoid paid API 429s, but context bloat still hurts latency. Keep pruning, request-byte checks, and loop guards enabled. |

## Implementation Evidence

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Shared request state | `codex-rs/state/migrations/0040_provider_request_state.sql` and `codex-rs/state/src/runtime/provider_requests.rs` store provider/model/key fingerprint, cooldown, lease, last status, last request id, input tokens, cached input tokens, request bytes, thread id, and turn id. | `cargo test -p codex-state provider_requests -- --nocapture` |
| Cooldown after `429` | `codex-rs/core/src/session/turn.rs` records provider results after sampling. `codex-rs/state/src/runtime/provider_requests.rs` sets local cooldown to provider reset timing when available, otherwise 30s, 60s, 120s, capped at 5m. | State tests cover first cooldown, repeated backoff, and explicit retry-after timing. |
| Reset-header parsing | `codex-rs/codex-api/src/api_bridge.rs` extracts `retry-after-ms`, `retry-after`, `x-ratelimit-reset-ms`, and `x-ratelimit-reset`. | `cargo test -p codex-api map_api_error_maps_retry_after_ms_for_generic_429 -- --nocapture` |
| Cross-process lease | Provider request leases are acquired before dispatch and released after stream success/failure. Lease TTL is 10 minutes to avoid stale process lockout. | State tests cover active lease blocking and release allowing the next worker. |
| Request-size preflight | `ModelClientSession::serialized_request_body_bytes(...)` serializes the exact outbound Responses or Chat request body before dispatch. Third-party providers warn at 32k input tokens or 128 KiB serialized body. | Fake-provider benchmark recorded a 38,577-byte request and then blocked the next immediate run locally. |
| Cache-aware preflight | `codex-rs/codex-api/src/endpoint/chat_completions.rs` parses `usage.prompt_tokens_details.cached_tokens`. `codex-rs/core/src/session/turn.rs` now uses last-turn provider cache telemetry to suppress noisy gross-size warnings after healthy cache hits and warn only after known large cache misses. | `cargo test -p codex-core third_party_cache -- --nocapture`; `scripts/glm-cache-live-probe --provider zai --prefix-lines 600`; `scripts/glm-cache-live-probe --provider ambient --prefix-lines 160` |
| Provider cache evidence in cooldown state | `codex-rs/state/migrations/0041_provider_request_cache_usage.sql` and `codex-rs/state/src/runtime/provider_requests.rs` persist provider-reported input and cached-input tokens after successful requests, so future local block messages can show the last real cache result. | `cargo test -p codex-state provider_requests -- --nocapture` |
| Repeated immediate provider calls after `429` | `try_run_sampling_request(...)` blocks before `client_session.stream(...)` when cooldown is active, so the provider is not contacted. | `scripts/hammer-reduction-benchmark` shows `new_post_requests_on_second_run: 0`. |
| Identical tool-call loop guard | `codex-rs/core/src/tools/parallel.rs` stops the fourth identical tool call in one turn using a tool-name plus payload hash signature. | `cargo test -p codex-core repeated_identical_tool_calls_are_stopped_after_budget -- --nocapture` |
| GLM-class edit routing | Existing structured edit/write routing remains the preferred path for GLM/Z.AI/Ambient-style profiles, with strict `apply_patch` still available for Codex-native profiles. | `cargo test -p codex-core glm_model_slug_uses_structured_edit_tools_instead_of_apply_patch -- --nocapture`; `cargo test -p codex-core zai_provider_uses_structured_edit_tools_instead_of_apply_patch -- --nocapture`; `cargo test -p codex-core repeated_strict_patch_failures_switch_turn_to_structured_edit_tools -- --nocapture` |

## Mechanical Benchmark

Benchmark command:

```bash
scripts/hammer-reduction-benchmark
```

Benchmark shape:

| Step | Result |
| --- | --- |
| Binary | `codex-rs/target/debug/pfterminal`, rebuilt on 2026-06-23 |
| Provider | Local fake OpenAI-compatible Chat provider returning `429` with no reset header |
| First run | One HTTP `POST /v1/chat/completions`, exit code 1 |
| First request body | `38,577` bytes |
| Second immediate run | Locally blocked by hammer-reduction cooldown, exit code 1 |
| New provider requests on second run | `0` |
| Local block reason | `Cooldown`, with last status `429` and last request bytes recorded |

Benchmark JSON excerpt:

```json
{
  "ok": true,
  "first_post_body_bytes": 38577,
  "post_requests_after_first": 1,
  "new_post_requests_on_second_run": 0,
  "blocked_locally_on_second_run": true,
  "requests_after_first": 1,
  "requests_after_second": 1
}
```

## Live GLM Cache Probe

Probe commands:

```bash
scripts/glm-cache-live-probe
scripts/glm-cache-live-probe --provider zai --prefix-lines 600
scripts/glm-cache-live-probe --provider ambient --prefix-lines 160
```

Probe shape:

| Provider | Endpoint class | Request bytes | Call 1 | Call 2 | Result |
| --- | --- | ---: | --- | --- | --- |
| Z.AI | Coding-plan Chat Completions | `72,864` | `prompt_tokens=21,029`, `cached_tokens=14,656` | `prompt_tokens=21,029`, `cached_tokens=20,992` | `99.82%` second-call cache hit |
| Ambient | Ambient Chat Completions | `19,605` | `prompt_tokens=5,635`, `cached_tokens=0` | `prompt_tokens=5,635`, `cached_tokens=5,632` | `99.95%` second-call cache hit |

The default all-provider probe also passed on 2026-06-23 with `99.95%` second-call cache hit on Ambient and `98.92%` cache hit on both Z.AI calls, because the Z.AI prefix was already warm from the prior probe.

Provider capacity caveat:

- Ambient returned `429 No workers are currently available` for the larger `51,065` byte and `31,705` byte live probes during this run.
- Z.AI returned provider overload code `1305` for a `116,424` byte live probe during this run.
- Those failures confirm caching is not a substitute for request-size control and provider cooldowns. It is a provider-side optimization PFTerminal should observe and use when available.

## Acceptance Criteria

- After a provider `429`, typing "continue" or "wat" does not send another provider request until cooldown expires or the user explicitly overrides.
- If no provider reset header is present, cooldown follows local backoff such as 30s, 60s, 120s, capped at 5m.
- Two PFTerminal processes using the same provider/model/key share cooldown and active-request state.
- A tiny user message with a 35k-token live context triggers preflight visibility before request dispatch.
- Preflight shows estimated input tokens, cached input tokens when known, serialized request bytes, provider/model/key fingerprint, cooldown state, lease state, and compaction recommendation.
- Old tool outputs are pruned or summarized before they dominate the prompt; raw outputs are preserved outside prompt context with path and hash.
- Third-party-provider profiles compact around 50-60 percent of usable context or earlier when request-byte limits require it.
- GLM-class models use structured edit/write by default; strict `apply_patch` remains available for Codex-native models.
- Repeated identical tool calls and repeated failed same-file edit attempts stop with a clear guard message.
- The UI can answer: what provider was hit, how large the request was, how much was cached, why it was blocked or sent, and when it is safe to retry.

## Source Snapshot Register

Commit values are recorded as captured in the study. OpenCode was captured with a full SHA; Hermes Agent, Kilo Code, and Cline were captured with short SHAs.

| Harness | Local path | Remote | Commit |
| --- | --- | --- | --- |
| OpenCode | `/home/postfiat/repos/opencode-current` | `https://github.com/anomalyco/opencode.git` | `f48f24ec4e1e26cc32c4d4953497fe2734c61ee1` |
| Hermes Agent | `/home/postfiat/repos/agent-harness-study/hermes-agent` | `https://github.com/NousResearch/hermes-agent.git` | `bb7ff7d` |
| Kilo Code | `/home/postfiat/repos/agent-harness-study/kilocode` | `https://github.com/Kilo-Org/kilocode.git` | `0f55066` |
| Cline | `/home/postfiat/repos/agent-harness-study/cline` | `https://github.com/cline/cline.git` | `19d4248` |

### Key Source Areas Checked

| Harness | Source areas | Evidence used |
| --- | --- | --- |
| OpenCode | `packages/opencode/src/tool/registry.ts`; `packages/opencode/src/tool/edit.ts`; `packages/opencode/src/session/retry.ts`; `packages/opencode/src/session/overflow.ts`; `packages/opencode/src/session/compaction.ts` | Model-gated `apply_patch`, structured edit/write, retry-after parsing, visible retry state, usable-context calculation, and old tool-output pruning. |
| Hermes Agent | `cli-config.yaml.example`; `agent/context_compressor.py`; `agent/conversation_compression.py`; `agent/retry_utils.py`; `agent/rate_limit_tracker.py`; `tools/code_execution_tool.py` | Compression threshold `0.50`, target ratio `0.20`, first/recent turn protection, old output pruning, compression locks, jittered backoff, rate-limit header tracking, concurrency caps, and stdout/stderr caps. |
| Kilo Code | Kilo native paths: `packages/kilo-vscode/webview-ui/src/components/settings/ContextTab.tsx`; `packages/core/src/config/compaction.ts`; `packages/kilo-vscode/src/util/retry.ts`. Kilo bundled/forked OpenCode package paths inside `/home/postfiat/repos/agent-harness-study/kilocode`, not the separate OpenCode checkout: `packages/opencode/src/session/prompt.ts`; `packages/opencode/src/session/compaction.ts`; `packages/opencode/src/session/processor.ts`. | User-facing compaction settings, compaction config fields, retry-after parsing, request-byte pruning, payload-limit pruning, media stripping, post-summary trimming, and doom-loop detection for repeated identical tool calls. |
| Cline | `apps/cli/README.md`; `apps/cli/src/utils/compaction-mode.ts`; `apps/vscode/src/core/context/context-management/ContextManager.ts`; `apps/vscode/src/core/context/context-management/context-window-utils.ts`; `apps/vscode/src/core/api/retry.ts`; `apps/vscode/src/core/api/providers/cerebras.ts`; `apps/vscode/src/core/prompts/responses.ts` | CLI compaction modes `basic`, `agentic`, and `off`; fixed safety buffers; context-window detection; retry-after and reset parsing; provider-specific max-token caps; duplicate-read and repeated-tool-call notices. The JetBrains plugin was not part of the open-source study. |

## Quantification

Current problem:

- Observed live PFTerminal follow-ups after provider `429` still sent about `36,414`, `37,211`, and `37,492` input tokens.
- Recent local request bodies were about 110-160 KB in the real session logs.
- The UI token line showed `263,887` non-cached input plus `624,384` cached input, so prompt caching reduced repeated computation but did not make the request shape small.

Implemented solution:

- Every provider request now has a preflight row containing provider/model/key fingerprint, input tokens, cached input tokens, serialized request bytes, cooldown state, lease state, thread id, and turn id.
- A provider-side `429` creates local provider/model/key cooldown before the next request is allowed.
- Concurrent workers share a provider/model/key lease, preventing separate tmux agents from silently sending overlapping large requests through the same credential.
- Repeated identical tool calls stop after 3 attempts in a single turn.

Measured improvement:

- In the fake-provider benchmark, the first request sent one `38,577` byte request and received `429`.
- The immediate second run sent `0` provider requests and `0` provider request bytes.
- For immediate post-`429` follow-ups, this is a 100 percent reduction in repeat provider calls and outbound provider bytes during the cooldown window.
- The structural failure mode is therefore closed for immediate retries: a tiny "continue" can no longer trigger another full-context provider request while local cooldown is active.
