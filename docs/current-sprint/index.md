# Current Sprint

The current PFTerminal sprint is the v0 credential store.

The sprint goal is to stop treating crypto keys and spend-bearing API keys as
ad hoc shell state. PFTerminal needs a native encrypted store for labeled
credentials, created automatically on login/startup and usable through
`/vault`.

## What Exists Now

| Area | Current State | Where To Read |
| --- | --- | --- |
| Ambient and Z.AI provider auth | API-key provider flows exist for Ambient and Z.AI. | [Ambient](../integrations/ambient.md), [Z.AI GLM 5.2](../integrations/zai-glm-52.md) |
| Local secret substrate | The Codex-derived workspace has local encrypted secret storage and keyring support. | `codex-rs/secrets`, `codex-rs/keyring-store` |
| Provider key path | Codex already has provider API-key storage, but it is not a general credential store. | `codex-rs/login/src/auth/manager.rs` |
| Vault/auth design | Active sprint spec defines automatic vault initialization, secure credential entry, labels, lock/unlock behavior, and provider-key use by label. | [Credential Store](authentication.md) |
| GLM 5.2 tool compatibility | Current sprint logs and OpenCode source show GLM-class models should be routed to structured edit/write tools instead of forced through strict `apply_patch`. | [GLM 5.2 Tool Compatibility](glm-52-tool-compatibility.md) |
| Hammer reduction | Current sprint study compares PFTerminal/Codex, OpenCode, Hermes Agent, Kilo Code, and Cline to reduce repeated large provider requests, 429 loops, and context bloat. | [Hammer Reduction Process](hammer-reduction-process.md) |
| Tool-call runaway remedy | Live worker logs show malformed oversized `structured_write` calls looping through normal follow-up handling. The remedy is a non-retriable malformed-tool boundary plus bounded/chunked write mechanics. | [Tool Call Runaway Remedy](tool-call-runaway-remedy.md) |
| Subagents | Current Codex supports explicit subagent workflows, but PFTerminal must make the tool exposure reliable and diagnosable across third-party provider sessions. | [PFTerminal Subagents](subagents.md) |
| Codex account login | OpenAI Codex account login should appear as a provider credential, use device auth from `/providers`, expose only GPT-5.5, and avoid wiping provider vault keys on default logout. | [Codex Account Login](codex-account-login.md) |
| Claude headless panes | `/panes` has an Ambient-backed Claude Code pane path passing mock website, NumPy vs Pandas benchmark, fresh+resumed code review, and turn auditability checks; non-Ambient profiles remain experimental. | [Implementation Note](claude-headless-panes.md), [Completion Spec](claude-code-integration-completion-spec.md) |

## Sprint Reading Path

1. Read [Credential Store](authentication.md).
2. Read [Ambient](../integrations/ambient.md) and
   [Z.AI GLM 5.2](../integrations/zai-glm-52.md) for the provider accounts
   that motivate the first API-key credentials.
3. Read [Codex Fork](../integrations/codex-fork.md) for the inherited runtime
   surfaces that the sprint should reuse.
4. Read [GLM 5.2 Tool Compatibility](glm-52-tool-compatibility.md) for the
   proposal to make open-source coding models reliable inside PFTerminal's
   tool loop.
5. Read [Hammer Reduction Process](hammer-reduction-process.md) for the current
   sprint plan to reduce provider hammering, context bloat, and repeated
   high-input retries.
6. Read [Tool Call Runaway Remedy](tool-call-runaway-remedy.md) for the
   structural fix needed before editing the runtime loop.
7. Read [PFTerminal Subagents](subagents.md) for the plan to make basic
   subagent delegation visible, provider-compatible, and debuggable.
8. Read [Codex Account Login](codex-account-login.md) for the plan to
   reintegrate OpenAI Codex account auth into `/providers` and Coding Plans.
9. Read [Claude Headless Panes](claude-headless-panes.md) for the current
   Ambient-backed `/panes` implementation.
10. Read [Claude Code Completion Spec](claude-code-integration-completion-spec.md)
   for the actual completion bar before calling the Claude Code integration
   done.

## Boundary

This sprint is not automatic transaction execution, hosted custody, MPC, or a
StakeHub replacement. It is a credential store plus the harness hardening
needed to use third-party coding providers without wasteful request loops.
