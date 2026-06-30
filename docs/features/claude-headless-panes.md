# Claude Headless Panes

Status: Ambient parity workflow suite passed on June 25, 2026 for wrapped
Claude Code panes after the hidden local tool-call ceiling was removed. A later
streaming protocol patch also passed the full four-workflow Ambient rerun. Z.AI,
Baseten, OpenRouter, Vercel, and Claude Plan remain experimental until they pass the
same workflow suite.

The Ambient path runs real `claude -p` headless turns against a local
PFTerminal Anthropic Messages bridge. The bridge translates Claude Code
Messages requests to Ambient Chat Completions with the vault-held Ambient key,
then returns Claude-compatible JSON/SSE back to the Claude process. Direct
Z.AI, Baseten, OpenRouter, Vercel, and Claude Plan profiles are also live-smoke-tested
through the same pane backend.

The local runner uses a hardened headless invocation and surfaces structured
provider errors from Claude Code stream JSON. The prior Ambient parity claim is
superseded because the bridge included a hidden 3-tool-call ceiling; the current
implementation removes that local work cap and records fresh parity evidence
below.

## Complete

- [x] Verified that current Codex/PFTerminal `/agent` panes are logical agent
  threads, not OS tmux panes.
- [x] Verified local Claude Code supports `--settings`,
  `--dangerously-skip-permissions`, `--permission-mode bypassPermissions`,
  `--session-id`, `--resume`, `--output-format json`, and
  `--output-format stream-json`.
- [x] Live-tested two-turn Claude Code headless continuity:
  first turn created a fixed session id, second turn resumed that id and
  correctly recalled the previous marker.
- [x] Confirmed the second resumed turn reported cache reuse
  (`cache_read_input_tokens`), which is the behavior needed for a pane-like
  session rather than a stateless one-shot call.
- [x] Implemented a user-pane registry for user-owned panes separate from
  Codex subagent thread navigation.
- [x] Added `/panes` with `User Panes`, `New Claude Pane`, and `Agent Panes`
  sections.
- [x] Added a `ClaudeHeadlessPane` backend that invokes Claude Code through
  headless `stream-json` mode.
- [x] Added a PFTerminal vault auth helper for Claude pane provider
  credentials.
- [x] Generate pane-local Claude settings from vault-backed provider profiles.
- [x] Added per-pane request locking so a Claude session cannot receive two
  turns concurrently.
- [x] Added tests for pane creation, provider settings generation, resume
  behavior, stream-json error handling, and redaction boundaries.
- [x] Hardened the headless invocation with `--bare`,
  `--output-format stream-json --verbose`, disabled nonessential traffic and
  experimental betas, and disabled non-streaming fallback to avoid rc-124
  retry hangs.
- [x] Live-tested the Ambient-backed Claude pane path with a vault-held key,
  and separately verified the raw Z.AI Anthropic endpoint/key.
- [x] Implemented a local Ambient Anthropic-to-Chat bridge for Claude Code
  headless panes.
- [x] Live-tested the Ambient bridge through the pane command path:
  first turn returned `OK-PFTERMINAL-LIVE`; resumed second turn returned the
  same marker.
- [x] Live-tested a Claude Code tool loop through the Ambient bridge:
  Claude inspected the working directory through Claude Code tooling and
  returned `FOUND-CARGO-TOML`.
- [x] Exercised the real `/panes` TUI path in tmux:
  created an Ambient Claude pane, saw the footer label switch to the Claude
  pane, submitted a prompt, and rendered `UX-PANE-BUILD-FOUND`.
- [x] Added `pfterminal claude-pane-smoke` and live-tested Ambient, Z.AI,
  Baseten, OpenRouter, and Claude Plan profiles through the pane backend.
- [x] Added per-turn audit JSON with provider, model, pane id, session id,
  artifact path, duration, usage, terminal reason, and tool-use metadata.
- [x] Added max-turn pause handling: `error_max_turns` is resumable and no
  longer treated as a fake success or opaque unstructured error.
- [x] Removed hidden local Claude-pane work ceilings: no local tool-call budget,
  no forced `--max-turns`, and no wall-clock turn timeout that a real Claude
  Code TUI session would not impose.
- [x] Updated the Ambient bridge to forward multiple upstream tool calls,
  preserve requested output token limits, emit Anthropic-compatible stream
  heartbeats while waiting on Ambient Chat Completions, and retry rate-limited
  upstream calls with `Retry-After` support.
- [x] Fixed Ambient streaming protocol handling after live review: upstream
  failures emit Anthropic `event: error`, `message_start` is deferred until
  upstream usage is known, input/cache usage lives on `message_start`, and
  non-streaming responses preserve cache-read usage.
- [x] Live-tested the uncapped Ambient workflow suite:
  code review with full diff inspection, resumed code review, mock website,
  NumPy vs Pandas benchmark, and turn-by-turn auditability.

## Goal

PFTerminal should let users switch between the main Codex/PFTerminal session,
Claude Code sessions, and Codex subagent threads without embedding a full
terminal multiplexer.

The target UX is:

```text
/panes

User Panes
> Codex - Main - OpenAI
  Claude Code - GLM 5.2 Ambient
  Claude Code - GLM 5.2 Z.AI
  Claude Code - GLM 5.2 Baseten
  Claude Code - GLM 5.2 Vercel
  Claude Code - GLM 5.2 Fast Vercel
  Claude Code - OpenRouter
  Claude Code - Claude Plan
  + New Pane

Agent Panes
  Main [default]
  Linnaeus [explorer]
  Orc [worker]
```

Selecting a user pane changes the active interactive surface. Selecting an
agent pane reuses the existing Codex `/agent` thread switching behavior.

## Decision

Use **Claude Code headless mode**, not embedded tmux, as the first
implementation path.

Embedding Claude's interactive TUI would require PFTerminal to become a
terminal multiplexer: ANSI rendering, resize handling, scrollback, alternate
screen behavior, keyboard forwarding, mouse forwarding, lifecycle management,
and prompt/status detection. That is the wrong first abstraction.

Headless Claude Code gives PFTerminal structured JSON events that can be
rendered into native history cells and later exposed as a Codex tool. The
tradeoff is that PFTerminal owns the UI instead of reusing Claude's terminal
UI.

## Verified Headless Session Behavior

The local test used a fixed Claude session id:

```bash
claude -p \
  --output-format stream-json \
  --verbose \
  --permission-mode bypassPermissions \
  --session-id 11111111-2222-4333-8444-555555555555 \
  'This is a continuity test. Remember the marker: PFT-PANE-721. Reply with exactly: stored.'
```

The first call succeeded and returned the same session id in JSON output.

A second call using `--session-id` failed with:

```text
Error: Session ID 11111111-2222-4333-8444-555555555555 is already in use.
```

The correct continuation path is:

```bash
claude -p \
  --output-format stream-json \
  --verbose \
  --permission-mode bypassPermissions \
  --resume 11111111-2222-4333-8444-555555555555 \
  'What marker did I ask you to remember in the previous turn? Reply with only the marker.'
```

That call returned:

```text
PFT-PANE-721
```

and reported cached prior context. Therefore the pane backend should create a
session once and resume it on later turns.

## Architecture

Add a user-pane layer above the existing Codex agent-thread picker.

```rust
enum UserPaneBackend {
    CodexThread(ThreadId),
    ClaudeHeadless(ClaudePaneSession),
}

struct ClaudePaneSession {
    pane_id: Uuid,
    claude_session_id: Option<Uuid>,
    provider_profile: ClaudeProviderProfile,
    cwd: PathBuf,
    settings_path: PathBuf,
    status: PaneStatus,
}
```

The current `/agent` implementation remains responsible for Codex subagent
threads. `/panes` should compose two data sources:

- user panes from the new pane registry;
- agent panes from existing `AgentNavigationState`.

This keeps "user-owned work surfaces" and "Codex-spawned worker threads"
separate instead of overloading `/agent`.

## Runtime Flow

First message to a Claude pane:

```text
PFTerminal
  -> resolve provider profile
  -> read credential from vault
  -> start local provider bridge if the profile requires one
  -> write pane-local Claude settings
  -> run claude --bare -p --output-format stream-json --verbose --session-id <pane_uuid>
  -> parse JSON events
  -> render native PFTerminal history cells
```

Later messages to the same pane:

```text
PFTerminal
  -> acquire pane lock
  -> refresh settings/auth helper
  -> run claude --bare -p --output-format stream-json --verbose --resume <claude_session_id>
  -> parse JSON events
  -> release pane lock
```

The pane lock is required because Claude rejects a session when another process
is already using the same session id.

## Vault Boundary

Claude Code should not receive direct vault access by default.

PFTerminal owns the vault and exposes credentials to Claude only as a runtime
provider credential for the selected pane. The credential must not be written
to chat history, persisted in global `~/.claude/settings.json`, or included in
the Codex transcript.

Preferred auth path:

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "https://api.z.ai/api/anthropic",
    "ANTHROPIC_API_KEY": "",
    "ANTHROPIC_MODEL": "opus",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.2[1m]",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.2[1m]",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-4.7",
    "ANTHROPIC_SMALL_FAST_MODEL": "glm-4.7",
    "CLAUDE_CODE_SUBAGENT_MODEL": "glm-5.2[1m]",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "1000000",
    "API_TIMEOUT_MS": "3000000",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS": "1",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    "CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK": "1"
  },
  "apiKeyHelper": "pfterminal vault auth-helper provider/zai_api_key"
}
```

If `apiKeyHelper` is not reliable across supported Claude Code builds, use a
pane-local settings file plus a child-process environment assembled from the
vault at launch time. That is less ideal, but still avoids mutating global
Claude config.

Do not expose a raw `vault_get_secret` MCP tool to Claude in v0. MCP tool
results enter the model/tool transcript. A later Claude plugin can expose safe
metadata actions such as list labels, describe credentials, or request a
provider profile, but raw secret reads should remain host-owned.

## Provider Profiles

| Pane profile | Claude base URL | Credential source | Notes |
| --- | --- | --- | --- |
| Claude Plan | native Claude auth | Claude's own login/keychain | No PFTerminal provider key required. |
| Ambient GLM 5.2 | local PFTerminal bridge to `https://api.ambient.xyz/v1/chat/completions` | `provider/ambient_api_key` | Primary parity target. Claude Code talks Anthropic Messages to loopback; PFTerminal translates to Ambient Chat Completions without a local tool-call ceiling. |
| Z.AI GLM 5.2 | `https://api.z.ai/api/anthropic` | `provider/zai_api_key` | Passed live pane smoke with tool use and resume. |
| Baseten GLM 5.2 | `https://inference.baseten.co` | `provider/baseten_api_key` | Passed live pane smoke with tool use and resume. |
| OpenRouter | `https://openrouter.ai/api` | `provider/openrouter_api_key` | Passed live pane smoke with tool use and resume. |
| Vercel GLM 5.2 | `https://ai-gateway.vercel.sh` | `provider/ai_gateway_api_key` | Anthropic-compatible Vercel AI Gateway route; pending live pane smoke. |
| Vercel GLM 5.2 Fast | `https://ai-gateway.vercel.sh` | `provider/ai_gateway_api_key` | Fast Vercel AI Gateway route; pending live pane smoke. |

## Current Live Provider Evidence

The local implementation was tested with vault-held keys and the freshly built
`pfterminal` binary. The Ambient workflow rows below were rerun after removing
the hidden local tool-call ceiling.

| Probe | Result | Interpretation |
| --- | --- | --- |
| Ambient uncapped code-review workflow | passed; report `/home/postfiat/.pfterminal/panes/workflow-reports/claude-pane-workflow-suite-1782408460.json` | Fresh pane inspected actual active diff hunks and returned `PFT_CODE_REVIEW_DONE` with `DIFF_INSPECTED: yes`. Audit `/home/postfiat/.pfterminal/panes/claude-2bc75d2d-c5da-49c3-aa83-6ed027dacaff/turn-0001.audit.json` recorded `tool_use_count=31`, `max_turns=null`, and `timeout_ms=null`. |
| Ambient uncapped resumed code-review workflow | passed | Resume artifact `/home/postfiat/.pfterminal/panes/claude-2bc75d2d-c5da-49c3-aa83-6ed027dacaff/turn-0002.jsonl`; audit `/home/postfiat/.pfterminal/panes/claude-2bc75d2d-c5da-49c3-aa83-6ed027dacaff/turn-0002.audit.json` recorded `tool_use_count=6`, `max_turns=null`, and `timeout_ms=null`. |
| Ambient uncapped mock website workflow | passed; report `/home/postfiat/.pfterminal/panes/workflow-reports/claude-pane-workflow-suite-1782409303.json` | Artifact `/home/postfiat/.pfterminal/panes/claude-388bf5bf-a406-47dd-8e81-72f7c2e8b9cc/turn-0001.jsonl`; audit `/home/postfiat/.pfterminal/panes/claude-388bf5bf-a406-47dd-8e81-72f7c2e8b9cc/turn-0001.audit.json`. |
| Ambient uncapped NumPy vs Pandas workflow | passed | Artifact `/home/postfiat/.pfterminal/panes/claude-0d36cdb3-6139-4466-9c34-d61a2525ede7/turn-0001.jsonl`; audit `/home/postfiat/.pfterminal/panes/claude-0d36cdb3-6139-4466-9c34-d61a2525ede7/turn-0001.audit.json` recorded `tool_use_count=12`, `max_turns=null`, and `timeout_ms=null`. |
| Ambient uncapped auditability workflow | passed | Artifact `/home/postfiat/.pfterminal/panes/claude-394c2adf-c9a1-40dd-a458-5673974ce774/turn-0003.jsonl`; audit `/home/postfiat/.pfterminal/panes/claude-394c2adf-c9a1-40dd-a458-5673974ce774/turn-0003.audit.json`. |
| Ambient Chat Completions, `https://api.ambient.xyz/v1/chat/completions`, `zai-org/GLM-5.2-FP8` | HTTP 200 | Ambient key and base API are valid. |
| PFTerminal Ambient Claude bridge, first turn | returned `OK-PFTERMINAL-LIVE` | Real `claude -p` headless process received assistant text through the local bridge. |
| PFTerminal Ambient Claude bridge, resumed second turn | returned `OK-PFTERMINAL-LIVE` | The pane command path preserves Claude session continuity across `--resume`. |
| PFTerminal Ambient Claude bridge, tool loop | returned `FOUND-CARGO-TOML` after being asked to inspect the working directory | The local bridge carries Claude Code tool definitions, model `tool_use`, host tool execution, tool results, and final assistant text. |
| Real `/panes` TUI path in tmux | rendered `UX-PANE-BUILD-FOUND`; artifact `/home/postfiat/.pfterminal/panes/claude-ee436dce-06dc-4b7d-acce-e92cf42b3b06/turn-0001.jsonl` | Proves the visible picker, pane selection, footer label, prompt submission, Claude Code tool loop, native history rendering, and artifact persistence work together. |
| Live regression command | `PFTERMINAL_LIVE_CODEX_HOME=/home/postfiat/.pfterminal cargo test -p codex-tui claude_panes::tests::live_ambient_bridge_runs_claude_headless_for_two_turns -- --ignored --nocapture` | Explicit opt-in test uses the real vault credential and real Claude CLI; it is ignored by default for CI. |
| Live tool-loop command | `PFTERMINAL_LIVE_CODEX_HOME=/home/postfiat/.pfterminal cargo test -p codex-tui claude_panes::tests::live_ambient_bridge_runs_claude_tool_loop -- --ignored --nocapture` | Explicit opt-in test proves the pane is not chat-only. |
| Live code-review command | `PFTERMINAL_LIVE_CODEX_HOME=/home/postfiat/.pfterminal cargo test -p codex-tui claude_panes::tests::live_ambient_bridge_runs_substantive_code_review -- --ignored --nocapture` | Claude inspected the pane implementation and returned review findings through the wrapped pane path. |
| Live disposable edit command | `PFTERMINAL_LIVE_CODEX_HOME=/home/postfiat/.pfterminal cargo test -p codex-tui claude_panes::tests::live_ambient_bridge_runs_disposable_edit_task -- --ignored --nocapture` | Claude edited a fixture file through the pane path and the test asserted the file content. |
| Full provider smoke | `pfterminal claude-pane-smoke --providers ambient,zai,baseten,openrouter,claude-plan --cwd /home/postfiat/repos/PfTerminal` | 5 passed, 5 checked; report `/home/postfiat/.pfterminal/panes/smoke-reports/claude-pane-smoke-1782391716.json`. |
| Vercel provider smoke command | `pfterminal claude-pane-smoke --providers vercel,vercel-fast --cwd /home/postfiat/repos/PfTerminal` | Pending live run with `provider/ai_gateway_api_key`. |
| Ambient first smoke turn | success; tools `Bash`, `Read`; duration 87.1s | Ambient bridge supports substantive review-style work. |
| Z.AI first smoke turn | success; tools `Bash`, `Read`; duration 83.0s | Direct Z.AI Anthropic route works through the pane backend. |
| Baseten first smoke turn | success; tools `Read`, `Bash`; duration 15.5s | Direct Baseten profile works through the pane backend. |
| OpenRouter first smoke turn | success; tools `Read`, `Bash`; duration 74.5s | Direct OpenRouter profile works through the pane backend. |
| Claude Plan first smoke turn | success; tools `Read`, `Bash`; duration 75.2s | Native Claude Plan profile works through the pane backend. |

## Rendering Contract

Claude output should be mapped into PFTerminal-native cells, not dumped as raw
terminal text.

| Claude event/source | PFTerminal rendering |
| --- | --- |
| assistant text | assistant history cell |
| tool call start | tool-call history cell |
| tool result | bounded tool-result cell |
| error | error history cell |
| usage | pane footer/status metadata |
| session id | pane registry state |

If a Claude turn produces a large transcript, store the raw JSONL under a pane
artifact path and return/render a bounded summary. Do not inject full Claude
transcripts into the active Codex context unless the user explicitly asks.

## Codex-To-Claude Delegation

After `/panes` works interactively, expose Claude panes to Codex as a bounded
tool:

```json
{
  "pane_id": "claude-zai-1",
  "prompt": "Review this diff for correctness and missing tests.",
  "mode": "review"
}
```

Return a bounded result:

```json
{
  "pane_id": "claude-zai-1",
  "status": "completed",
  "summary": "Found one likely bug in auth fallback.",
  "changed_files": [],
  "artifacts": [
    {
      "type": "transcript",
      "path": ".pfterminal/panes/claude-zai-1/turn-004.jsonl"
    }
  ]
}
```

This lets Codex coordinate with Claude without pretending Claude Code is a
normal Codex model provider.

## Implementation Plan

### P0: Headless Probe Harness

Add a local probe command or test utility that verifies:

- create with `--session-id`;
- resume with `--resume`;
- output parses as JSON/JSONL;
- repeated same-session concurrent use fails and is handled by a lock;
- provider settings can be supplied without editing `~/.claude/settings.json`.

### P1: Vault Auth Helper

Add:

```bash
pfterminal vault auth-helper <label>
```

The helper prints only the requested secret to stdout for Claude's auth helper
contract and logs no secret material. It should fail closed when the vault is
locked, the label is missing, or the label is not an allowed provider
credential for the pane profile.

### P1: Pane Registry And `/panes`

Add a registry for user panes:

- stable pane id;
- title;
- backend type;
- cwd;
- provider profile;
- Claude session id, when known;
- status;
- latest usage summary;
- artifact path.

Add `SlashCommand::Panes`, `AppEvent::OpenPanePicker`, and `AppEvent` variants
for selecting and creating panes. Reuse the existing `ListSelectionView`
surface, but render grouped rows with `User Panes` and `Agent Panes`.

### P1: Claude Headless Backend

Implement the backend that:

- writes pane-local settings;
- resolves provider credentials through the vault;
- invokes Claude Code headless;
- parses JSON/stream-json;
- maps events to PFTerminal history cells;
- stores raw JSONL artifacts outside the active chat context.

### P2: Codex Tool Bridge

Expose a bounded `claude_code_exec` tool after interactive panes are stable.
The tool should return summary and artifact references, not raw full
transcripts.

### P3: Claude Plugin

Build a Claude plugin only after the host-owned vault/helper path works. The
plugin can provide metadata and user-approved actions, but should not expose
raw secrets to Claude model context.

## Acceptance Criteria

- `/panes` opens a picker with `User Panes` and `Agent Panes`.
- An Ambient Claude pane can be created from a vault-stored
  `provider/ambient_api_key` without editing `~/.claude/settings.json`.
- The active footer labels the selected Claude pane so users know new prompts
  are routed to Claude Code, not the main Codex model.
- Direct Z.AI, Baseten, OpenRouter, Vercel, and Claude Plan profiles pass the live
  provider smoke or fail with structured provider/audit status.
- A two-turn Claude pane remembers prior context through `--resume`.
- The pane backend prevents concurrent sends to the same Claude session.
- Raw provider keys do not appear in PFTerminal chat history, Claude transcript
  rendering, logs, or command argv.
- Agent panes continue to use existing `/agent` behavior.
- The implementation can be disabled without changing core Codex subagent
  behavior.

## Risks

- Claude Code CLI flags and settings behavior can change. The backend must
  fail with a clear diagnostic when a required flag is missing.
- Provider-specific Anthropic-compatible endpoints may differ in header,
  model-name, and caching behavior.
- `apiKeyHelper` support must be validated on every supported Claude Code
  version; otherwise use child-process env injection as a fallback.
- Raw Claude stream-json may include large tool outputs. Store artifacts and
  render bounded summaries.
- The first implementation must not become a hidden terminal multiplexer. If
  full interactive Claude TUI embedding is needed later, treat it as a separate
  PTY/tmux feature.

## References

- Claude Code settings: <https://code.claude.com/docs/en/settings>
- Claude Code CLI reference: <https://code.claude.com/docs/en/cli-reference>
- Z.AI Claude Code integration: <https://docs.z.ai/devpack/tool/claude>
- Baseten Model APIs: <https://docs.baseten.co/inference/model-apis/overview>
- OpenRouter Claude Code integration:
  <https://openrouter.ai/docs/cookbook/coding-agents/claude-code-integration>
