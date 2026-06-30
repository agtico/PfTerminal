# Native Provider Performance

Date: 2026-06-28

## Executive Summary

Native non-OpenAI turns in PFTerminal were slow for fixable local reasons, not
because Vercel, Baseten, Z.AI, or Ambient are inherently unusable.

The largest measured delay was inside PFTerminal before the provider stream
started. A resumed native Z.AI turn spent about 9.5s in local client/provider
setup before any useful provider streaming could happen. After removing the
hot-path vault fingerprint read, caching provider auth in the model provider,
and caching decrypted local secrets inside the secrets backend, the same setup
class dropped to about 1.6s in a fresh process.

The second large issue was that Vercel's Responses path was not using server
conversation continuation. PFTerminal only set `store: true` for Azure, did not
persist the last response id in the rollout, and therefore could not send
`previous_response_id` on a later HTTP Responses turn. After the patch, Vercel
Responses requests use `store: true`, persist `ModelResponseCompleted`, restore
the last response id on resume, and send only the incremental input when the
previous logical request can be matched.

That makes the Vercel Responses path competitive on simple resumed turns. A
fresh-process benchmark of a large resumed thread produced a 4.05s wall clock
turn, with about 1.6s in local setup and about 1.6s in provider request/stream
time after setup. The dumped Vercel request had `store: true`,
`previous_response_id` populated, `input_len: 1`, and `input_chars: 119`; tools
and instructions still made up most of the remaining body.

The fast path that wrapped Claude Code was using is now available natively:
PFTerminal has a native Anthropic Messages wire path with `cache_control`
content blocks, tool replay, streamed tool-use parsing, and curated built-in
providers for `zai-anthropic`, `vercel-anthropic`, `vercel-anthropic-fast`, and
`baseten-anthropic`. A built-in `openrouter-anthropic` route is now verified as
well. Live `pfterminal exec` runs prove the Vercel Fast Anthropic provider
reaches the same cache-control mechanism as wrapped Claude Code: the resumed
built-in `vercel-anthropic-fast` run reported 20,224 cached input tokens out of
20,304 total input tokens, zero reasoning tokens, and a 4.38s fresh-process
wall clock with about 1.94s in provider request/stream time after local client
setup. After fixing provider env-var precedence so
`AI_GATEWAY_API_KEY` bypasses vault lookup when explicitly supplied, a resumed
native run completed in 3.53s wall clock with `anthropic_http_after_client_setup`
at 0ms, 40,512 cached input tokens, and zero reasoning tokens. A direct wrapped
Claude Code Vercel Fast resume measured 3.60s wall clock and 3.09s API
duration.

Current completion-audit runs show native PFTerminal is in the wrapped-Claude
performance band, or better, for the working non-OpenAI routes tested here:
Vercel Fast, OpenRouter, Z.AI, and Ambient Kimi. Chat Completions now has
provider-gated `prompt_cache_key` and explicit `cache_control` markers for
gateway model families that the comparator agents also mark. Z.AI/Ambient GLM
Chat now sends explicit `enable_thinking: false` for normal `high`/`xhigh`
config instead of forcing hidden reasoning, while preserving explicit custom
`max`/`deep` as an opt-in. Z.AI, OpenRouter, and Vercel now have native
Anthropic-wire routes; Ambient Kimi is not slower natively than through the
Claude bridge, Ambient GLM currently has no upstream worker through either
native Codex or wrapped Claude, and Baseten is blocked by account payment status
through both paths. A real same-process no-alt-screen TUI run now confirms the
interactive path: turn one and turn two both rendered `OK`, with zero client
setup time and the warm turn completing the provider stream request at about
1.97s. The manual TUI check has also been turned into an automated PTY harness,
`scripts/native-provider-tui-benchmark`, which ran two real TUI turns through
the native Anthropic providers and captured zero client setup time on warm
turns.
The wrapped-Claude pane backend has also been benchmarked through
`pfterminal claude-pane-smoke`; the smoke report now records first/resume
durations and first/resume artifact paths.
The `/model` picker now routes the measured fast rows to those providers:
`zai/glm-5.2-fast` selects `vercel-anthropic-fast`, and `z-ai/glm-5.2` selects
`openrouter-anthropic`.

## Measured Evidence

### Before The Fixes

| Path | Observation |
| --- | --- |
| Native Z.AI resumed thread | `current_client_setup()` was about 9539ms. Total wall time was about 15.46s. Provider streaming after setup was about 5.15s. |
| Native Z.AI request shape | Large resumed thread sent about 51k input tokens, with about 27k provider-reported cached input tokens. |
| Direct Vercel request body replay | The exact serialized Vercel streaming request completed in about 1.4s when sent directly, proving the provider request itself was not the only bottleneck. |
| Wrapped Claude Code smoke | A wrapped Claude Code Vercel Fast turn reported about 1319ms API duration and about 1328ms TTFT on a tiny turn. |

### After The Fixes

| Path | Observation |
| --- | --- |
| Native Z.AI after provider auth cache | `current_client_setup()` dropped from about 9.5s to about 3.2s. |
| Native Z.AI after secrets cache | `current_client_setup()` dropped again to about 1.6s in a fresh process. Total wall was about 7.7s; the remaining Chat path provider stream was still about 5.4s. |
| Native Vercel Responses after server-state patch | Fresh-process resumed turn wall was about 4.05s. Local setup was about 1.6s; provider request/stream after setup was about 1.6s. |
| Dumped Vercel Responses request | `store: true`, `previous_response_id` populated, `input_len: 1`, `input_chars: 119`, `tools_len: 14`, `tools_chars: 21754`, `instructions_chars: 420`. |
| Native OpenRouter Chat first turn | `minimax/minimax-m3` accepted the new Chat `cache_control` blocks and `prompt_cache_key`; wall was 6.92s, local setup was about 1.6s, cached input was 128 tokens. |
| Native OpenRouter Chat resumed turn | Same thread and model returned `OK`; wall was 5.74s, local setup was about 1.6s, last-turn input was 12,670 tokens with 12,544 cached input tokens. |
| Native OpenRouter Anthropic custom-provider smoke | A one-off Anthropic-wire custom provider using `https://openrouter.ai/api/v1` returned `OK`; first turn wall was 4.05s with provider stream at 1.65s, and resume wall was 7.24s with 10,112 cached tokens. |
| Native Z.AI GLM baseline with `xhigh -> max` | Fresh-process wall was 10.17s; provider request/stream bucket was 9.33s; output was 52 tokens with 48 reasoning tokens. |
| Native Z.AI GLM with manual medium/high | Fresh-process wall was 7.16s; provider request/stream bucket was 6.35s; output was 3 tokens with 0 reasoning tokens. |
| Native Z.AI GLM with `prompt_cache_key` experiment | Fresh-process wall regressed to 11.49s even though the provider reported 12,352 cached input tokens. |
| Native Z.AI GLM after reasoning cap and no cache key | Fresh-process wall was 8.32s; provider request/stream bucket was 7.24s; request body had no `prompt_cache_key` and sent `reasoning_effort: "high"`. The provider still reported 12,352 cached input tokens implicitly, so the cache counter is not a speed guarantee on this path. |
| Native Z.AI GLM with thinking fields omitted | Fresh-process wall regressed to 14.69s; reasoning tokens dropped to 0, but provider-reported cached input also dropped to 0. This variant was rejected. |
| Native Z.AI GLM with `enable_thinking: false` | Fresh-process wall was 8.14s; provider request/stream bucket was 7.38s; output was 2 tokens with 0 reasoning tokens. Provider-reported cached input was still 0, so this fixes hidden reasoning but not GLM Chat prefill/cache behavior. |
| Native Z.AI Anthropic first turn | Using `wire_api = "anthropic"` and `https://api.z.ai/api/anthropic/v1`, wall was 6.97s with 1,280 cached input tokens and zero reasoning tokens. |
| Native Z.AI Anthropic resumed turn | Same thread wall was 6.61s with 11,392 cached input tokens out of 20,298 input tokens and zero reasoning tokens. |
| Built-in `zai-anthropic` first turn | No custom provider table; wall was 7.80s, request contained Anthropic `cache_control`, output was `OK`, and reasoning tokens were zero. |
| Built-in `zai-anthropic` resumed turn | Same thread wall was 8.32s with 10,112 cached input tokens out of 20,301 input tokens and zero reasoning tokens. |
| Current native `zai-anthropic` completion-audit TUI harness | Current binary ran two real TUI turns with `client_setup_ms=[0,0]`, `stream_request_ok_ms=[5266,2935]`, `stream_completed_ms=[2,72]`, usage `10,361 input / 10,304 cached / 4 output`, and resume hint `019f0f12-0cb2-77c0-9098-430572f7169b`. |
| Current wrapped Claude pane Z.AI completion-audit smoke | `pfterminal claude-pane-smoke --providers zai` on the same current binary passed with first turn 73,323ms and resume turn 40,476ms. Native Z.AI TUI is much faster than the wrapped pane on this run. |
| Native Vercel Anthropic first turn | Using `https://ai-gateway.vercel.sh/v1/messages`, wall was 4.74s with standard Anthropic SSE and zero reasoning tokens. |
| Native Vercel Anthropic resumed turn | Same thread wall was 4.78s with 10,112 cached input tokens out of 20,298 input tokens and zero reasoning tokens. |
| Built-in `vercel-anthropic-fast` first turn | No custom provider table; wall was 5.62s, request contained Anthropic `cache_control`, and the provider reported 10,112 cached input tokens. |
| Built-in `vercel-anthropic-fast` resumed turn | Same thread wall was 4.38s; provider-stream time after local setup was about 1.94s; usage was 20,304 input, 20,224 cached input, 4 output, 0 reasoning. |
| Built-in `vercel-anthropic-fast` with env key after auth precedence fix | Same native path with `AI_GATEWAY_API_KEY` set had `anthropic_http_after_client_setup=0ms`; wall was 3.53s, usage was 40,711 input, 40,512 cached input, 8 output, 0 reasoning. |
| Built-in `openrouter-anthropic` first turn | No custom provider table; wall was 3.85s, provider stream returned at 1.50s, output was `OK`, and reasoning tokens were zero. |
| Built-in `openrouter-anthropic` resumed turn | Same thread wall was 2.90s, provider stream returned at 0.57s, usage was 20,313 input, 10,112 cached input, 6 output, 0 reasoning. |
| Current native `openrouter-anthropic` completion-audit TUI harness | Current binary ran two real TUI turns with `client_setup_ms=[0,0]`, `stream_request_ok_ms=[1654,568]`, `stream_completed_ms=[20,29]`, usage `10,444 input / 10,240 cached / 6 output`, and resume hint `019f0f10-ab25-7480-88c9-b46a05fb2a38`. |
| Current wrapped Claude pane OpenRouter completion-audit smoke | `pfterminal claude-pane-smoke --providers openrouter` on the same current binary passed with first turn 46,076ms and resume turn 1,958ms. Native OpenRouter warm TUI is faster than the wrapped pane on this run. |
| Wrapped Claude Code Vercel Fast current benchmark | Direct Claude CLI run with Vercel Anthropic env completed first turn in 3.21s wall, 2.74s API duration, 2.82s TTFT, 1,642 input tokens, 0 cached input. |
| Wrapped Claude Code Vercel Fast resumed benchmark | Same Claude session completed in 3.60s wall, 3.09s API duration, 3.14s TTFT, 66 uncached input tokens, 1,600 cached input tokens. |
| Wrapped Claude pane Vercel Fast smoke | `pfterminal claude-pane-smoke --providers vercel-fast` passed through the pane registry/execution backend. Latest report had first turn 14,416ms and resume turn 2,295ms; the Claude result event for resume reported 1,825ms wall, 1,791ms API, 1,817ms TTFT, and 13,632 cached input tokens. |
| Wrapped Claude pane OpenRouter smoke | `pfterminal claude-pane-smoke --providers openrouter` passed through the pane registry/execution backend. Latest report had first turn 54,531ms and resume turn 1,705ms; the Claude result event for resume reported 1,233ms wall, 1,179ms API, 1,220ms TTFT, and 18,432 cached input tokens. |
| Wrapped Claude pane Ambient Kimi smoke | `pfterminal claude-pane-smoke --providers ambient-kimi` passed through the local Ambient Chat bridge. Latest report had first turn 41,308ms and resume turn 4,706ms, so the wrapped path is slower than native on first turn and equivalent on resume. |
| Wrapped Claude pane Ambient GLM smoke | `pfterminal claude-pane-smoke --providers ambient` failed in 2,687ms with Ambient HTTP 429/model worker unavailable for `glm52-fp8-202k`, matching the native Ambient GLM availability failure rather than exposing a native Codex performance problem. |
| Wrapped Claude pane Baseten smoke | `pfterminal claude-pane-smoke --providers baseten` failed in 1,073ms with HTTP 402 payment status, matching the native Baseten account block. |
| Baseten Anthropic endpoint smoke | `https://inference.baseten.co/v1/messages` returned HTTP 402 for the current key/account (`please check your current payment status`), so endpoint shape is known but live benchmark is blocked by account status. After the retry classifier fix, the same live command failed once in 1.55s wall with no `Reconnecting...` loop. |
| Provider key reveal | `pfterminal vault auth-helper provider/zai_api_key` took 1.68s then 1.65s in fresh processes, matching the remaining `current_client_setup()` floor in fresh `exec` benchmarks. |
| Native `vercel-anthropic-fast` real TUI first turn | Real PTY with `--no-alt-screen`, `AI_GATEWAY_API_KEY`, and initial prompt rendered `OK`; `anthropic_http_after_client_setup=0ms`, stream request completed at about 2.64s. |
| Native `vercel-anthropic-fast` real TUI warm turn | Same TUI process accepted a second prompt via carriage return and rendered `OK`; `anthropic_http_after_client_setup=0ms`, stream request completed at about 1.97s; shutdown reported `input=10,355 (+ 10,304 cached) output=4`. |
| Automated native `vercel-anthropic-fast` TUI harness | `scripts/native-provider-tui-benchmark` ran a real PTY with two TUI turns. It completed both turns, submitted the second prompt through the TUI, reported `client_setup_ms=[0,0]`, latest `stream_request_ok_ms=[2429,1769]`, and final usage of 20,608 cached tokens. |
| Current native `vercel-anthropic-fast` completion-audit TUI harness | Current binary ran two real TUI turns with `client_setup_ms=[0,0]`, `stream_request_ok_ms=[5378,4037]`, `stream_completed_ms=[85,14]`, and resume hint `019f0f0e-c6f5-75a2-9c35-fc879412c609`. Provider latency fluctuated upward on this run, but local setup remained eliminated. |
| Current native `vercel-anthropic-fast` completion-audit resume | Resuming that same TUI thread through native `pfterminal exec` with `AI_GATEWAY_API_KEY` completed in 2.77s wall, `anthropic_http_stream_request_ok=1892ms`, usage `31,030 input / 10,304 cached / 6 output / 0 reasoning`, and output `OK`. |
| Current wrapped Claude pane Vercel Fast completion-audit smoke | `pfterminal claude-pane-smoke --providers vercel-fast` on the same current binary passed with first turn 14,666ms and resume turn 2,725ms. The native resume above is in the same band and has lower provider-stream timing. |
| Native Ambient GLM Chat smoke | With `AMBIENT_API_KEY` supplied in the environment, native setup was 0ms and the request reached Ambient in about 1.47s, then returned HTTP 429. The dumped request used `enable_thinking: false`, no `reasoning_effort`, no `prompt_cache_key`, and a 62KB full-context Chat body. |
| Native Ambient Kimi Chat first turn | `moonshotai/kimi-k2.7-code` returned `OK`; wall was 7.05s, `chat_http_after_client_setup=0ms`, provider stream request completed at 4.53s, and usage was 11,523 input, 0 cached, 21 output, 20 reasoning. |
| Native Ambient Kimi Chat resumed turn | Same thread completed in 4.72s, `chat_http_after_client_setup=0ms`, provider stream request completed at 2.30s, and usage was 23,075 input, 11,520 cached, 23 output, 21 reasoning. |
| Automated native Ambient Kimi TUI harness | `scripts/native-provider-tui-benchmark --provider ambient --model moonshotai/kimi-k2.7-code --dump-request-api chat` ran two real TUI turns. It reported `client_setup_ms=[0,0]`, `stream_request_ok_ms=[2275,2312]`, `turns_completed=2`, `success=true`, and resume hint `019f0f04-e603-7682-bc3a-bb755624ac3a`. |

Main benchmark thread:

```text
019f0e6c-1280-7ae0-8ac5-da47eeee0c40
```

Main rollout:

```text
/home/postfiat/.pfterminal/sessions/2026/06/28/rollout-2026-06-28T13-29-51-019f0e6c-1280-7ae0-8ac5-da47eeee0c40.jsonl
```

OpenRouter Chat cache-control smoke rollout:

```text
/home/postfiat/.pfterminal/sessions/2026/06/28/rollout-2026-06-28T14-34-42-019f0ea7-722f-7d63-8038-38668f03d6d2.jsonl
```

## Root Causes

### 1. Provider Auth Was On The Hot Path

The provider preflight path fingerprinted stored provider credentials by asking
the vault for the secret. The actual request path also resolved provider auth,
and multiple `current_client_setup()` calls could hit the same encrypted
secrets file in one process.

This was a local startup/setup bug. It inflated every fresh process and every
resumed exec/TUI process before the provider had a chance to stream.

Fixed in:

- `codex-rs/core/src/session/turn.rs`: `provider_request_key` now uses a stable
  `stored:{ENV_KEY}` fingerprint when the key is vault-backed, instead of
  reading the secret just to identify throttle state.
- `codex-rs/model-provider/src/provider.rs`: provider-scoped vault auth is
  cached per provider instance, and explicit environment variables such as
  `AI_GATEWAY_API_KEY` now take precedence over stored provider keys.
- `codex-rs/secrets/src/local.rs`: decrypted local secrets are cached inside the
  backend, and local fallback is checked before OS keyring fallback when present.

### 2. Vercel Responses Was Not Continuing Server-Side State

The old request builder only set `store: true` for Azure. For Vercel, this meant
HTTP Responses turns behaved like stateless full-context calls even though
Vercel's Responses API supports OpenAI-compatible request shape and automatic
caching. Vercel documents AI Gateway automatic caching and request options such
as provider options and the Responses API:

- https://vercel.com/docs/ai-gateway/models-and-providers/automatic-caching
- https://vercel.com/docs/ai-gateway/models-and-providers/provider-options
- https://vercel.com/docs/ai-gateway/sdks-and-apis/responses

Fixed in:

- `codex-rs/codex-api/src/common.rs`: `ResponsesApiRequest` now serializes
  `previous_response_id`, and websocket request conversion preserves it.
- `codex-rs/core/src/client.rs`: Vercel Responses requests use `store: true`.
- `codex-rs/core/src/client.rs`: HTTP Responses requests can reuse the last
  server response id and shrink the next request to incremental input.
- `codex-rs/protocol/src/protocol.rs`, `codex-rs/rollout/src/policy.rs`, and
  `codex-rs/core/src/session/mod.rs`: completed response ids are recorded in
  the rollout and restored on resume when model and provider still match.

### 3. HTTP Transports Were Rebuilt Per Request

The native path created a new `reqwest::Client` for multiple unary and streaming
calls. That throws away connection pooling and makes every turn pay fresh client
setup costs.

Fixed in:

- `codex-rs/core/src/client.rs`: `ModelClientState` owns a shared
  `reqwest::Client`, and provider calls clone that client into `ReqwestTransport`.

### 4. Chat Completions Needed Explicit Cache Markers

This is partially fixed.

OpenCode, Kilo Code, Cline, and Hermes do not just hope implicit prefix matching
does the right thing. They inject provider-specific cache markers or gateway
caching options:

| Harness | Local evidence |
| --- | --- |
| OpenCode | `/home/postfiat/repos/opencode-current/packages/opencode/src/provider/transform.ts` applies `cacheControl`, `cache_control`, `cachePoint`, and gateway `caching: "auto"` depending on provider. |
| Kilo Code | `/home/postfiat/repos/agent-harness-study/kilocode/packages/opencode/src/provider/transform.ts` mirrors the OpenCode cache transform; `/home/postfiat/repos/agent-harness-study/kilocode/packages/llm/README.md` says prompt caching is on by default and translated per protocol. |
| Cline | `/home/postfiat/repos/agent-harness-study/cline/apps/vscode/src/core/api/transform/vercel-ai-gateway-stream.ts` and `openrouter-stream.ts` add `cache_control` to supported messages. |
| Hermes Agent | `/home/postfiat/repos/agent-harness-study/hermes-agent/run_agent.py` injects `cache_control` on system content; `gateway/run.py` caches agent instances per session to preserve prompt caching. |

PFTerminal now sends Chat `prompt_cache_key` for OpenRouter/Vercel Chat routes
and explicit OpenAI-compatible `cache_control` content blocks for the same
gateway model families that Cline marks: `anthropic/*`, `minimax/*`, and the
known OpenRouter Qwen/DeepSeek models that require explicit cache blocks.

This deliberately does not send raw `cache_control` to Baseten, Z.AI, Ambient,
or arbitrary custom OpenAI-compatible providers. Those providers need either
documented provider-specific fields or a native Anthropic-compatible wire path
before the runtime should emit Anthropic-shaped fields by default.

### 5. GLM Thinking Was Too Aggressive

For Z.AI/Ambient-style Chat requests, the fork mapped the configured `xhigh`
reasoning effort to provider `reasoning_effort: "max"` and enabled thinking on
every turn. That creates invisible reasoning-token latency before the first
visible text on some turns.

Fixed in:

- `codex-rs/core/src/client.rs`: normal `high`/`xhigh` config now sends
  `enable_thinking: false` and omits `reasoning_effort`.
- `codex-rs/core/src/client.rs` and `codex-rs/core/src/config/mod.rs`: explicit
  custom `max`, `deep`, or equivalent strings are preserved as opt-in deep
  reasoning.

This is not the main Vercel Responses issue, but it is still a contributor on
Z.AI/Ambient Chat paths. Live Z.AI tests showed three important constraints:
adding `prompt_cache_key` produced high reported cached-token counts but worse
wall time; omitting thinking fields disabled reasoning but also dropped reported
cache hits to zero and regressed wall time; explicit `enable_thinking: false`
disabled hidden reasoning without the severe omission regression, but still did
not restore GLM Chat cache hits. Z.AI/Ambient Chat therefore deliberately do not
receive `prompt_cache_key`, and the real remaining fix is a better wire path or
documented provider cache option.

The Claude pane Ambient path does not contradict this. It is a local
Anthropic-to-Ambient bridge, not a true Anthropic upstream. The bridge converts
Claude Messages into Ambient Chat Completions and collapses content blocks to
plain text, so Claude Code's Anthropic `cache_control` fields are not forwarded
to Ambient. That path is valuable for Claude CLI compatibility and tool
translation, but it is not an Ambient cache-control solution by itself. Live
pane smokes confirm that distinction: Ambient Kimi through the bridge took
41.3s on first turn and 4.7s on resume, while native Ambient Kimi was 7.05s on
first turn and 4.72s on resume. Ambient GLM fails through both paths with
Ambient HTTP 429/no worker available.

### 6. Wrapped Claude Code Used Anthropic Messages, Native Codex Did Not

Wrapped Claude Code was not fast because it had unique model access. It was fast
because the Claude CLI used Anthropic-compatible Messages routes, including
content-block `cache_control`, while native PFTerminal forced most non-OpenAI
models through Chat Completions or Responses request builders.

Fixed in:

- `codex-rs/model-provider-info/src/lib.rs`: `WireApi::Anthropic` and built-in
  Anthropic-wire providers for Z.AI, OpenRouter, Vercel, Vercel Fast, and
  Baseten.
- `codex-rs/codex-api/src/endpoint/anthropic_messages.rs`: Anthropic Messages
  SSE parser for text, usage/cache tokens, reasoning deltas, and streamed
  tool-use blocks.
- `codex-rs/core/src/client.rs`: Anthropic Messages request builder with
  system/tool/user `cache_control`, tool-call/tool-result replay, and deep
  thinking only when explicitly requested.
- `codex-rs/core/src/config/mod.rs`: provider defaults make
  `vercel-anthropic-fast` select `zai/glm-5.2-fast` without manual TOML.

Live native results confirm the path: `vercel-anthropic-fast` resumed with
20,224 cached tokens out of 20,304 input tokens, OpenRouter Anthropic resumed
with 10,112 cached tokens out of 20,313 input tokens, and Z.AI Anthropic resumed
with 11,392 cached tokens out of 20,298 input tokens.

### 7. Text Holdback Hid Visible Output

The Chat Completions compatibility endpoint buffered the beginning of text that
looked like serialized tool JSON. The old probe waited for up to 256 characters
before emitting ordinary JSON text that started with `{`, which could hide
several seconds of visible output on slow streams.

Fixed in:

- `codex-rs/codex-api/src/endpoint/chat_completions.rs`: the probe is reduced
  to 96 characters, and the heuristic still keeps serialized tool-call text
  hidden when early keys such as `arguments`, `name`, or `call_id` indicate a
  tool call whose `type` key may arrive later.

This did not explain the large setup delays above, but it improves perceived TPS
for JSON/code-heavy responses.

### 8. Permanent HTTP 4xx Errors Were Retried As Stream Outages

The session retry gate treated every `UnexpectedStatus` as retryable. That was
wrong for permanent provider/account errors such as Baseten HTTP 402. The user
experience was a slow five-attempt reconnect loop even though the provider had
already returned a final answer: the account cannot currently run that route.

Fixed in:

- `codex-rs/protocol/src/error.rs`: `UnexpectedStatus` is retryable only for
  transient HTTP statuses: request timeout, rate limit, and server errors.
- `codex-rs/protocol/src/error_tests.rs`: coverage proves HTTP 402 is not
  retryable while 408, 429, 502, 503, and 504 remain retryable.

## What Was Fixed In This Patch Set

| Area | Result |
| --- | --- |
| Provider preflight auth | No vault read just to fingerprint a stored provider key. |
| Provider auth resolution | Provider-scoped vault auth cached per model-provider instance; explicit env keys take precedence over stored provider keys. |
| Local secrets | Decrypted secrets file cached per backend, avoiding repeat decrypt in one process. |
| Vercel Responses state | `store: true`, persisted response ids, resume seeding, and HTTP `previous_response_id`. |
| Request body size for Vercel resumed turns | Large resumed turn shrank to one incremental input item when previous state matched. |
| HTTP client reuse | One shared `reqwest::Client` per model client. |
| Chat prompt cache key | OpenRouter/Vercel Chat request bodies can carry stable `prompt_cache_key`. |
| Chat explicit cache blocks | OpenRouter/Vercel Chat can mark supported Anthropic/MiniMax/Qwen-style messages with `cache_control`. |
| Native Anthropic Messages | Built-in `zai-anthropic`, `openrouter-anthropic`, `vercel-anthropic`, `vercel-anthropic-fast`, and `baseten-anthropic` providers can use Anthropic `cache_control` without wrapping Claude Code. |
| Model picker fast-route selection | The normal `/model` rows for Vercel GLM 5.2 Fast and OpenRouter GLM 5.2 now persist `vercel-anthropic-fast` and `openrouter-anthropic`, respectively, instead of the slower/default Chat or Responses routes. |
| GLM thinking default | Z.AI/Ambient `high`/`xhigh` config sends `enable_thinking: false`; custom `max`/`deep` enables thinking with provider `reasoning_effort: "max"`. |
| Chat text holdback | Ordinary JSON-like text flushes after a 96-character probe instead of waiting for 256 characters; serialized tool-call text still does not leak. |
| Permanent HTTP errors | HTTP 402 and other non-transient `UnexpectedStatus` failures now fail once instead of going through the stream reconnect loop. |
| Diagnostics | `PFTERMINAL_TRACE_STREAM_TIMING`, `PFTERMINAL_DUMP_RESPONSES_REQUEST`, `PFTERMINAL_DUMP_CHAT_REQUEST`, and `PFTERMINAL_DUMP_ANTHROPIC_REQUEST` expose setup, stream, and request-shape timings. Claude pane smoke reports now include first/resume durations plus first/resume artifact and audit paths. |

## Current State

Native Vercel Responses is no longer structurally doomed. With server-side
continuation active, the provider request shape is small and the measured
fresh-process wall time is close enough to wrapped Claude Code to continue
optimizing locally instead of abandoning native Codex.

Native Anthropic Messages is now the validated fast path for Vercel Fast and
Z.AI GLM. For Vercel Fast, native PFTerminal no longer needs to wrap Claude Code
to get Anthropic cache-control behavior. With `AI_GATEWAY_API_KEY` supplied in
the environment, native `vercel-anthropic-fast` matched the current wrapped
Claude Code wall-clock band in fresh-process `exec`: 3.53s native versus 3.60s
wrapped resume. The native run carried a much larger prompt, but most of it was
cached. In a real same-process no-alt-screen TUI run, the warm turn had no
client setup cost and completed the provider stream request at about 1.97s.
The automated real-PTY TUI harness reproduced the same shape with
`client_setup_ms=[0,0]` and two completed turns.
With vault-backed auth, fresh-process `exec` still pays local provider auth
setup; inside a long-lived TUI or app-server process, that cost is amortized.
The wrapped pane smoke runner now gives a direct backend comparison: Vercel
Fast resume through the Claude pane completed in 2.295s at the pane layer, and
OpenRouter resume through the Claude pane completed in 1.705s at the pane
layer. Native `vercel-anthropic-fast` and `openrouter-anthropic` are therefore
in the same warm-turn performance band on the measured routes.

Native Chat providers are improved but still not done. OpenRouter/Vercel Chat
now have the same cache-key/cache-marker request mechanics used by the
comparator agents for supported model families. Z.AI/Ambient Chat no longer get
the bad hidden-reasoning default, and they deliberately do not get
`prompt_cache_key` because the live test regressed wall time. Z.AI should use
`zai-anthropic` for GLM when latency matters. OpenRouter now has
`openrouter-anthropic` for the same direct Messages path exposed by the Claude
pane, and the normal OpenRouter GLM 5.2 picker row now routes there. The normal
Vercel GLM 5.2 Fast row now routes to `vercel-anthropic-fast`. Ambient still
needs documented cache control if we want to improve it beyond the current
native Chat behavior, but the current evidence does not show wrapped Claude
beating native Codex for working Ambient Kimi turns. The existing Claude pane
Ambient bridge drops Anthropic cache-control blocks while translating to Chat.
Baseten Anthropic is implemented but the current account key returns HTTP 402
through both native Codex and the wrapped Claude pane; native now fails fast
instead of retrying as a recoverable stream outage.

## Follow-Up Work

These are not blockers for the current wrapped-Claude parity result, but they
are the next useful optimizations.

1. Extend cache controls only where provider documentation or live smoke tests
   prove support. Do not send raw `cache_control` to strict OpenAI-compatible
   endpoints.
2. Add a documented/provider-native cache option for Ambient Chat, or route it
   through an Anthropic-compatible bridge when available. Explicit no-thinking
   is now wired for GLM Chat, but it does not recover cache hits.
3. Keep optimizing fresh-process startup separately. The remaining about 1.6s
   local setup cost is now visible and should be attacked with longer-lived app
   server/provider processes or eager provider auth warmup.

## Validation Commands

Commands used during this investigation and patch set:

```bash
cargo test -p codex-secrets
cargo test -p codex-model-provider
cargo test -p codex-core openrouter_ -- --nocapture
cargo test -p codex-core baseten_chat_completions_strips_strict_without_zai_reasoning_fields -- --nocapture
cargo test -p codex-core ambient_chat_completions_request_disables_thinking_unless_deep_reasoning_is_explicit ambient_responses_request_disables_thinking_unless_deep_reasoning_is_explicit -- --nocapture
cargo test -p codex-core load_config_ambient_provider_preserves_custom_max_reasoning_effort -- --nocapture
cargo test -p codex-api flushes_ordinary_json_text_after_short_probe parses_serialized_function_call_text_without_leaking_as_text -- --nocapture
just test -p codex-model-provider configured_provider_prefers_env_key_over_stored_provider_key -- --nocapture
just test -p codex-model-provider-info test_built_in_model_providers_include_zai_anthropic test_built_in_model_providers_include_baseten_anthropic test_built_in_model_providers_include_vercel_anthropic test_deserialize_anthropic_wire_api -- --nocapture
just test -p codex-model-provider-info test_built_in_model_providers_include_openrouter_anthropic -- --nocapture
just test -p codex-core load_config_baseten_anthropic_provider_uses_api_login load_config_vercel_anthropic_provider_uses_api_login load_config_vercel_anthropic_fast_defaults_to_fast_model load_config_zai_anthropic_provider_uses_api_login anthropic_messages_request_adds_cache_control_and_replays_tools -- --nocapture
just test -p codex-core load_config_openrouter_anthropic_provider_uses_api_login -- --nocapture
just test -p codex-api parses_text_usage_and_cache_tokens parses_streamed_tool_use -- --nocapture
just test -p codex-tui model_provider_for_selection_maps_cross_provider_models -- --nocapture
just test -p codex-tui model_picker_hides_fake_openai_models_and_shows_curated_provider_models model_picker_dismisses_after_selecting_openrouter_model_without_effort_choices model_picker_opens_openrouter_reasoning_options_for_gemini -- --nocapture
cargo test -p codex-protocol unexpected_status -- --nocapture
cargo check -p codex-api -p codex-core -p codex-model-provider-info -p codex-config
cargo check -p codex-tui -p codex-cli
cargo build -p codex-cli --bin pfterminal
./target/debug/pfterminal claude-pane-smoke --providers vercel-fast --cwd /home/postfiat/repos/PfTerminal
./target/debug/pfterminal claude-pane-smoke --providers openrouter --cwd /home/postfiat/repos/PfTerminal
./target/debug/pfterminal claude-pane-smoke --providers ambient-kimi --cwd /home/postfiat/repos/PfTerminal
./target/debug/pfterminal claude-pane-smoke --providers ambient --cwd /home/postfiat/repos/PfTerminal
./target/debug/pfterminal claude-pane-smoke --providers baseten --cwd /home/postfiat/repos/PfTerminal
scripts/native-provider-tui-benchmark --dump-request /tmp/pft_tui_native_benchmark_after_openrouter_request.json --output /tmp/pft_tui_native_benchmark_after_openrouter_result.json
scripts/native-provider-tui-benchmark --dump-request /tmp/pft_tui_native_completion_audit_request.json --output /tmp/pft_tui_native_completion_audit_result.json
scripts/native-provider-tui-benchmark --provider openrouter-anthropic --model z-ai/glm-5.2 --key-env OPENROUTER_API_KEY --vault-label provider/openrouter_api_key --dump-request-api anthropic --dump-request /tmp/pft_tui_openrouter_completion_audit_request.json --output /tmp/pft_tui_openrouter_completion_audit_result.json --turn1 'PFT_TUI_OPENROUTER_COMPLETION_AUDIT_1 Reply with exactly OK.' --turn2 'PFT_TUI_OPENROUTER_COMPLETION_AUDIT_2 Reply with exactly OK.'
scripts/native-provider-tui-benchmark --provider zai-anthropic --model glm-5.2 --key-env ZAI_API_KEY --vault-label provider/zai_api_key --dump-request-api anthropic --dump-request /tmp/pft_tui_zai_completion_audit_request.json --output /tmp/pft_tui_zai_completion_audit_result.json --turn1 'PFT_TUI_ZAI_COMPLETION_AUDIT_1 Reply with exactly OK.' --turn2 'PFT_TUI_ZAI_COMPLETION_AUDIT_2 Reply with exactly OK.'
scripts/native-provider-tui-benchmark --provider ambient --model moonshotai/kimi-k2.7-code --key-env AMBIENT_API_KEY --vault-label provider/ambient_api_key --dump-request-api chat --dump-request /tmp/pft_tui_ambient_kimi_request.json --output /tmp/pft_tui_ambient_kimi_result.json --turn1 'PFT_TUI_AMBIENT_KIMI_BENCH_1 Reply with exactly OK.' --turn2 'PFT_TUI_AMBIENT_KIMI_BENCH_2 Reply with exactly OK.'
```

Representative Vercel request-shape benchmark:

```bash
env PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_RESPONSES_REQUEST=1 \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="vercel"' \
  -m zai/glm-5.2-fast \
  --json resume 019f0e6c-1280-7ae0-8ac5-da47eeee0c40 \
  'PFT_VERCEL_STATE_FOURTEEN reply with exactly OK'
```

Representative OpenRouter Chat cache-control benchmark:

```bash
env PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_CHAT_REQUEST=/tmp/pft_chat_cache_request.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="openrouter"' \
  -m minimax/minimax-m3 \
  --json 'PFT_CHAT_CACHE_SMOKE reply with exactly OK'

env PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_CHAT_REQUEST=/tmp/pft_chat_cache_request_resume.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="openrouter"' \
  -m minimax/minimax-m3 \
  --json resume 019f0ea7-722f-7d63-8038-38668f03d6d2 \
  'PFT_CHAT_CACHE_SMOKE_2 reply with exactly OK'
```

Representative Z.AI GLM thinking/cache-key benchmark:

```bash
env PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_CHAT_REQUEST=/tmp/pft_zai_chat_enable_false_default.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="zai"' \
  -m glm-5.2 \
  --json 'PFT_ZAI_ENABLE_FALSE_DEFAULT_SMOKE reply with exactly OK'
```

Representative Ambient Chat smoke:

```bash
KEY=$(/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  vault auth-helper provider/ambient_api_key)

env AMBIENT_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_CHAT_REQUEST=/tmp/pft_ambient_native_chat_smoke_request.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="ambient"' \
  -m zai-org/GLM-5.2-FP8 \
  --json 'PFT_AMBIENT_NATIVE_CHAT_SMOKE reply with exactly OK'
```

The provider returned HTTP 429 on this run after the request reached the stream
path at about 1.47s. The dumped body confirmed the corrected GLM Chat shape:
`enable_thinking: false`, no `reasoning_effort`, no `prompt_cache_key`.

Representative Ambient Kimi Chat smoke:

```bash
KEY=$(/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  vault auth-helper provider/ambient_api_key)

env AMBIENT_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_CHAT_REQUEST=/tmp/pft_ambient_kimi_native_smoke_request.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="ambient"' \
  -m moonshotai/kimi-k2.7-code \
  --json 'PFT_AMBIENT_KIMI_NATIVE_SMOKE reply with exactly OK'

env AMBIENT_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_CHAT_REQUEST=/tmp/pft_ambient_kimi_native_resume_request.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec resume \
  -c 'model_provider="ambient"' \
  -m moonshotai/kimi-k2.7-code \
  --json 019f0f04-09ac-7b52-b5ca-8f96ce4ceedc \
  'PFT_AMBIENT_KIMI_NATIVE_SMOKE_2 reply with exactly OK'
```

Representative Baseten Anthropic fail-fast check:

```bash
KEY=$(/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  vault auth-helper provider/baseten_api_key)

env BASETEN_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_ANTHROPIC_REQUEST=/tmp/pft_baseten_anthropic_failfast_request.json \
  /usr/bin/time -f 'wall_seconds=%e exit_code=%x' \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="baseten-anthropic"' \
  -m zai-org/GLM-5.2 \
  --json 'PFT_BASETEN_ANTHROPIC_FAILFAST_RETRY_CHECK reply with exactly OK'
```

This returned one HTTP 402 error in 1.55s wall clock with no
`Reconnecting...` retry events.

Representative native Anthropic Messages benchmark:

```bash
env PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_ANTHROPIC_REQUEST=/tmp/pft_builtin_vercel_anthropic_fast_request_1.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="vercel-anthropic-fast"' \
  -m zai/glm-5.2-fast \
  --json 'PFT_BUILTIN_VERCEL_ANTHROPIC_FAST_SMOKE_1 reply with exactly OK'

env PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_ANTHROPIC_REQUEST=/tmp/pft_builtin_vercel_anthropic_fast_request_2.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec resume \
  -c 'model_provider="vercel-anthropic-fast"' \
  -m zai/glm-5.2-fast \
  --json 019f0ed5-def4-7e93-bdfb-206b2c9ffc7b \
  'PFT_BUILTIN_VERCEL_ANTHROPIC_FAST_SMOKE_2 reply with exactly OK'

KEY=$(/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  vault auth-helper provider/ai_gateway_api_key)

env AI_GATEWAY_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_ANTHROPIC_REQUEST=/tmp/pft_builtin_vercel_anthropic_fast_envkey_after_fix_request.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec resume \
  -c 'model_provider="vercel-anthropic-fast"' \
  -m zai/glm-5.2-fast \
  --json 019f0ed5-def4-7e93-bdfb-206b2c9ffc7b \
  'PFT_BUILTIN_VERCEL_ANTHROPIC_FAST_ENVKEY_AFTER_FIX reply with exactly OK'

env AI_GATEWAY_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_ANTHROPIC_REQUEST=/tmp/pft_vercel_fast_completion_audit_exec_request.json \
  /usr/bin/time -f 'wall_seconds=%e exit_code=%x' \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec resume \
  -c 'model_provider="vercel-anthropic-fast"' \
  -m zai/glm-5.2-fast \
  --json 019f0f0e-c6f5-75a2-9c35-fc879412c609 \
  'PFT_COMPLETION_AUDIT_NATIVE_VERCEL_FAST_RESUME reply exactly OK'
```

Representative built-in OpenRouter Anthropic benchmark:

```bash
KEY=$(/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  vault auth-helper provider/openrouter_api_key)

env OPENROUTER_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_ANTHROPIC_REQUEST=/tmp/pft_builtin_openrouter_anthropic_smoke_request.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec \
  -c 'model_provider="openrouter-anthropic"' \
  -m z-ai/glm-5.2 \
  --json 'PFT_BUILTIN_OPENROUTER_ANTHROPIC_SMOKE reply with exactly OK'

env OPENROUTER_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_ANTHROPIC_REQUEST=/tmp/pft_builtin_openrouter_anthropic_smoke_resume_request.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo exec resume \
  -c 'model_provider="openrouter-anthropic"' \
  -m z-ai/glm-5.2 \
  --json 019f0ef3-4a3a-70c1-81f7-835cc05412e1 \
  'PFT_BUILTIN_OPENROUTER_ANTHROPIC_SMOKE_2 reply with exactly OK'
```

Representative wrapped Claude Code comparison:

```bash
KEY=$(/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  vault auth-helper provider/ai_gateway_api_key)

env ANTHROPIC_BASE_URL='https://ai-gateway.vercel.sh' \
  ANTHROPIC_API_KEY='' \
  ANTHROPIC_AUTH_TOKEN="$KEY" \
  ANTHROPIC_MODEL='opus' \
  ANTHROPIC_DEFAULT_OPUS_MODEL='zai/glm-5.2-fast' \
  ANTHROPIC_DEFAULT_SONNET_MODEL='zai/glm-5.2-fast' \
  ANTHROPIC_DEFAULT_HAIKU_MODEL='zai/glm-5.2-fast' \
  ANTHROPIC_SMALL_FAST_MODEL='zai/glm-5.2-fast' \
  CLAUDE_CODE_SUBAGENT_MODEL='zai/glm-5.2-fast' \
  CLAUDE_CODE_AUTO_COMPACT_WINDOW='1000000' \
  API_TIMEOUT_MS='3000000' \
  CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS='1' \
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1' \
  CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK='1' \
  CLAUDECODE='' \
  /home/postfiat/.claude/local/claude \
  --bare -p --output-format stream-json --verbose \
  --permission-mode bypassPermissions \
  --exclude-dynamic-system-prompt-sections \
  --model opus --setting-sources project \
  --resume 33b5ac30-84db-4813-8952-c9842eae0b14 \
  'PFT_WRAPPED_CLAUDE_VERCEL_FAST_BENCH_2 reply with exactly OK'
```

Representative wrapped Claude pane backend comparison:

```bash
/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  claude-pane-smoke \
  --providers vercel-fast \
  --cwd /home/postfiat/repos/PfTerminal

/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  claude-pane-smoke \
  --providers openrouter \
  --cwd /home/postfiat/repos/PfTerminal
```

Latest reports:

```text
/home/postfiat/.pfterminal/panes/smoke-reports/claude-pane-smoke-1782662799.json
/home/postfiat/.pfterminal/panes/smoke-reports/claude-pane-smoke-1782662869.json
```

Representative same-process native TUI benchmark:

```bash
KEY=$(/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  vault auth-helper provider/ai_gateway_api_key)

env AI_GATEWAY_API_KEY="$KEY" \
  PFTERMINAL_TRACE_STREAM_TIMING=1 \
  PFTERMINAL_DUMP_ANTHROPIC_REQUEST=/tmp/pft_tui_native_anthropic_request.json \
  /home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal \
  --yolo --no-alt-screen \
  -c 'model_provider="vercel-anthropic-fast"' \
  -m zai/glm-5.2-fast \
  'PFT_TUI_NATIVE_ANTHROPIC_1 Reply with exactly OK.'
```

After the first turn rendered `OK`, the same PTY submitted
`PFT_TUI_NATIVE_ANTHROPIC_2 Reply with exactly OK.` followed by carriage
return. Shutdown reported:

```text
Token usage: total=10,359 input=10,355 (+ 10,304 cached) output=4
To continue this session, run pfterminal resume 019f0ee2-4878-71d0-afec-083803d0310e
```

Automated same-process native TUI benchmark:

```bash
scripts/native-provider-tui-benchmark \
  --dump-request /tmp/pft_tui_native_benchmark_after_openrouter_request.json \
  --output /tmp/pft_tui_native_benchmark_after_openrouter_result.json
```

Latest result:

```json
{
  "client_setup_ms": [0, 0],
  "exit_code": 0,
  "provider": "vercel-anthropic-fast",
  "model": "zai/glm-5.2-fast",
  "resume_hint": "To continue this session, run pfterminal resume 019f0ef4-73df-7db0-b9dc-e93521ef689c",
  "second_prompt_submitted": true,
  "stream_request_ok_ms": [2429, 1769],
  "success": true,
  "token_usage": {
    "cached": 20608,
    "input": 57,
    "output": 4,
    "total": 61
  },
  "turns_completed": 2
}
```
