# Unified TPS Estimates

Status: draft implementation spec.

This document defines how PFTerminal should calculate and render tokens per
second (TPS). The goal is to prevent the UI from showing multiple incompatible
TPS numbers, such as a live `Working` row value that disagrees with the footer.

The core requirement is simple:

```text
There must be one visible TPS value, backed by one estimator, with one
definition.
```

Normative requirements in this document use `must`. File paths, suggested type
names, and recommended ownership locations are implementation guidance for the
current PFTerminal codebase; they can change if the behavior, data contract, and
tests remain equivalent.

Implementation summary:

1. Ship one footer TPS value backed by one estimator.
2. Remove TPS from the running `Working` indicator.
3. Render `~` whenever the visible aggregate includes stream-estimated tokens.
4. Use provider usage when available and replace earlier stream estimates.
5. Add core model-call lifecycle events before claiming precise TPS.

## Current Problem

The TUI has several adjacent concepts that can be mistaken for TPS:

- Turn lifecycle state in `tui/src/chatwidget/turn_runtime.rs`
- Streaming assistant and reasoning deltas in `tui/src/chatwidget/streaming.rs`
- Token accounting in `protocol/src/protocol.rs` via `TokenUsage` and
  `TokenUsageInfo`
- Footer/status-line rendering in `tui/src/chatwidget/status_surfaces.rs`,
  `tui/src/bottom_pane/status_line_setup.rs`, and
  `tui/src/bottom_pane/footer.rs`
- The transient running indicator in `tui/src/status_indicator_widget.rs`

If each layer computes throughput independently, the same screen can show two
different values under the same `TPS` label. That is a correctness bug, not a
styling issue.

## Validity Levels

There are two validity levels. The UI must not pretend they are equivalent.

### Precise TPS

Precise TPS requires explicit model-call lifecycle events from core/provider
code:

```text
ModelCallStarted
ModelCallTokenUsage
ModelCallCompleted
ModelCallFailed
```

Only this level can guarantee that tool execution time, approval wait time, and
inter-call idle time are excluded from the denominator.

### Interim TPS

Interim TPS is a best-effort TUI fallback using current turn and streaming
hooks. It is allowed only until model-call lifecycle events exist.

Interim TPS must still use the same estimator and the same footer label, but it
has known limitations:

- It may not perfectly exclude tool execution time.
- It may start at a turn boundary rather than an exact model-call boundary.
- It may estimate tokens from streamed text when provider usage is missing.

The implementation must keep the source metadata internally so tests and logs
can distinguish `ProviderUsage` from `StreamEstimate`. The user-facing footer
still shows one value. Provider-backed values render without a marker:

```text
TPS: N.N tok/s
```

Stream-estimated values render with a lightweight approximation marker:

```text
TPS: ~N.N tok/s
```

The marker is part of the one TPS value. It is not a second metric. It means
the aggregate contains at least one estimated input, not that every token in the
window was estimated.

## Milestones

### Milestone 1: Unified Interim TPS

This milestone is shippable before core model-call lifecycle events exist.

It must:

- Add one estimator.
- Render one footer TPS label.
- Remove TPS from `StatusIndicatorWidget`.
- Use stream deltas as `StreamEstimate` inputs.
- Render `TPS: ~N.N tok/s` for stream-estimated or mixed-source windows.
- Replace estimates with provider usage when usage is available.
- Keep tests explicit that this is interim, not precise model-call TPS.

This milestone must not claim to exclude tool execution time perfectly, because
turn-level hooks cannot prove exact model-call boundaries.

### Milestone 2: Precise Model-Call TPS

This milestone adds core/provider lifecycle events.

It must:

- Emit model-call start/completion/failure events below the TUI layer.
- Correlate provider usage by call id.
- Exclude tool execution, approval wait, user idle time, and inter-call idle
  time from the denominator.
- Preserve the same estimator and footer rendering contract from Milestone 1.

Only this milestone may claim precise model-call TPS.

## Definitions

### Model Call

A model call is one request to a model provider and the corresponding response
stream/completion. A user turn can contain multiple model calls.

Example:

```text
user prompt
model call 1 starts
model call 1 streams a tool request
model call 1 completes
tool executes
model call 2 starts
model call 2 streams final answer
model call 2 completes
turn completes
```

Only `model call 1` and `model call 2` contribute TPS samples. Tool execution
does not contribute generated tokens and should not contribute duration once
precise lifecycle events exist.

### Generated Tokens

Generated tokens are model-produced output tokens for one model call.

Provider source:

- Prefer per-call usage from `TokenUsageInfo.last_token_usage`.
- Count `output_tokens`.
- Count `reasoning_output_tokens` only when that field is provider-reported for
  the same model call.
- Do not use `total_token_usage` as a sample directly. If only cumulative usage
  is available, compute the sample by subtracting the call-start cumulative
  usage from the call-end cumulative usage.

Fallback source:

- If provider usage is absent, estimate from streamed assistant and reasoning
  deltas.
- Prefer an existing tokenizer already available to the codebase or provider
  adapter. If no tokenizer is available, use the explicit crude fallback:

```text
estimated_tokens = ceil(streamed_unicode_scalar_count / 4.0)
```

This fallback is intentionally crude and biased by language/content shape. It is
allowed only because the footer will mark the value as estimated with `~`.
All fallback implementations must feed the same estimator and mark the sample as
`StreamEstimate`.

`streamed_unicode_scalar_count` means the count of Unicode scalar values in the
concatenated assistant-output and reasoning-output stream deltas for the current
model call. Do not include tool stdout/stderr, tool-call JSON arguments, prompt
text, status text, or UI labels.

### Active Generation Duration

Active generation duration is the elapsed time for one model call.

For precise TPS:

- Starts at `ModelCallStarted`.
- Ends at `ModelCallCompleted` or `ModelCallFailed`.
- Includes provider queueing and time-to-first-token.
- Includes pauses inside an active model response.
- Excludes tool execution time.
- Excludes approval wait time.
- Excludes user idle/composer time.
- Excludes time between model calls.

For interim TPS:

- Starts at the best available current hook, initially
  `tui/src/chatwidget/turn_runtime.rs::on_task_started`.
- Ends at `tui/src/chatwidget/turn_runtime.rs::on_task_complete` or reset paths.
- Is explicitly less precise because turn boundaries are not guaranteed to be
  model-call boundaries.

All internal durations are stored as `Duration` or nonzero milliseconds derived
from `Duration`. Do not store signed duration values in estimator state.

## UI Contract

There is exactly one visible TPS value.

Footer example:

```text
glm-5.2 deep · ~/repos · Post Fiat Terminal · TPS: 28.7 tok/s
```

Footer example while only stream-estimated data is available:

```text
glm-5.2 deep · ~/repos · Post Fiat Terminal · TPS: ~28.7 tok/s
```

Running row example:

```text
Working (35s • esc to interrupt)
```

Rules:

- Do not render `TPS` in `StatusIndicatorWidget`.
- Do not render both live TPS and rolling TPS under the same label.
- The footer TPS must update during an active model call or interim active
  generation window.
- When no samples exist, render `TPS: -- tok/s`.
- When the visible value is backed only by `StreamEstimate` samples, render
  `TPS: ~N.N tok/s`.
- When the visible value includes any `ProviderUsage` sample and any
  `StreamEstimate` sample, render `TPS: ~N.N tok/s` because the aggregate is
  still partially estimated.
- When the visible value is backed only by `ProviderUsage` samples, render
  `TPS: N.N tok/s`.
- If generation is active, the displayed value includes the current provisional
  sample.
- If generation is inactive, the displayed value uses completed samples only.

If a future design requires TPS in the running row, it must render the exact
same string returned by the unified estimator. It must not compute its own
value.

## Estimator Ownership

Create one estimator and make every TPS surface read from it.

Recommended location:

```text
tui/src/chatwidget/tps.rs
```

The estimator should be owned by `ChatWidget` in `tui/src/chatwidget.rs` and
initialized from `tui/src/chatwidget/constructor.rs`.

Avoid separate `LiveTpsTracker` and `RollingTpsTracker` fields. Those names
encode the split that caused incoherent UI values.

Suggested state:

```rust
pub(crate) struct TpsEstimator {
    completed_samples: VecDeque<TpsSample>,
    active: Option<ActiveModelCall>,
}

pub(crate) struct ActiveModelCall {
    call_id: Option<ModelCallId>,
    model_id: String,
    provider_id: Option<String>,
    started_at: Instant,
    generated_tokens: u64,
    source: TpsTokenSource,
}

pub(crate) struct TpsSample {
    call_id: Option<ModelCallId>,
    model_id: String,
    provider_id: Option<String>,
    generated_tokens: u64,
    duration: Duration,
    source: TpsTokenSource,
}

pub(crate) enum TpsTokenSource {
    ProviderUsage,
    StreamEstimate,
}
```

Rules:

- `generated_tokens` is always the current best non-negative token count for
  the call.
- Provider payloads may use signed or nullable fields. Normalize them at the
  ingestion boundary before updating estimator state.
- `source` tracks whether the count came from provider usage or stream
  estimation.
- Provider usage replaces stream estimates for the same call.
- `duration` must be greater than zero before a sample can affect the displayed
  value.
- Interrupted or failed calls are discarded unless core has already emitted a
  completed model-call sample.

The public UI-facing API should expose one formatted label:

```rust
impl TpsEstimator {
    pub(crate) fn label(&self, now: Instant) -> String;
}
```

All UI renderers must call this same API.

## Weighted Rolling TPS

The footer value is a duration-weighted rolling throughput over a completed
window plus the active provisional sample.

The completed window contains at most the last 10 completed model-call samples.
The active provisional sample is appended outside that cap, so the visible
calculation can contain up to 11 samples while a model call is active. When the
active call finalizes, it becomes a completed sample and is then subject to the
10-completed-sample cap.

The displayed value is not the arithmetic mean of per-call TPS values. Equal
weighting of a 1-second call and a 100-second call is misleading.

Algorithm:

```text
visible_samples = last up to 10 completed samples
if active sample exists:
    active_duration = now.saturating_duration_since(active.started_at)
    if active.generated_tokens > 0 and active_duration > 0:
        visible_samples += provisional sample using active_duration
if visible_samples is empty:
    return "TPS: -- tok/s"

tokens = sum(sample.generated_tokens for sample in visible_samples)
seconds = sum(sample.duration.as_secs_f64() for sample in visible_samples)
if tokens == 0 or seconds <= 0:
    return "TPS: -- tok/s"
prefix = "~" if any(sample.source == StreamEstimate for sample in visible_samples) else ""
return "TPS: {prefix}{tokens / seconds:.1} tok/s"
```

Never divide when `seconds <= 0`, even if `tokens > 0`. Render the placeholder
until the active duration or completed sample duration is positive.

The active provisional sample does not count against the completed-sample cap
until it finalizes.

The default window size is 10 because it is small enough to respond to a model
or provider change within a session, but large enough to avoid one tiny response
dominating the displayed value. If this number becomes configurable, keep 10 as
the default and test both configured and default behavior. For the initial
implementation, treat 10 as a named constant rather than a user-facing setting.

## Provider Usage Override

Provider usage overrides stream estimates for the same model call.

Example:

```text
stream estimate during call: 80 tokens, source=StreamEstimate
provider final usage: 120 output + 30 reasoning output
final sample: 150 tokens, source=ProviderUsage
```

Do not add the estimate and provider usage together. Replace the estimate.

If provider usage arrives after a provisional sample has already affected the
footer, update the active sample in place and redraw the footer. If usage
arrives after finalization, update the matching completed sample by `call_id`
when available. Without a `call_id`, only update if the sample is unambiguously
the most recent active/finalized sample.

## Core Event Requirements

Precise TPS belongs at the model transport/protocol boundary, not in the TUI.
The TUI can render and maintain the estimator, but core/provider code must emit
model-call boundaries.

Add events to the protocol layer in `protocol/src/protocol.rs`:

```text
ModelCallStarted {
    call_id,
    model,
    provider,
}

ModelCallTokenUsage {
    call_id,
    usage,
}

ModelCallCompleted {
    call_id,
}

ModelCallFailed {
    call_id,
    error_kind,
}
```

Implementation path:

1. Add protocol event types in `protocol/src/protocol.rs`.
2. Emit `ModelCallStarted` immediately before dispatching a provider request in
   the core/provider request path.
3. Emit streamed deltas as they are already emitted today.
4. Emit `ModelCallTokenUsage` when provider usage is parsed. For existing
   `TokenUsageInfo`, use `last_token_usage` when it corresponds to the current
   model call.
5. Emit `ModelCallCompleted` when the provider response completes normally.
6. Emit `ModelCallFailed` on retry exhaustion, transport failure, malformed
   stream termination, or cancellation before completion.
7. Route those events through the existing TUI event handling path into
   `ChatWidget`.

The exact provider adapter files may vary by wire API, but this must be done
below the TUI layer so tool execution and UI waiting are not part of the model
duration.

## Interim TUI Integration

Until core lifecycle events exist, the TUI can support one interim estimator
using current hooks:

- Start provisional generation in
  `tui/src/chatwidget/turn_runtime.rs::on_task_started`.
- Update estimated generated tokens from deltas in
  `tui/src/chatwidget/streaming.rs::on_agent_message_delta` and
  `on_agent_reasoning_delta`.
- Replace estimated token counts with provider usage in
  `tui/src/chatwidget.rs::set_token_info` / `apply_token_info`.
- Finalize the active sample in
  `tui/src/chatwidget/turn_runtime.rs::on_task_complete`.
- Reset without recording on interrupt/failure in
  `tui/src/chatwidget/turn_runtime.rs::finalize_turn`.

This interim mode must be represented in code and tests as `StreamEstimate` or
`Interim`, not silently treated as precise provider TPS.

## Redraw Requirements

The footer must not remain stale while generation is active.

Required behavior:

- Any estimator update that changes the formatted label must refresh status
  surfaces.
- Streaming deltas must schedule redraws for active provisional TPS updates.
- Redraws may be throttled to avoid excessive terminal work, but the footer
  should update at least once per second while an active generation window has
  new token data.
- `StatusLineItem::Tps` must render the estimator's current `label(now)` rather
  than a cached completed-turn value.

Relevant code paths:

- `tui/src/chatwidget/status_surfaces.rs`
- `tui/src/bottom_pane/footer.rs`
- `tui/src/bottom_pane/status_line_setup.rs`
- `tui/src/bottom_pane/status_surface_preview.rs`

## Status-Line Integration

The footer status line is configured through:

- `tui/src/bottom_pane/status_line_setup.rs`
- `tui/src/bottom_pane/status_surface_preview.rs`
- `tui/src/bottom_pane/status_line_style.rs`
- `tui/src/chatwidget/status_surfaces.rs`

Add or keep one status item:

```rust
StatusLineItem::Tps
```

`StatusLineItem::Tps` must render `ChatWidget.tps_estimator.label(now)`.

The default status line in `tui/src/chatwidget.rs` may include `tps`, but tests
must not hardcode a provider/model string. They should assert only stable
behavior:

- brand appears
- `TPS: -- tok/s` appears before samples
- `TPS: N.N tok/s` appears after samples
- `TPS: ~N.N tok/s` appears when samples are stream-estimated

## Running Indicator Integration

`tui/src/status_indicator_widget.rs` should not render TPS.

`Working (Ns • esc to interrupt)` is elapsed task state, not model throughput.
Keeping TPS out of this widget prevents duplicate labels and prevents users from
comparing task elapsed time against model-call throughput.

## Edge Cases

- No completed samples and no active sample: show `TPS: -- tok/s`.
- Active sample with zero generated tokens: show the completed-sample weighted
  value if one exists, otherwise `TPS: -- tok/s`.
- Active sample with zero elapsed duration: ignore the active sample until
  duration is positive.
- Provider usage with negative token fields: clamp to zero at the ingestion
  boundary before updating estimator state and log a sanitized warning without
  raw prompt/response text.
- Interrupted call without completed model-call event: discard the active
  provisional sample.
- Failed call after provider usage and completion event: preserve only samples
  that core marked completed.
- Provider usage for an unknown call id: ignore it and log a sanitized warning.
- Model/provider switch: do not clear completed samples solely because the model
  changed, but preserve model/provider metadata on each sample for diagnostics.
- Resume/replay: do not fabricate TPS samples from historical transcript text.

## Audit Requirements

Before landing this feature, audit and remove competing TPS render paths:

```bash
rg -n "TPS|tps|tokens per second|tok/s|throughput" codex-rs/tui/src codex-rs/core/src codex-rs/protocol/src
```

Requirements:

- There is one estimator implementation.
- There is one footer/status-line renderer for TPS.
- `StatusIndicatorWidget` does not compute or render TPS.
- No test freezes a literal provider/model display string when it is not testing
  model display formatting.
- No debug log prints raw streamed text, prompts, API keys, account ids, or raw
  provider payloads as part of TPS estimation.

## Logging And Privacy

TPS telemetry must not log secrets or content.

Allowed log fields:

- source: `ProviderUsage` or `StreamEstimate`
- generated token count
- duration
- model id
- provider id, if already non-secret and used elsewhere in diagnostics
- call id

Disallowed log fields:

- raw prompts
- raw completion text
- raw reasoning text
- API keys or credential paths
- provider account identifiers unless explicitly redacted or hashed

## Acceptance Criteria

- Only one visible `TPS:` label is present in the TUI.
- Estimated values use the single visible label with an approximation marker:
  `TPS: ~N.N tok/s`.
- Provider-backed values use the same label without the approximation marker:
  `TPS: N.N tok/s`.
- The footer TPS changes during active generation.
- The footer does not remain stale from a previous call while a new call is
  streaming.
- The displayed TPS uses `sum(tokens) / sum(duration)` over the visible rolling
  window, not an unweighted mean of per-call TPS values.
- Provider token usage overrides stream estimates for the same call.
- Missing provider usage still produces a fallback estimate from streamed text.
- Interrupts/failures clear unfinished active provisional samples.
- Tool execution time does not count against TPS in precise mode.
- Interim mode is explicitly marked in code/tests as less precise until
  model-call lifecycle events exist.
- The rolling window uses the last 10 completed model calls plus the current
  provisional call.
- Tests do not hardcode provider/model display strings.
- Status surfaces redraw while active generation updates the TPS label.

## Test Plan

Add focused tests near existing status and layout coverage in
`tui/src/chatwidget/tests/status_and_layout.rs`:

- `tps_label_is_placeholder_before_samples`
- `tps_positive_tokens_zero_duration_renders_placeholder`
- `tps_stream_estimate_renders_approximation_marker`
- `tps_provider_usage_renders_without_approximation_marker`
- `tps_mixed_window_renders_approximation_marker`
- `tps_footer_updates_during_active_generation`
- `tps_completed_sample_enters_weighted_window`
- `tps_weighted_window_keeps_last_10_completed_calls`
- `tps_weighted_window_does_not_equal_unweighted_mean_for_skewed_calls`
- `tps_provider_usage_overrides_stream_estimate`
- `tps_missing_usage_uses_stream_estimate`
- `tps_interrupt_discards_unfinished_active_sample`
- `status_indicator_does_not_render_tps`
- `tps_status_surface_redraws_during_active_generation`

Time-dependent tests should use a deterministic clock or explicit `Instant`
values passed into `TpsEstimator::label(now)`.

Add protocol/core tests when model-call events are added:

- `model_call_started_emitted_before_provider_request`
- `model_call_completed_emitted_after_provider_completion`
- `model_call_failed_emitted_on_transport_failure`
- `model_call_usage_correlates_by_call_id`
- `tool_execution_time_is_outside_model_call_duration`

Add snapshot coverage only for stable UI shape. Do not snapshot literal model
names unless the test is specifically about model display formatting.

## Non-Goals

- TPS is not a benchmark framework.
- TPS is not a provider billing report.
- TPS does not explain latency by itself; TTFT, stall ratio, and tool latency are
  separate metrics.
- TPS should not expose provider account IDs, API keys, prompts, or raw response
  text in logs.

## Migration Notes From Current Work

If current code contains separate live and rolling trackers:

- Replace them with one `TpsEstimator`.
- Remove status-row inline TPS overrides from `tui/src/bottom_pane/mod.rs`.
- Remove TPS rendering from `tui/src/status_indicator_widget.rs`.
- Keep `StatusLineItem::Tps`, but make it read only from the unified estimator.
- Keep fallback stream estimation, but make it feed the unified estimator rather
  than a separate live display path.
- Add core model-call lifecycle events before claiming precise model-call TPS.
