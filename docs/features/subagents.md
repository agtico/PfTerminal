# PFTerminal Subagents

## Context

PFTerminal is a fork of Codex (`codex-rs/`) that retargets the host onto
third-party model providers (OpenRouter, ZAI, Ambient) rather than the default
OpenAI Responses API. Most behavior is inherited unchanged; the divergence that
matters here is **provider capabilities**: the default OpenAI path advertises a
capability that the third-party providers do not, and that capability is what
gates the subagent tool surface.

This document records what was verified against the `PfTerminal/codex-rs`
source on 2026-06-24, why a real PFTerminal session can truthfully report that
it has no subagent tools, and the work to make subagents usable across all
configured providers.

## Complete

- [x] Verified subagent delegation in a current Codex host session by spawning an `explorer` agent, waiting for its report, and closing it.
- [x] Confirmed current Codex behavior: subagents are enabled by default, are only spawned when explicitly requested, and are managed with `/agent`.
- [x] Traced PFTerminal's inherited multi-agent implementation, feature flags, slash commands, role loading, and tool exposure path **to source**, including the provider-capability gate that actually hides the tools.
- [x] Identified the true mechanism (below) for why a PFTerminal session can truthfully say it has no subagent tools even though the repo contains subagent support. The earlier `tool_search` explanation was a hypothesis; source confirmed the gate is the `namespace_tools` provider capability.

## To Do

- [ ] Add a `/subagents` diagnostics view that reports the effective provider capability and whether subagent tools survived serialization for the active session.
- [x] Ship V1 spawn tools that serialize as plain functions (not `ToolSpec::Namespace`) so they survive providers whose `namespace_tools` capability is false, or document V2 as the supported cross-provider path.
- [ ] Ship a small default project agent set for common PFTerminal workflows, starting with code review, repo exploration, and test triage.
- [x] Add a smoke test that asserts the spawn tool is visible in the model-visible spec set for at least one third-party provider fixture (OpenRouter, ZAI, Ambient).

## Goal

PFTerminal should support a basic Codex-style subagent workflow without users
needing to understand provider-capability flags.

```text
Spawn a subagent to review this branch for security issues.
```

PFTerminal should then create a bounded subagent thread, give it a task, let it
run with the same sandbox/provider context, return a concise result, and show
active and completed subagents through `/agent` or `/subagents`. If subagents
are unavailable, PFTerminal should name the exact missing condition, not leave
the model to guess from its visible tool list.

## Tested Codex Behavior

The current Codex host exposed subagent tools only after `tool_search` was used.
That exposed a `multi_agent_v1` namespace with `spawn_agent`, `send_input`,
`wait_agent`, `resume_agent`, and `close_agent`. A read-only `explorer`
subagent was spawned against the PFTerminal repo, returned file-level findings,
and was then closed.

This confirms the mechanics: subagents are a host/runtime tool surface, not a
model ability by itself. **Whether the model can see that surface is decided in
the tool plan by provider capability, as the next section shows.**

## Verified Mechanics

All claims below were read from `PfTerminal/codex-rs`.

| Area | Verified behavior | Source |
| --- | --- | --- |
| Stable feature | `multi_agent` maps to `Feature::Collab` and is default-enabled. | `codex-rs/features/src/lib.rs` |
| Under-development feature | `multi_agent_v2` is present but default-disabled. | `codex-rs/features/src/lib.rs` |
| Version selection | V2 if `MultiAgentV2` enabled, else V1 if `Collab` enabled, else Disabled. | `codex-rs/core/src/config/mod.rs:1369` (`multi_agent_version_from_features`) |
| Built-in roles | `default`, `explorer`, and `worker`. | `codex-rs/core/src/agent/role.rs` |
| Custom roles | Loaded from `agents/*.toml` under each config layer. | `codex-rs/core/src/config/agent_roles.rs` |
| V1 spawn spec | Built as `ToolSpec::Namespace` named `multi_agent_v1` wrapping `spawn_agent`. | `codex-rs/core/src/tools/handlers/multi_agents_spec.rs:72` (`create_spawn_agent_tool_v1`) |
| V2 spawn spec | Built as a plain `ToolSpec::Function` named `spawn_agent` (no namespace wrapper). | `codex-rs/core/src/tools/handlers/multi_agents_spec.rs:96` (`create_spawn_agent_tool_v2`) |
| Visibility filter | Namespace specs are dropped unless `namespace_tools` capability is true. | `codex-rs/core/src/tools/spec_plan.rs:269` |
| Capability gate | `namespace_tools_enabled()` reads `provider.capabilities().namespace_tools`. | `codex-rs/core/src/tools/spec_plan.rs:351` |
| Provider caps | `namespace_tools: false` for Ambient, ZAI, OpenRouter, Baseten, and Vercel; `true` only by default (OpenAI path). | `codex-rs/model-provider/src/provider.rs:228` (`capabilities()`) |
| V1 exposure | `Deferred` only when both `search_tool_enabled` and `namespace_tools_enabled`; otherwise `Direct`. | `codex-rs/core/src/tools/spec_plan.rs:856` (`add_collaboration_tools`) |
| Depth enforcement | `max_depth` is enforced at spawn time: `next_thread_spawn_depth` is compared with `exceeds_thread_spawn_depth_limit`, which gates `collab_tools_enabled`. | `codex-rs/core/src/tools/spec_plan.rs:362` and `codex-rs/core/src/agent/registry.rs:71` |
| Thread cap | `DEFAULT_AGENT_MAX_THREADS = 6`; enforced in `reserve_spawn_slot`. | `codex-rs/core/src/config/mod.rs:204`, `codex-rs/core/src/agent/registry.rs` |
| V2 conflict | Setting `agents.max_threads` while `multi_agent_v2` is enabled is a hard error. | `codex-rs/core/src/config/mod.rs:1379` (`validate_multi_agent_v2_config`) |
| Slash commands | `/agent` and `/subagents` open the agent picker/thread surface. | `codex-rs/tui/src/slash_command.rs` |

## Why It Does Not Work On Third-Party Providers

This corrects the earlier `tool_search` hypothesis. The real failure path is:

1. `multi_agent` (`Collab`) is enabled, so V1 is selected.
2. `add_collaboration_tools` registers `SpawnAgentHandler` and friends. Exposure is `Deferred` when the provider supports both the search tool and namespace tools, otherwise `Direct`.
3. The V1 `spawn_agent` spec is built as `ToolSpec::Namespace` (`multi_agent_v1`).
4. At plan finalization, `merge_into_namespaces(...)` keeps or drops each spec: a `ToolSpec::Namespace` survives only if `namespace_tools_enabled(turn_context)` is true (`spec_plan.rs:269`).
5. Ambient, ZAI, OpenRouter, Baseten, and Vercel all advertise `namespace_tools: false` (`provider.rs:228`).
6. Therefore the entire `multi_agent_v1` namespace is filtered out of the model-visible spec set on those providers — regardless of exposure, and regardless of `tool_search`.

So the product bug is **not** "deferred tools need `tool_search` and the
provider lacks it." It is that **V1 emits a namespace-wrapped spec and the
third-party providers cannot serialize namespaces.** `tool_search` was a red
herring: it only governs deferred exposure, not whether the spec survives the
capability filter. The fix must address the spec shape, not the search tool.

Note the asymmetry the doc previously hand-waved: V2's `spawn_agent` is a plain
`ToolSpec::Function` (`multi_agents_spec.rs:96`) and is **not** caught by the
namespace filter. V2 tools therefore survive on OpenRouter/ZAI/Ambient where V1
does not. This is now a verified fork in behavior, not a guess.

## Risks and Rollback

Any change to tool serialization is provider-facing and must be reversible.

- **Wire-compatibility.** Flattening V1's namespace into a plain function changes the tool name the model sends (`multi_agent_v1.spawn_agent` -> `spawn_agent`) and risks a name collision with V2. The P0 change must namespace via a non-`ToolSpec::Namespace` mechanism (for example a `spawn_agent_v1` function name) or must ship exclusively under V2.
- **Safety of direct exposure.** Making spawn tools directly visible on providers that previously hid them increases prompt-token cost (every turn now lists spawn/wait/send/close) and widens the surface the model can reach. The existing `agents.max_threads` and `agents.max_depth` guards remain the binding limits; the P0 change must not weaken them.
- **Rollback.** Every P0 change should be behind the `multi_agent` feature flag so that disabling the feature restores today's behavior exactly. A failing smoke test (subagent tool visible on a provider where it was previously dropped) should block merge.
- **Provider regression.** The OpenAI Responses path already advertises `namespace_tools: true`; flattening the spec must not change the OpenAI wire format or break the working `/agent` flow on first-party sessions.

## Required User Experience

`/subagents` should report, for the active session:

| Field | Example |
| --- | --- |
| Feature state | `multi_agent: enabled` |
| Runtime version | `V1` |
| Provider | `openrouter`, `zai`, `ambient`, or `openai` |
| `namespace_tools` capability | `true` / `false` |
| Spawn tool visible to model | `yes` / `no — filtered by namespace_tools=false` |
| Limits | `max_threads=6`, `max_depth=1` |
| Roles | `default`, `explorer`, `worker`, plus project/global custom roles |
| Action needed | e.g. `enable V2, or ship flattened V1 spawn tool` |

If unavailable, surface a deterministic error that names the real cause:

```text
Subagents are enabled but the spawn tool is not visible in this session.
Reason: provider reports namespace_tools=false and V1 emits a namespace-wrapped spec that is filtered out.
Try: enable multi_agent_v2, or update PFTerminal to ship a plain-function spawn tool for this provider.
```

## Implementation Plan

### P0: Fix the Spec Shape (Provider-Compatible)

The unit of work is the tool spec, not the search tool.

V1 `spawn_agent` is a `ToolSpec::Namespace` (`multi_agents_spec.rs:72`). The
filter at `spec_plan.rs:269` drops namespace specs when
`provider.capabilities().namespace_tools` is false. Third-party providers all
return false here (`provider.rs:228`). Therefore V1 spawn tools are invisible
on OpenRouter/ZAI/Ambient.

Two options, pick one:

1. **Flatten V1 spawn** to a plain `ToolSpec::Function` with a non-colliding
   name (for example `spawn_agent_v1`), so it is not caught by the namespace
   filter. Mirror `send_input`/`wait_agent`/`close_agent`. Keep them guarded by
   `agents.max_threads` (`registry.rs`) and `agents.max_depth` (`spec_plan.rs:362`).
2. **Promote V2** as the supported cross-provider path, since V2 already emits
   plain `ToolSpec::Function` specs (`multi_agents_spec.rs:96`) and survives
   the filter. This requires resolving the `agents.max_threads` conflict
   (`config/mod.rs:1379`) and validating the V2 TUI surface.

Either way, the acceptance check is the same: on a third-party provider
fixture, the spawn tool must appear in the model-visible spec set produced by
`merge_into_namespaces`.

### P1: `/subagents` Diagnostics

Show feature state, selected version, provider, the `namespace_tools`
capability, whether the spawn tool survived serialization, limits, and roles.
Keep the field set to the rows above; anything else belongs in `--verbose`.
The diagnostic must read from the effective turn/tool plan, not from a second
copy of the config.

### P2: Default Project Agents

Ship project-scoped role files under `.codex/agents/` only after P0 is
reliable. Initial candidates: `reviewer`, `explorer` (reuse built-in unless
behavior differs), `test-triage`. Prefer built-ins; do not ship a large
taxonomy before the tool path works.

### P2: Smoke Tests

- V1 config resolves to `MultiAgentVersion::V1`.
- On an OpenRouter/ZAI/Ambient provider fixture, the spawn tool is present in
  the model-visible spec set after the P0 fix (this is the regression test).
- `agents.max_depth` blocks a second-level spawn (depth enforcement at
  `spec_plan.rs:362`).
- `/subagents` explains disabled, namespace-filtered, and healthy states.

## Basic Configuration

For the V1 rollout:

```toml
[features]
multi_agent = true

[agents]
max_threads = 6
max_depth = 1
```

Notes:

- `max_threads = 6` is `DEFAULT_AGENT_MAX_THREADS` (`config/mod.rs:204`), enforced in `reserve_spawn_slot`.
- `max_depth = 1` allows the root session to spawn direct children and blocks recursion, enforced at `spec_plan.rs:362` via `exceeds_thread_spawn_depth_limit`.
- Do not enable `multi_agent_v2` for the V1 rollout. If V2 is chosen as the cross-provider path, `agents.max_threads` is rejected (`config/mod.rs:1379`); V2 uses `features.multi_agent_v2.max_concurrent_threads_per_session` instead.

## Acceptance Criteria

- A fresh PFTerminal install, on a third-party provider, can run `spawn one explorer subagent to inspect this repo and summarize the docs structure`, and the spawn succeeds.
- `/subagents` shows whether the spawn tool is visible before the user tries to use it, and reports the `namespace_tools` capability.
- If unavailable, the error names the exact cause (`namespace_tools=false` and namespace-wrapped spec), not a generic "tool unavailable."
- Third-party provider sessions do not silently lose subagent support: the spawn tool survives serialization after the P0 fix.
- The OpenAI Responses path is unchanged and still works.
- The default behavior remains bounded: no recursive fan-out (`max_depth=1`), no unlimited thread creation (`max_threads=6`), no automatic spawning without explicit user instruction.
