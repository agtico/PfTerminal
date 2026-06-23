# Core Integrations

Status: current repository state as of 2026-06-23.

PFTerminal is a Codex CLI fork with product-specific model provider,
onboarding, packaging, and branding changes.

The important boundary: PFTerminal still uses the Codex execution engine, tool
system, approval flows, sandboxing, and session mechanics, while adding Ambient,
Z.AI, OpenRouter, and Baseten as first-class provider choices.

## What Exists Now

| Area | Current state | Primary paths |
| --- | --- | --- |
| Ambient provider | Built-in provider named `ambient`, using `AMBIENT_API_KEY` and the Chat Completions wire shape. | `codex-rs/model-provider-info/src/lib.rs` |
| Ambient default model | Bundled model `zai-org/GLM-5.2-FP8`, displayed as `Ambient GLM 5.2`. | `codex-rs/models-manager/models.json` |
| Z.AI provider | Built-in provider named `zai`, using `ZAI_API_KEY` and the Z.AI coding plan API base URL. | `codex-rs/model-provider-info/src/lib.rs` |
| OpenRouter provider | Built-in provider named `openrouter`, using `OPENROUTER_API_KEY` and OpenRouter metered model slugs. | `codex-rs/model-provider-info/src/lib.rs` |
| Baseten provider | Built-in provider named `baseten`, using `BASETEN_API_KEY` and Baseten GLM 5.2. | `codex-rs/model-provider-info/src/lib.rs` |
| Provider key vault | Provider keys are stored in encrypted vault labels such as `provider/zai_api_key`. | `codex-rs/login/src/auth/provider_key_vault.rs`, `codex-rs/vault/` |
| GLM request shaping | Ambient and Z.AI requests map PFTerminal reasoning levels to provider-specific `reasoning_effort`, `enable_thinking`, and `emit_usage` fields. | `codex-rs/core/src/client.rs` |
| Ambient/Z.AI input conversion | Responses-style turn items are flattened for Ambient/Z.AI string input, while hidden reasoning is not replayed. | `codex-rs/codex-api/src/common.rs` |
| Onboarding | Provider API-key picker supports Ambient, Z.AI, OpenRouter, and Baseten accounts. | `codex-rs/tui/src/onboarding/auth.rs` |
| Model picker | The PFTerminal model picker groups Ambient/Z.AI under Coding Plans and OpenRouter/Baseten under Pay Per API Call. | `codex-rs/tui/src/chatwidget/model_popups.rs` |
| Product branding | TUI, login prompts, installer messages, package names, and status surfaces use PFTerminal/Post Fiat Terminal branding. | `codex-rs/tui/`, `codex-rs/login/`, `codex-cli/`, `scripts/install/` |

## Design line

Provider-specific compatibility should stay small and explicit:

- Provider constants and built-in provider definitions belong in `codex-rs/model-provider-info`.
- Model metadata belongs in `codex-rs/models-manager/models.json`.
- Request serialization differences belong in `codex-rs/core/src/client.rs` or `codex-rs/codex-api`.
- UI selection behavior belongs in the TUI model and onboarding modules.

Avoid spreading provider assumptions through prompts or docs-only instructions. If the agent needs a capability, model, or provider behavior, it should be represented in configuration, model metadata, or typed code.

## Reading Path

1. [Ambient](ambient.md) for the default provider path.
2. [Z.AI GLM 5.2](zai-glm-52.md) for the direct Z.AI coding-plan path.
3. [OpenRouter](openrouter.md) for metered OpenRouter models.
4. [Baseten](baseten.md) for metered Baseten GLM 5.2.
5. [Codex Fork](codex-fork.md) for product changes around command names,
   packaging, status surfaces, and model picker behavior.
