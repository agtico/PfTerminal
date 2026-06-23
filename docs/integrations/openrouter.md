# OpenRouter Integration

OpenRouter is a built-in metered provider for PFTerminal.

## Current Provider

The built-in OpenRouter provider is defined in
`codex-rs/model-provider-info/src/lib.rs`:

| Field | Value |
| --- | --- |
| Provider id | `openrouter` |
| Display name | `OpenRouter` |
| Base URL | `https://openrouter.ai/api/v1` |
| API key env var | `OPENROUTER_API_KEY` |
| Wire API | `chat` |
| OpenAI auth required | `false` |
| WebSockets | `false` |

Auth guidance shown to users:

```text
Set OPENROUTER_API_KEY to your OpenRouter API key.
```

## Current Models

Visible OpenRouter models are bundled in
`codex-rs/models-manager/models.json`:

| Slug | Display name | Listed pricing text |
| --- | --- | --- |
| `z-ai/glm-5.2` | OpenRouter GLM 5.2 | `$0.98/M input`, `$3.08/M output` |
| `minimax/minimax-m3` | OpenRouter MiniMax M3 | `$0.30/M input`, `$1.20/M output` |
| `openrouter/owl-alpha` | OpenRouter Owl Alpha | `$0/M input`, `$0/M output` |
| `google/gemini-3.5-flash` | OpenRouter Gemini 3.5 Flash | `$1.50/M input`, `$9.00/M output` |

OpenRouter GLM supports `high` and `xhigh` reasoning levels in the current
model metadata. The other visible OpenRouter models are listed without a
default reasoning level unless the model metadata specifies one.

## Model And Provider Selection

PFTerminal maps these model slugs to provider `openrouter` in
`codex-rs/tui/src/chatwidget/model_popups.rs`.

Examples:

```bash
pfterminal -m z-ai/glm-5.2
pfterminal -m minimax/minimax-m3
pfterminal -m openrouter/owl-alpha
pfterminal -m google/gemini-3.5-flash
```

## Vault Behavior

OpenRouter keys saved through onboarding are stored in the encrypted vault at:

```text
provider/openrouter_api_key
```

The environment variable `OPENROUTER_API_KEY` is still supported for temporary
shells and automation.

## Source

- `codex-rs/model-provider-info/src/lib.rs`
- `codex-rs/models-manager/models.json`
- `codex-rs/tui/src/onboarding/auth.rs`
- `codex-rs/tui/src/chatwidget/model_popups.rs`
- `codex-rs/login/src/auth/provider_key_vault.rs`
