# Ambient Integration

Ambient is currently the default PFTerminal provider path.

## Current Provider

The built-in Ambient provider is defined in `codex-rs/model-provider-info/src/lib.rs`:

| Field | Value |
| --- | --- |
| Provider id | `ambient` |
| Display name | `Ambient` |
| Base URL | `https://api.ambient.xyz/v1` |
| API key env var | `AMBIENT_API_KEY` |
| Wire API | `chat` |
| OpenAI auth required | `false` |
| WebSockets | `false` |

Auth guidance shown to users:

```text
Set AMBIENT_API_KEY to your Ambient API key.
```

`codex-rs/login/src/auth/manager.rs` also reads `AMBIENT_API_KEY` as an API-key auth source when API-key environment authentication is enabled.

## Current Model

Ambient's visible default model is bundled in `codex-rs/models-manager/models.json`:

| Field | Value |
| --- | --- |
| Slug | `zai-org/GLM-5.2-FP8` |
| Display name | `Ambient GLM 5.2` |
| Context window | `202752` tokens |
| Default reasoning level | `medium` |
| Listed in picker | yes |
| Parallel tool calls | yes |

The bundled model uses PFTerminal base instructions instead of upstream Codex-only phrasing.

`ambient/large` remains bundled as a hidden compatibility model with a `131072` token context window.

## Request Behavior

Ambient uses the Chat Completions style provider surface. PFTerminal applies GLM-specific request shaping in `codex-rs/core/src/client.rs`:

- `medium` or default effort maps to provider `reasoning_effort=high`.
- `xhigh`, `high`, or custom deep/max aliases map to `reasoning_effort=max`.
- Ambient/Z.AI requests set `enable_thinking=true`.
- Ambient/Z.AI requests set `emit_usage=true`.
- Strict function-tool wrapper metadata is removed for GLM compatibility while preserving the JSON schema.

For Responses-style code paths, PFTerminal keeps only function tools for Ambient/Z.AI and avoids OpenAI-only metadata such as prompt cache keys and encrypted reasoning includes.

## Input Conversion

`codex-rs/codex-api/src/common.rs` converts structured Responses turn items into Ambient/Z.AI-compatible string input when needed. Hidden reasoning items are intentionally not replayed into the provider input, because that would turn model-internal reasoning state into visible conversational context.

Tool calls and tool outputs are serialized into assistant/tool chunks so the model can continue from previous tool interactions without requiring the provider to accept the full Responses item schema.

## Onboarding And UI

The onboarding provider picker can show Ambient as an API-key provider account. The relevant code lives in `codex-rs/tui/src/onboarding/auth.rs`.

Ambient keys saved through onboarding are stored in the encrypted vault at:

```text
provider/ambient_api_key
```

The environment variable `AMBIENT_API_KEY` is still supported for temporary
shells and automation.

The model picker recognizes Ambient model slugs from:

- `zai-org/`
- `ambient/`
- the exact default `zai-org/GLM-5.2-FP8`

Reasoning labels are productized for GLM:

- `Standard` for normal mode.
- `Deep` for max/deep mode.

The status line renders those same modes as `standard` and `deep`.

## Source

- `codex-rs/model-provider-info/src/lib.rs`
- `codex-rs/models-manager/models.json`
- `codex-rs/core/src/client.rs`
- `codex-rs/codex-api/src/common.rs`
- `codex-rs/tui/src/onboarding/auth.rs`
- `codex-rs/tui/src/chatwidget/model_popups.rs`
- `codex-rs/login/src/auth/provider_key_vault.rs`
