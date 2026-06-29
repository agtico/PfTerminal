# Current Sprint

The current PFTerminal sprint is the v0 credential store.

The sprint goal is to stop treating crypto keys and spend-bearing API keys as
ad hoc shell state. PFTerminal needs a native encrypted store for labeled
credentials, created automatically on login/startup and usable through
`/vault`.

## What Exists Now

| Area                           | Current State                                                                                                                                                                                             | Where To Read                                                                                                  |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| Ambient and Z.AI provider auth | API-key provider flows exist for Ambient and Z.AI.                                                                                                                                                        | [Ambient](../integrations/ambient.md), [Z.AI GLM 5.2](../integrations/zai-glm-52.md)                           |
| Local secret substrate         | The Codex-derived workspace has local encrypted secret storage and keyring support.                                                                                                                       | `codex-rs/secrets`, `codex-rs/keyring-store`                                                                   |
| Provider key path              | Codex already has provider API-key storage, but it is not a general credential store.                                                                                                                     | `codex-rs/login/src/auth/manager.rs`                                                                           |
| Vault/auth design              | Active sprint spec defines automatic vault initialization, secure credential entry, labels, lock/unlock behavior, and provider-key use by label.                                                          | [Credential Store](authentication.md)                                                                          |
| GLM 5.2 tool compatibility     | Current sprint logs and OpenCode source show GLM-class models should be routed to structured edit/write tools instead of forced through strict `apply_patch`.                                             | [GLM 5.2 Tool Compatibility](glm-52-tool-compatibility.md)                                                     |
| Hammer reduction               | Current sprint study compares PFTerminal/Codex, OpenCode, Hermes Agent, Kilo Code, and Cline to reduce repeated large provider requests, 429 loops, and context bloat.                                    | [Hammer Reduction Process](hammer-reduction-process.md)                                                        |
| Native provider performance    | Live benchmarks and patch notes for making native third-party provider turns competitive with wrapped Claude Code.                                                                                        | [Native Provider Performance](native-provider-performance.md)                                                  |
| Tool-call runaway remedy       | Live worker logs show malformed oversized `structured_write` calls looping through normal follow-up handling. The remedy is a non-retriable malformed-tool boundary plus bounded/chunked write mechanics. | [Tool Call Runaway Remedy](tool-call-runaway-remedy.md)                                                        |
| Subagents                      | Current Codex supports explicit subagent workflows, but PFTerminal must make the tool exposure reliable and diagnosable across third-party provider sessions.                                             | [PFTerminal Subagents](subagents.md)                                                                           |
| Spawn orchestration            | Scope down `docs/nazgul_spec.md` into `/spawn`: a user-facing creation flow for Troll and Orc roles with explicit model, harness, parent/child status, and upward reporting to the Nazgul pane.           | [Spawn Orchestration](spawn-orchestration.md)                                                                  |
| Spawn acceptance gate          | Live use showed `/spawn` is not done until pane state, dispatch provenance, supervisor action, duplicate-task prevention, and interrupts are pane-local and auditable.                                    | [Spawn Orchestration Acceptance](spawn-orchestration-acceptance.md)                                            |
| Agent vault access             | Agents, subagents, and Claude panes need provider credentials without reading raw vault records or inheriting long-lived API keys.                                                                        | [Agent Vault Access](agent-vault-access.md)                                                                    |
| Task Node GitHub bridge        | `/tasknode` should let a GitHub-authenticated PFTerminal user link Task Node, read tasks/rewards, and perform server-backed task actions without terminal wallet functions.                               | [Task Node GitHub Bridge](tasknode-github-bridge.md)                                                           |
| Codex account login            | OpenAI Codex account login should appear as a provider credential, use device auth from `/providers`, expose only GPT-5.5, and avoid wiping provider vault keys on default logout.                        | [Codex Account Login](codex-account-login.md)                                                                  |
| Claude headless panes          | Ambient `/panes` parity workflow suite passed after removing the hidden local tool-call ceiling; non-Ambient profiles remain experimental until they pass the same suite.                                 | [Implementation Note](claude-headless-panes.md), [Completion Spec](claude-code-integration-completion-spec.md) |

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
6. Read [Native Provider Performance](native-provider-performance.md) for the
   current native-provider latency measurements, fixes, and remaining parity
   work.
7. Read [Tool Call Runaway Remedy](tool-call-runaway-remedy.md) for the
   structural fix needed before editing the runtime loop.
8. Read [PFTerminal Subagents](subagents.md) for the plan to make basic
   subagent delegation visible, provider-compatible, and debuggable.
9. Read [Spawn Orchestration](spawn-orchestration.md) for the scoped `/spawn`
   plan that turns the Nazgul/Troll/Orc hierarchy into a concrete
   parent-aware workflow.
10. Read [Spawn Orchestration Acceptance](spawn-orchestration-acceptance.md)
    for the actual done criteria before calling `/spawn` orchestration complete.
11. Read [Agent Vault Access](agent-vault-access.md) for the proposal to let
    agents use provider credentials without direct secret access.
12. Read [Task Node GitHub Bridge](tasknode-github-bridge.md) for the `/tasknode`
    bridge contract between PFTerminal and Task Node.
13. Read [Codex Account Login](codex-account-login.md) for the plan to
    reintegrate OpenAI Codex account auth into `/providers` and Coding Plans.
14. Read [Claude Headless Panes](claude-headless-panes.md) for the current
    Ambient-backed `/panes` implementation.
15. Read [Claude Code Completion Spec](claude-code-integration-completion-spec.md)
    for the actual completion bar before calling the Claude Code integration
    done.

## Boundary

This sprint is not automatic transaction execution, hosted custody, MPC, or a
StakeHub replacement. It is a credential store plus the harness hardening
needed to use third-party coding providers without wasteful request loops.
