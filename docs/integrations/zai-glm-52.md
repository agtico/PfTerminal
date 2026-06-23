# Z.AI GLM 5.2 Integration

PFTerminal also supports direct GLM 5.2 access through the Z.AI coding plan API.

## Current Provider

The built-in Z.AI provider is defined in `codex-rs/model-provider-info/src/lib.rs`:

| Field | Value |
| --- | --- |
| Provider id | `zai` |
| Display name | `Z.AI` |
| Base URL | `https://api.z.ai/api/coding/paas/v4` |
| API key env var | `ZAI_API_KEY` |
| Wire API | `chat` |
| OpenAI auth required | `false` |
| WebSockets | `false` |

Auth guidance shown to users:

```text
Set ZAI_API_KEY to your Z.AI Plan API key.
```

## Current Model

The visible Z.AI model is bundled in `codex-rs/models-manager/models.json`:

| Field | Value |
| --- | --- |
| Slug | `glm-5.2` |
| Display name | `Z.AI GLM 5.2` |
| Description | `GLM 5.2 through the Z.AI coding plan API.` |
| Context window | `1000000` tokens |
| Default reasoning level | `medium` |
| Listed in picker | yes |
| Parallel tool calls | yes |

## Model And Provider Selection

PFTerminal maps model selections to providers in `codex-rs/tui/src/chatwidget/model_popups.rs`:

- `glm-*` and `glm-5.2` resolve to provider `zai`.
- `zai-org/*` resolves to provider `ambient`.
- The all-models popup only shows PFTerminal-relevant Ambient and Z.AI models by default.

Configuration normalization in `codex-rs/core/src/config/mod.rs` keeps Z.AI sessions on Z.AI-compatible models:

- If `model_provider = "zai"` and no compatible model is configured, PFTerminal selects `glm-5.2`.
- If a configured Z.AI model does not start with `glm-`, PFTerminal replaces it with `glm-5.2`.
- Z.AI reasoning effort is normalized to the GLM `Standard`/`Deep` behavior.

## Request Behavior

Z.AI shares the GLM request compatibility path with Ambient:

- PFTerminal emits `enable_thinking=true`.
- PFTerminal emits `emit_usage=true`.
- PFTerminal sends `reasoning_effort=high` for Standard mode.
- PFTerminal sends `reasoning_effort=max` for Deep mode.
- Function-tool schemas omit the OpenAI `strict` wrapper bit because GLM chat streams tool calls correctly without it.

Z.AI has one extra guard in the Chat Completions path: when native `web_search` and function tools would be mixed, PFTerminal preserves client-executed function tools and removes native `web_search`, because coding sessions need shell/file tools to continue.

## Onboarding

The onboarding flow can present Z.AI as a provider API-key account alongside Ambient. This is tested in `codex-rs/tui/src/onboarding/auth.rs`.

Z.AI keys saved through onboarding are stored in the encrypted vault at:

```text
provider/zai_api_key
```

The environment variable `ZAI_API_KEY` is still supported for temporary shells
and automation.

## Source

- `codex-rs/model-provider-info/src/lib.rs`
- `codex-rs/models-manager/models.json`
- `codex-rs/core/src/config/mod.rs`
- `codex-rs/core/src/client.rs`
- `codex-rs/tui/src/onboarding/auth.rs`
- `codex-rs/tui/src/chatwidget/model_popups.rs`
- `codex-rs/login/src/auth/provider_key_vault.rs`
