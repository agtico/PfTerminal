# Configuration

PFTerminal inherits Codex configuration but ships PFTerminal-specific provider
defaults. Most users do not need to define model providers manually.

## Config Location

PFTerminal reads config from `CODEX_HOME/config.toml`.

Recommended PFTerminal home:

```bash
export CODEX_HOME="${PFTERMINAL_HOME:-$HOME/.pfterminal}"
```

If you use an installed `pfterminal` wrapper, it may set this automatically. If
you run the source-built binary directly, set it yourself to keep PFTerminal
state separate from stock Codex.

## Built-In Providers

These providers are compiled into PFTerminal:

| Provider id | Display name | Base URL | Env key | Wire API |
| --- | --- | --- | --- | --- |
| `ambient` | Ambient | `https://api.ambient.xyz/v1` | `AMBIENT_API_KEY` | Chat Completions |
| `zai` | Z.AI | `https://api.z.ai/api/coding/paas/v4` | `ZAI_API_KEY` | Chat Completions |
| `openrouter` | OpenRouter | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | Chat Completions |
| `baseten` | Baseten | `https://inference.baseten.co/v1` | `BASETEN_API_KEY` | Chat Completions |

Provider credentials should normally be stored through onboarding or `/vault`.
Environment variables are supported for temporary sessions and automation.

## Common Model Configs

Set a default provider and model in `$CODEX_HOME/config.toml`.

Ambient:

```toml
model_provider = "ambient"
model = "zai-org/GLM-5.2-FP8"
```

Z.AI:

```toml
model_provider = "zai"
model = "glm-5.2"
```

OpenRouter GLM:

```toml
model_provider = "openrouter"
model = "z-ai/glm-5.2"
```

OpenRouter MiniMax:

```toml
model_provider = "openrouter"
model = "minimax/minimax-m3"
```

Baseten GLM:

```toml
model_provider = "baseten"
model = "zai-org/GLM-5.2"
```

You can also select a model per run:

```bash
pfterminal -m glm-5.2
pfterminal -m z-ai/glm-5.2
pfterminal -m zai-org/GLM-5.2
```

The model picker maps these model slugs to the correct built-in provider.

## Vault And Secrets

Provider API keys saved by PFTerminal are stored in the encrypted vault, not in
`config.toml`.

Vault labels:

```text
provider/ambient_api_key
provider/zai_api_key
provider/openrouter_api_key
provider/baseten_api_key
```

Do not put long-lived provider keys in `experimental_bearer_token` unless you
are intentionally running an automation-only setup. For interactive use, use
onboarding or `/vault`.

## Provider Overrides

Advanced users can still define custom providers under `[model_providers]`.
Prefer built-ins unless you need a different base URL, custom headers, or an
external bearer-token command.

Example custom OpenAI-compatible Chat provider:

```toml
model_provider = "custom-chat"
model = "some/model"

[model_providers.custom-chat]
name = "Custom Chat Provider"
base_url = "https://example.com/v1"
env_key = "CUSTOM_PROVIDER_API_KEY"
wire_api = "chat"
```

For inherited Codex configuration details, see:

- Basic configuration: <https://developers.openai.com/codex/config-basic>
- Advanced configuration: <https://developers.openai.com/codex/config-advanced>
- Full reference: <https://developers.openai.com/codex/config-reference>

## Lifecycle Hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
