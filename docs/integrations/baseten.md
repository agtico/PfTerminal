# Baseten Integration

Baseten is a built-in metered provider path for GLM 5.2.

## Current Provider

The built-in Baseten provider is defined in
`codex-rs/model-provider-info/src/lib.rs`:

| Field | Value |
| --- | --- |
| Provider id | `baseten` |
| Display name | `Baseten` |
| Base URL | `https://inference.baseten.co/v1` |
| API key env var | `BASETEN_API_KEY` |
| Wire API | `chat` |
| OpenAI auth required | `false` |
| WebSockets | `false` |

Auth guidance shown to users:

```text
Set BASETEN_API_KEY to your Baseten API key.
```

## Current Model

The visible Baseten model is bundled in
`codex-rs/models-manager/models.json`:

| Field | Value |
| --- | --- |
| Slug | `zai-org/GLM-5.2` |
| Display name | `Baseten GLM 5.2` |
| Description | `Baseten: GLM 5.2 - $1.50/M input, $0.30/M cached input, $4.50/M output.` |
| Context window | `1048576` tokens |
| Listed in picker | yes |
| Parallel tool calls | yes |

## Model And Provider Selection

PFTerminal maps the exact model `zai-org/GLM-5.2` to provider `baseten` in
`codex-rs/tui/src/chatwidget/model_popups.rs`.

Example:

```bash
pfterminal -m zai-org/GLM-5.2
```

## Vault Behavior

Baseten keys saved through onboarding are stored in the encrypted vault at:

```text
provider/baseten_api_key
```

The environment variable `BASETEN_API_KEY` is still supported for temporary
shells and automation.

## Source

- `codex-rs/model-provider-info/src/lib.rs`
- `codex-rs/models-manager/models.json`
- `codex-rs/tui/src/onboarding/auth.rs`
- `codex-rs/tui/src/chatwidget/model_popups.rs`
- `codex-rs/login/src/auth/provider_key_vault.rs`
