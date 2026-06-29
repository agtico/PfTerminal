# PFTerminal Specification

**Version:** 1.0 — 2026-06-26
**Status:** Living document. Describes the current product, the active sprint, and the planned evolution toward full agentic orchestration.

---

## 1. Overview

PFTerminal is a **terminal-native AI engineering command center** built as a product fork of the open-source Codex CLI. It keeps the Codex local coding-agent runtime — tools, sandbox, approvals, MCP, sessions — and retargets the product onto third-party model providers (Ambient, Z.AI, OpenRouter, Baseten, Vercel) and OpenAI Codex account login, rather than defaulting only to the OpenAI Responses API.

PFTerminal is **not a web chat**. It lives in the user's repository, shell, and panes. It spawns, names, dispatches to, and monitors agent panes so that planning, implementation, review, and status all stay visible in the place engineers already work.

### Core thesis

A single human working one-on-one with a single coding agent hits two persistent failure modes:

1. The agent **misrepresents completion** — it reports work is done without proving it.
2. The agent does what you *said*, not what you *meant* — literal compliance without intent alignment.

PFTerminal's thesis is that a **hierarchy** of specialized agents — planning, execution, adversarial review, research, documentation — separated into distinct roles under a chain of command produces more reliable, verifiable engineering work than a single monolithic assistant. The human stops juggling dozens of terminal tabs and talks to **one orchestrator**, which deploys and supervises everything.

---

## 2. Design Principles

- **Hierarchy as a control structure.** Rank encodes authority, capability, and cost. Higher entities plan and supervise; lower entities execute and check.
- **Archetype ≠ model.** Roles are behavioral templates, not fixed model assignments. Each spawned instance is configured with a specific model and parameters, bound to a milestone.
- **Terminal-native.** Work stays in the repo, shell, and panes. PFTerminal fits the command line instead of forcing engineering work into a separate web UI.
- **Human-in-the-loop.** The human (Sauron) is the final authority for goals, tool approvals, risky operations, and acceptance of completed work.
- **Adversarial QA to minimize human review.** Reviewers (Trolls) are structurally adversarial to executors (Orcs), so most quality enforcement happens before a human ever looks.
- **Verifiability.** "Done" must be demonstrable, not asserted. Completion produces an artifact: a proof, a passing check, a doc entry.
- **Right model for the right job, costed.** Planning, execution, review, and research have different cost/latency/quality curves. The orchestration layer assigns models accordingly and tracks token spend.
- **Document-driven flow.** The plan is the engine. Work is scored against the document, and the document's gaps drive execution.

---

## 3. Architecture

### 3.1 Runtime lineage

PFTerminal inherits the Codex CLI Rust workspace under `codex-rs/` and preserves its major subsystems:

| Subsystem | Inherited from Codex |
| --- | --- |
| TUI and slash commands | `codex-rs/tui/` |
| Model/provider client runtime | `codex-rs/core/src/client.rs`, `codex-rs/codex-api/` |
| Tool registry and execution | `codex-rs/core/src/tools/` |
| Sandbox and approval flows | `codex-rs/core/`, `codex-rs/exec/` |
| MCP support | `codex-rs/codex-mcp/` |
| Session, rollout, and thread storage | `codex-rs/core/`, `codex-rs/login/` |
| Exec and review modes | inherited |

The product changes are code-level: provider definitions, model metadata, request-shaping, onboarding, credential storage, packaging, branding, and orchestration — not just prompt text.

### 3.2 Product command names

PFTerminal keeps upstream-compatible `codex` paths while adding product-facing command names:

- `codex-rs/cli/Cargo.toml` defines both `codex` and `pfterminal` binaries.
- The npm package `@agticorp/pfterminal` exposes both `pfterminal` and `codex` command aliases.
- The standalone installer creates a `pfterminal` launcher and leaves any existing stock `codex` command alone.
- State defaults to `$HOME/.pfterminal`, separate from a stock Codex install using `$HOME/.codex`.

### 3.3 Repository layout

```
codex-rs/          Rust workspace (inherited from Codex CLI)
  core/            Tool loop, client, config, agent graph
  tui/             Terminal UI, slash commands, panes, model picker
  codex-api/       Chat Completions adapter, request shaping
  model-provider-info/  Built-in provider definitions
  models-manager/  Bundled model metadata (models.json)
  login/           Auth, provider keys, vault integration
  vault/           Encrypted credential store
  secrets/         Managed-secrets substrate
codex-cli/         npm CLI package
scripts/install/   Standalone installer scripts
sdk/               SDK surfaces
docs/              MkDocs user-facing documentation
```

---

## 4. Model Providers

PFTerminal ships built-in providers. Users do not need to define providers manually in `config.toml`; they only need a credential for the provider they plan to use.

### 4.1 Built-in providers

| Provider id  | Display name | Base URL                                | Env key              | Wire API         | Default model            |
| ------------ | ------------ | --------------------------------------- | -------------------- | ---------------- | ------------------------ |
| `openai`     | OpenAI       | `https://chatgpt.com/backend-api/codex` | Codex account login  | Responses        | `gpt-5.5`                |
| `ambient`    | Ambient      | `https://api.ambient.xyz/v1`            | `AMBIENT_API_KEY`    | Chat Completions | `zai-org/GLM-5.2-FP8`    |
| `zai`        | Z.AI         | `https://api.z.ai/api/coding/paas/v4`   | `ZAI_API_KEY`        | Chat Completions | `glm-5.2`                |
| `openrouter` | OpenRouter   | `https://openrouter.ai/api/v1`          | `OPENROUTER_API_KEY` | Chat Completions | `z-ai/glm-5.2`           |
| `baseten`    | Baseten      | `https://inference.baseten.co/v1`       | `BASETEN_API_KEY`    | Chat Completions | `zai-org/GLM-5.2`        |
| `vercel`     | Vercel       | `https://ai-gateway.vercel.sh/v1`       | `AI_GATEWAY_API_KEY` | Responses        | `zai/glm-5.2`            |

### 4.2 Model catalog

The `/model` picker groups models into two sections:

**Coding Plans** (OpenAI Codex, Ambient, Z.AI):

| Model | Provider | Notes |
| --- | --- | --- |
| `gpt-5.2` | OpenAI | Codex account model |
| `zai-org/GLM-5.2-FP8` | Ambient | Ambient default GLM 5.2 coding model |
| `glm-5.2` | Z.AI | Z.AI coding-plan GLM 5.2 |

**Pay Per API Call** (OpenRouter, Baseten, Vercel):

| Model | Provider | Pricing |
| --- | --- | --- |
| `z-ai/glm-5.2` | OpenRouter | $0.98/M input, $3.08/M output |
| `minimax/minimax-m3` | OpenRouter | $0.30/M input, $1.20/M output |
| `openrouter/owl-alpha` | OpenRouter | $0/M |
| `google/gemini-3.5-flash` | OpenRouter | $1.50/M input, $9.00/M output |
| `zai-org/GLM-5.2` | Baseten | $1.50/M input, $4.50/M output |
| `zai/glm-5.2` | Vercel | $1.40/M input, $4.40/M output |
| `zai/glm-5.2-fast` | Vercel | $3.00/M input, $10.25/M output |

### 4.3 Model selection

Switch models at runtime with `/model` or launch with a specific model:

```bash
pfterminal -m glm-5.2
pfterminal -m z-ai/glm-5.2
pfterminal -m gpt-5.5
```

Set a default provider and model in `$CODEX_HOME/config.toml`:

```toml
model_provider = "ambient"
model = "zai-org/GLM-5.2-FP8"
```

### 4.4 GLM request shaping

Ambient and Z.AI requests map PFTerminal reasoning levels to provider-specific fields (`reasoning_effort`, `enable_thinking`, `emit_usage`). Responses-style turn items are flattened into string input for the Chat Completions wire shape, and hidden reasoning is not replayed. This keeps GLM-class models working through the Codex tool-call loop without forcing them through OpenAI-specific semantics.

---

## 5. Authentication & Vault

PFTerminal has three credential surfaces:

1. **OpenAI Codex account login** for the `openai` provider (device auth).
2. **Provider API keys** for Ambient, Z.AI, OpenRouter, Baseten, and Vercel.
3. **The encrypted `/vault` credential store** for provider keys and other user-managed secrets.

### 5.1 Vault storage

The vault is backed by the Codex managed-secrets substrate:

- **Encrypted data:** `$CODEX_HOME/secrets/local.age`
- **Passphrase storage:** OS keyring when available (Secret Service / GNOME Keyring on Linux, Keychain on macOS).
- **Linux fallback:** local `0600` keyring fallback file for the vault passphrase when no Secret Service is available.
- **Legacy fallback:** `provider_auth.json` is read for migration compatibility and removed after a successful vault write.

Provider keys use stable vault labels:

| Provider key         | Vault label                   |
| -------------------- | ----------------------------- |
| `AMBIENT_API_KEY`    | `provider/ambient_api_key`    |
| `ZAI_API_KEY`        | `provider/zai_api_key`        |
| `OPENROUTER_API_KEY` | `provider/openrouter_api_key` |
| `BASETEN_API_KEY`    | `provider/baseten_api_key`    |
| `AI_GATEWAY_API_KEY` | `provider/ai_gateway_api_key` |

The vault is global to the PFTerminal home directory, so stored credentials are available from any working directory that uses the same `CODEX_HOME`.

### 5.2 Vault commands

```text
/vault                                    # Open the vault action menu
/vault list                               # List credential labels and metadata
/vault show provider/zai_api_key          # Inspect one credential's metadata
/vault credential add                     # Add a credential through masked secure-entry
/vault credential delete provider/openrouter_api_key
```

`/vault credential add` opens a masked entry flow. Raw secrets must not be typed into chat — the secure entry path keeps secrets out of prompt history, transcript history, and model context. `/vault show <label>` displays metadata only; raw reveal/export is handled through secure UI, not chat output.

### 5.3 Key resolution order

1. Encrypted vault (first).
2. Legacy `provider_auth.json` (migration compatibility).
3. Environment variables (temporary shells, CI, automation).

### 5.4 Agent vault access (design)

Agents, subagents, and Claude panes need to use provider API keys **without** learning, printing, inheriting, or storing raw secrets. The security model is:

- Users store credentials in `/vault` or `/providers`.
- Agents request provider capabilities by name.
- PFTerminal resolves vault credentials **outside** the model context.
- Tools and provider transports receive only the minimum credential material needed.
- Audit logs record credential access without recording the secret.

The planned architecture includes a **provider capability registry**, a **vault lease broker** that mints scoped short-lived leases (the model sees only a lease identifier, never the provider key), and **no raw key inheritance** to shell-capable agent processes.

---

## 6. Multi-Agent Hierarchy

PFTerminal runs a multi-agent hierarchy with a clear chain of command. Intent flows down through the hierarchy. Evidence, summaries, reviews, and approval requests flow back up.

### 6.1 Roles

| Role   | Level                   | Responsibility                                                                 |
| ------ | ----------------------- | ----------------------------------------------------------------------------- |
| Sauron | Human — Final Authority | Sets the mission, approves sensitive actions, resolves tradeoffs, ships.       |
| Nazgul | CTO — Orchestrator      | Translates mission into a plan, chooses workstream owners, dispatches to Trolls, integrates reports. |
| Troll  | VP-Eng — Supervisor     | Owns a domain, supervises Orc executors, reviews progress, escalates conflicts. |
| Orc    | IC — Executor           | Takes scoped tasks, edits code, runs checks, sends evidence-rich reports up.   |

A PFTerminal terminal and a Nazgul are a unified entity: **at most one Nazgul per PFTerminal terminal**. Direct human↔Nazgul communication happens in the TUI. Trolls report up to the Nazgul. Orcs report up to Trolls. Agents must not exist as isolated tabs with no visible chain of command.

### 6.2 `/spawn` orchestration

The `/spawn` command creates managed work entities with explicit role, model, harness, and parent/child relationships.

**Status:** P0 runtime and TUI implementation landed; live Troll → Orc acceptance passed.

```text
/spawn

Spawn
  Bind a Nazgul pane or create managed work with role, model, harness, and parent.

  Role
  > Nazgul   Select an existing user pane as the hierarchy root.
    Troll    Review/foreman. Reports to the Nazgul.
    Orc      Executor. Reports to a Troll.

  Harness
  > PFTerminal Agent
    Claude Code Headless

  Model
  > Inherit current model
    zai-org/GLM-5.2-FP8 via Ambient
    glm-5.2 via Z.AI
    ...

  Task
  > Review the auth diff and assign one Orc to fix the highest-risk issue.
```

### 6.3 Role graph enforcement

The v0 role graph is fixed and enforced by host-side validation, not only by prompts:

```
Nazgul binding (existing user pane)
  → Troll
       → Orc
```

- Nazgul spawns Troll. ✓
- Troll spawns Orc. ✓
- Nazgul spawns Orc without a supervising Troll. ✗
- Troll spawns Troll. ✗
- Orc spawns anything. ✗
- Any spawn exceeding depth 2. ✗

The multi-agent depth configuration allows depth 2 (depth 0: Nazgul/root, depth 1: Troll, depth 2: Orc) and rejects depth 3. Thread cap is `DEFAULT_AGENT_MAX_THREADS = 6`.

### 6.4 Harnesses

| Harness | Status | Description |
| --- | --- | --- |
| PFTerminal Agent | P0 (enabled) | Uses the existing Codex/PFTerminal multi-agent runtime with parent/child graph, role metadata, status subscriptions, and `wait_agent`. |
| Claude Code Headless | P1 (experimental) | Claude Code panes exist under `/panes` but need to emit the same `SpawnNode` status and completion events as native agents before treated as complete. |

### 6.5 Completion semantics

"Done" means the status is final and visible to the parent.

- **Orc:** final state is `Completed`, `Errored`, `Interrupted`, `Shutdown`, or `NotFound`; completion includes result evidence; supervising Troll sees the status.
- **Troll:** all required Orcs are final; Troll has reviewed their output; Troll reports upward to Nazgul; Nazgul view shows the Troll final state and summary.

### 6.6 Pane hierarchy

After creation, `/panes` shows a hierarchy, not a flat list:

```text
User Panes
> Nazgul - Main - GLM 5.2 Ambient

Spawned Work
  Trolls
  > auth-review [troll] running - reviewing auth diff
      Orcs
      - auth-fix-1 [orc] running - patching provider validation
      - test-sweep [orc] done - tests passed
```

---

## 7. Slash Commands

PFTerminal inherits Codex slash commands and adds product-specific vault, provider, and spawn workflows.

| Command                 | Purpose                                                       |
| ----------------------- | ------------------------------------------------------------- |
| `/model`                | Select model/provider and effort mode                         |
| `/providers`            | Provider onboarding picker (Codex account login or API keys)  |
| `/vault`                | Open the encrypted credential vault action menu               |
| `/vault list`           | List credential labels and metadata without revealing secrets |
| `/vault show <label>`   | Inspect one credential's metadata                             |
| `/vault credential add` | Add a credential through the masked secure-entry flow         |
| `/spawn`                | Open the spawn wizard (role, harness, model, task)            |
| `/spawn status`         | Show the hierarchy and current statuses                       |
| `/spawn nazgul`         | Bind an existing user pane as the Nazgul root                 |
| `/spawn troll`          | Create a Troll                                                |
| `/spawn orc`            | Create an Orc                                                 |
| `/panes`                | Switch between user panes and agent panes                     |
| `/agent`                | Low-level thread/agent switcher                               |
| `/skills`               | Browse bundled, repo, user, and plugin skills                 |
| `/status`               | Inspect current model/provider/session state                  |
| `/compact`              | Compact a long conversation                                   |

For the full set of inherited Codex slash commands, see the [Codex CLI slash-commands documentation](https://developers.openai.com/codex/cli/slash-commands).

---

## 8. Skills

PFTerminal inherits Codex skills and ships bundled system skills.

### Skill loading paths

| Scope | Path |
| --- | --- |
| Bundled system skills | `$CODEX_HOME/skills/.system/` (i.e. `$HOME/.pfterminal/skills/.system/`) |
| User global skills | `$HOME/.agents/skills/` |
| Repo-scoped skills | `<repo>/.agents/skills/` |

### Current bundled skills

| Skill             | Purpose                                             |
| ----------------- | --------------------------------------------------- |
| `frontend-design` | Browser frontend design and implementation guidance |
| `imagegen`        | Generate or edit raster images                      |
| `openai-docs`     | Reference OpenAI/Codex docs                         |
| `plugin-creator`  | Scaffold Codex plugins                              |
| `skill-creator`   | Create or update a skill                            |
| `skill-installer` | Install curated or GitHub-hosted skills             |

---

## 9. Configuration

### 9.1 Config location

PFTerminal reads config from `$CODEX_HOME/config.toml`.

```bash
export CODEX_HOME="${PFTERMINAL_HOME:-$HOME/.pfterminal}"
```

### 9.2 Common configs

```toml
# Ambient default
model_provider = "ambient"
model = "zai-org/GLM-5.2-FP8"

# Z.AI coding plan
model_provider = "zai"
model = "glm-5.2"

# OpenRouter metered
model_provider = "openrouter"
model = "z-ai/glm-5.2"

# Baseten GLM
model_provider = "baseten"
model = "zai-org/GLM-5.2"

# Vercel GLM
model_provider = "vercel"
model = "zai/glm-5.2"
```

### 9.3 Multi-agent config

```toml
[features]
multi_agent = true

[agents]
max_threads = 6
max_depth = 1
```

- `max_threads = 6` is `DEFAULT_AGENT_MAX_THREADS`, enforced in `reserve_spawn_slot`.
- `max_depth` allows the root session to spawn direct children and blocks recursion.
- Do not enable `multi_agent_v2` for the V1 rollout; V2 uses `features.multi_agent_v2.max_concurrent_threads_per_session` instead.

### 9.4 Custom providers

Advanced users can define custom providers under `[model_providers]`:

```toml
[model_providers.custom-chat]
name = "Custom Chat Provider"
base_url = "https://example.com/v1"
env_key = "CUSTOM_PROVIDER_API_KEY"
wire_api = "chat"
```

### 9.5 Lifecycle hooks

Admins can set `allow_managed_hooks_only = true` in `requirements.toml` to ignore user, project, and session hook configs while still allowing managed hooks. This setting is only supported in `requirements.toml`.

---

## 10. Provider Hardening

### 10.1 Hammer reduction

PFTerminal implements shared provider request state in SQLite, keyed by provider/model/key fingerprint, to prevent wasteful repeated large requests:

- **Provider cooldown circuit breaker** after `429`, including `Retry-After` and reset-header parsing. Local exponential cooldown: 30s → 60s → 120s → capped at 5m.
- **Cross-process request leases** so concurrent PFTerminal workers share active-request state and do not send large requests through the same credential simultaneously.
- **Request-byte/input-token preflight telemetry** and third-party hammer-risk warnings before dispatch.
- **Hard identical-tool-call loop guard** to detect and break repeated identical tool calls.
- **Cache-aware provider telemetry** so large third-party requests do not produce user-facing warnings when the previous live request reported a healthy provider cache hit.

If cooldown or lease is active, PFTerminal does not send; it shows wait time and offers wait, compact, switch provider/model, or start a fresh thread.

### 10.2 GLM 5.2 tool compatibility

GLM 5.2 works through the OpenAI-compatible transport and Codex tool-call loop. The observed failure was narrow: GLM repeatedly missed Codex's strict `apply_patch` grammar. PFTerminal follows the harness pattern of exposing model-family-appropriate edit primitives:

- **`structured_edit` and `structured_write`** for GLM/Z.AI/Ambient-style model profiles.
- **Strict `apply_patch`** preserved unchanged for Codex-native models that handle the grammar correctly.
- Model-authored Python heredoc source rewrites are rejected for structured-edit profiles, so shell remains available for tests/builds but is no longer the normal edit fallback.

### 10.3 Tool-call runaway remedy

When a model emits malformed or truncated tool-call arguments (e.g. `EOF while parsing a string`), PFTerminal:

- Treats the malformed call as **non-retriable** rather than recording it as a normal tool result and asking the model for a follow-up.
- Stops persisting raw malformed oversized tool-call arguments into conversation history.
- Provides bounded/chunked write mechanics for large generated files (in progress).

This prevents the failure mode where a model repeatedly attempts the same large write, generating another provider request and another malformed tool call in an infinite loop.

### 10.4 Subagent provider compatibility

Subagent tool visibility is gated by the `namespace_tools` provider capability. Third-party providers (Ambient, Z.AI, OpenRouter, Baseten, Vercel) advertise `namespace_tools: false`, which causes the V1 `multi_agent_v1` namespace-wrapped spec to be filtered out. PFTerminal ships V1 spawn tools that serialize as plain functions (not `ToolSpec::Namespace`) so they survive providers whose `namespace_tools` capability is false. V2's `spawn_agent` is already a plain `ToolSpec::Function` and survives the filter natively.

---

## 11. Installation

### 11.1 System requirements

| Requirement  | Details                                                     |
| ------------ | ---------------------------------------------------------- |
| OS           | macOS 12+, Ubuntu 20.04+/Debian 10+, or Windows 11 via WSL2 |
| Git          | 2.23+ recommended                                           |
| RAM          | 4 GB minimum, 8 GB recommended                              |
| Rust         | Required only for source builds                             |
| Node.js      | Required only for npm/package development                    |
| Linux sandbox | `bubblewrap` recommended                                   |
| Linux keyring | A Secret Service provider such as GNOME Keyring recommended |

### 11.2 Release installer (preferred)

```bash
curl -fsSL https://github.com/agtico/PfTerminal/releases/latest/download/install.sh | sh
```

Creates a `pfterminal` launcher, defaults state to `$HOME/.pfterminal`, and leaves any existing stock `codex` command untouched.

### 11.3 Build from source

```bash
git clone https://github.com/agtico/PfTerminal.git
cd PfTerminal/codex-rs
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo build -p codex-cli --bin pfterminal
```

Launch from the workspace you want PFTerminal to inspect:

```bash
cd ~/repos/my-project
pfterminal
```

### 11.4 npm package

```bash
npm install -g @agticorp/pfterminal
pfterminal --version
```

---

## 12. Planned Evolution (Full Orchestration)

The current `/spawn` slice is intentionally smaller than the full orchestration spec. The long-term ontology adds the following entities and capabilities:

### 12.1 Full bestiary

| Entity | Role | Default model |
| --- | --- | --- |
| **Sauron** | Human user; the will/intent | (human) |
| **The Eye** | Interface that conveys intent to the Nazgul | Task node / automation |
| **Nazgul** | Orchestrator the user talks to; lives in one PFTerminal | Planning-tier |
| **Balrog** | Planner; configures & spawns creatures; runs the harness; owns the Grimoire | Planning-tier |
| **Troll** | Adversarial QA / foreman over Orcs | Strong reviewer |
| **Orc** | Executor; does the work | Per-instance |
| **Goblin** | Fast sanity checks (quick review, bug sweep) | Fast model |
| **Wyvern** | Researcher / web + academic search | Claude Deep Research |
| **Golem** | Always-on background daemon | Free OpenRouter model |
| **Sorcerer/Scribe** | Records campaign results to MkDocs | Configurable |
| **Carrion-eater** | Campaign-completion QA / cleanup | Configurable |

### 12.2 Creature configuration

Every spawned creature is an instance defined by:

- **archetype** — behavioral role (orc, troll, goblin, wyvern, golem, etc.)
- **specialization** — optional free-text purpose tag (designer, siege, elite, security, …)
- **model** — provider + model id
- **params** — reasoning/thinking effort, temperature, tools, context
- **bound_to** — the milestone/gate this instance serves
- **constraints** — e.g. Golems operate on open-source repos only

```yaml
id: orc-design-01
archetype: orc
specialization: designer
model: anthropic/claude-opus-4.8
params:
  reasoning: high
  tools: [repo, editor]
bound_to: gate/ui-redesign
spawned_by: balrog
```

### 12.3 The Grimoire (model-performance memory)

The system's institutional memory about model performance — the feedback loop that makes casting smarter over time. Owned by the Balrog: read at plan time to inform creature configuration, written at the end of a battle with observations about how each model performed in each role. Records per (archetype × model × task-type) signals such as quality score, gate pass/fail, rework rate, latency, and token cost.

### 12.4 The Text Improvement Harness

The mechanism by which the Balrog raises the quality of any document — most importantly the battle plan itself:

1. **Score** the input document with three separate models, five times each.
2. Aggregate into a **score packet** listing concrete strengths and criticisms.
3. **Rewrite** to overcome the criticisms.
4. **Re-score** the rewrite.
5. **Repeat** until the score plateaus.
6. **Execution sprint** — when the score taps out but gaps remain, the harness emits an instruction to build the missing element, then reintegrates the result and re-scores. A quality gap becomes a concrete buildable task.

### 12.5 Wallet (roadmap)

PFTerminal `/wallet` is planned as the native credential and small-spend layer for agents:

- API keys for model providers, GPU clouds, RPC vendors, exchanges — each with labels, purpose, scope, spend caps, and rotation metadata.
- Small Base USDC hot-wallet balances for buying search, inference, and other capacity.
- Core invariant: agents can use approved accounts and spend within limits, but cannot read, export, print, or copy raw secrets.

### 12.6 Remote access

The Nazgul is planned to be connected to a wallet linked to Nostr, so Sauron can message it from a phone, Slack, or elsewhere. The user can request a report from the Nazgul remotely and receive completion wake-ups.

### 12.7 MkDocs-native documentation

MkDocs lives inside the repo — not a separate product. Rendered as a clean, viewable site with a standardized base structure. Kept continuously correct through Golem and Sorcerer responsibility.

---

## 13. Branding

The TUI and login surfaces use PFTerminal/Post Fiat branding:

- Device-code prompt: `Welcome to PFTerminal` and `Post Fiat's command-line coding agent`.
- Session cards render `PFTerminal`.
- Composer placeholder: `Ask PFTerminal to do anything`.
- Status surfaces include `Post Fiat Terminal`.
- GLM reasoning is presented as `Standard` and `Deep` instead of raw OpenAI-style effort labels.

---

## 14. Development

### 14.1 Build and test

```bash
cd codex-rs
cargo build -p codex-cli --bin pfterminal
just fmt          # format after code changes
just test -p codex-tui   # test specific crate
```

Avoid `--all-features` for routine local runs; it expands the build matrix and increases `target/` disk usage.

### 14.2 Tracing

The TUI records diagnostics in bounded local stores by default. Set `log_dir` explicitly for a plaintext TUI log:

```bash
pfterminal -c log_dir=./.pfterminal-log
tail -F ./.pfterminal-log/codex-tui.log
```

### 14.3 Docs

User-facing docs live in `docs/` and are built by MkDocs:

```bash
mkdocs serve
mkdocs build
```

---

## 15. License

Apache-2.0.

---

## 16. Upstream

PFTerminal is based on the open-source Codex CLI project. Upstream changes are isolated through the `upstream` remote, and PFTerminal changes land through the `agtico/PfTerminal` repository. Product-specific provider constants stay in provider metadata modules, product model choices stay in bundled model metadata, UI branding stays in TUI-facing modules, and request-compatibility shims stay close to API serialization — so upstream Codex changes can be merged without hiding PFTerminal product behavior in scattered prompt text.
