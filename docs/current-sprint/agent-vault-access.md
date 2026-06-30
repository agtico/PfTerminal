# Agent Vault Access Proposal

Status: current security sprint.

## Goal

PFTerminal agents, subagents, and Claude panes need to use provider API keys
without learning, printing, inheriting, or storing raw secrets.

The correct security model is:

- Users store credentials in `/vault` or `/providers`.
- Agents request provider capabilities by name.
- PFTerminal resolves vault credentials outside the model context.
- Tools and provider transports receive only the minimum credential material
  needed to execute the request.
- Audit logs record credential access without recording the secret.

Agents should not know how to read the vault directly.

## Problem

Today an agent can need keys for Ambient, Z.AI, OpenRouter, Baseten, Vercel, or
Claude Code routing. If this is handled by telling the model how to read
secrets, or by placing long-lived provider keys in process environment
variables, the system creates avoidable leaks:

- The key can enter the prompt, transcript, JSONL artifact, or audit file.
- A shell-capable agent can run `env` and read inherited secrets.
- A subagent can copy the credential into its own context.
- A failed or looping tool call can repeatedly expose secret-bearing state.
- Provider credentials become distributed across shells instead of staying in
  the vault.

This is especially bad for PFTerminal because the app is explicitly meant to
orchestrate multiple paid providers and spend-bearing credentials.

## Security Boundary

The vault is a host capability. It is not an agent tool.

| Actor | Allowed | Not Allowed |
| --- | --- | --- |
| User | Add, replace, inspect metadata, and explicitly copy/reveal credentials. | Accidentally paste secrets into chat context. |
| Agent/model | Ask to use a provider capability such as `zai` or `openrouter`. | Read raw vault records, print secrets, or choose arbitrary vault labels. |
| PFTerminal runtime | Resolve approved vault labels, mint short-lived leases, proxy provider calls, and audit access. | Persist raw secrets in pane artifacts or transcripts. |
| Provider transport | Receive the real provider key only inside the final outbound request path. | Expose the key to model-visible tool output. |

## Proposed Architecture

### 1. Provider Capability Registry

Create a single registry that maps provider capabilities to vault labels and
transport behavior.

Example:

| Capability | Vault Label | Transport |
| --- | --- | --- |
| `ambient` | `provider/ambient_api_key` | Ambient API / Claude bridge |
| `zai` | `provider/zai_api_key` | Z.AI API / Anthropic-compatible bridge |
| `openrouter` | `provider/openrouter_api_key` | OpenRouter API |
| `baseten` | `provider/baseten_api_key` | Baseten API |
| `vercel` | `provider/ai_gateway_api_key` | Vercel AI Gateway |

Pane profiles and model-provider configs should reference capabilities, not raw
environment variables.

### 2. Vault Lease Broker

Add a local runtime broker that mints scoped, short-lived leases.

The model sees only a lease identifier, never the provider key.

Example launch environment for a Claude pane:

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:<pfterminal-port>/broker/claude/zai
ANTHROPIC_AUTH_TOKEN=pft_lease_<opaque-id>
```

The broker:

- validates the lease;
- checks that the lease is scoped to the requested provider and pane;
- reads the real key from vault;
- forwards the outbound request;
- strips or redacts secret-bearing headers from logs;
- records non-secret audit metadata.

### 3. No Raw Key Inheritance

Do not pass provider keys as long-lived environment variables to shell-capable
agent processes.

This is the core difference between an acceptable prototype and a safe
production design. If a process can run Bash and inherits `ZAI_API_KEY`, then
the model can ask Bash to print it. A brokered lease avoids that class of leak.

### 4. Missing Credential UX

When a pane or provider needs a missing credential, the app should not show a
raw environment-variable error as the primary instruction.

Preferred message:

```text
Z.AI API key is not configured.
Run /providers and choose "Provider: Z.AI API Key".
```

The environment variable name can appear as secondary technical detail, but the
first path should be the PFTerminal provider UI.

### 5. Audit Model

Every credential use should write a non-secret audit event:

```json
{
  "event": "vault_credential_used",
  "pane_id": "claude-...",
  "capability": "zai",
  "vault_label": "provider/zai_api_key",
  "transport": "claude_broker",
  "timestamp": "...",
  "request_id": "...",
  "secret_material_recorded": false
}
```

Do not record:

- raw API keys;
- authorization headers;
- request bodies containing secrets;
- provider-specific bearer tokens;
- shell environment dumps.

## Runtime Flow

### Provider-backed Codex Turn

1. User selects `glm-5.2` through `/model`.
2. Model provider config resolves to capability `zai`.
3. Runtime asks vault for `provider/zai_api_key`.
4. Runtime sends the key only inside the outbound HTTP request.
5. Transcript records provider/model metadata, not the key.

### Claude Pane Turn

1. User creates `Claude Code - GLM 5.2 Z.AI`.
2. Pane profile resolves to capability `zai`.
3. Runtime creates a lease scoped to that pane and provider.
4. Claude receives local broker URL plus lease token.
5. Broker forwards Claude-compatible requests with the real vault key.
6. Pane artifacts record tool calls, status, and audit paths, not the key.

### Subagent Turn

1. Parent agent starts a subagent with a provider profile.
2. PFTerminal assigns a capability set to the subagent.
3. Subagent can use provider transport through the broker.
4. Subagent cannot enumerate or reveal vault contents.
5. Audit ties each credential use to the subagent id.

## Implementation Plan

### Phase 0: Policy and Labels

- Define canonical provider capability ids.
- Define canonical provider vault labels.
- Make `/providers` and `/vault` write the same labels.
- Add one source of truth for provider label display names.

### Phase 1: Resolve Keys Through Vault

- Replace missing env-var startup failures with vault-first lookup.
- Keep env vars only as migration fallback or explicit developer override.
- Ensure provider login writes to vault and removes migrated plaintext records
  when safe.

### Phase 2: Broker for Shell-Capable Panes

- Add a local HTTP broker bound to loopback.
- Mint per-pane leases with expiration and provider scope.
- Route Claude pane provider calls through the broker.
- Remove raw provider API keys from inherited pane environments.

### Phase 3: Subagent Integration

- Attach allowed capabilities to subagent metadata.
- Pass broker leases to subagent runtime where needed.
- Add `/panes` and `/agents` detail views showing allowed providers without
  showing secrets.

### Phase 4: Audit and Redaction Hardening

- Add structured audit events for every broker credential use.
- Add tests that scan artifacts and audit files for seeded fake secrets.
- Redact secret-bearing headers from all provider transport errors.

## Acceptance Criteria

1. A user can add Ambient, Z.AI, OpenRouter, Baseten, and Vercel keys through
   `/providers`.
2. `/vault` shows provider credential metadata without printing secrets.
3. A Codex provider turn can use a vault-backed provider key without an env var.
4. A Claude pane can use a provider key without inheriting the raw key in `env`.
5. A shell-capable pane running `env` does not reveal provider keys.
6. A subagent can use an assigned provider capability but cannot enumerate or
   reveal vault records.
7. Pane artifacts, JSONL logs, transcripts, and audit files do not contain a
   seeded fake API key.
8. Missing credentials direct the user to `/providers`, not raw shell setup.

## Non-Goals

- Building a general hosted secrets service.
- Replacing OpenBao or enterprise secret management.
- Giving agents arbitrary secret lookup tools.
- Letting models choose unapproved vault labels.
- Automatic crypto transaction signing.

## Open Questions

- Should leases be stored only in memory, or also in a short-lived local
  sqlite table for crash recovery?
- Should the broker support per-request user confirmation for high-cost
  providers later?
- Should broker endpoints be one per provider transport, or a generic
  provider proxy with typed routes?
- How should leases behave when a pane is resumed after process restart?

## Recommendation

Implement vault-first provider resolution immediately, but treat raw environment
injection as temporary.

For production-quality agent and pane support, implement the local lease broker.
That keeps PFTerminal's core promise intact: many provider credentials, one
safe local vault, and no model-visible secret handling.
