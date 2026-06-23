# Getting Started

Use this page after PFTerminal is installed. For a new-machine install, start
with [Install And First Run](install.md).

## First Run

Start PFTerminal from the workspace you want it to inspect:

```bash
cd ~/repos/my-project
pfterminal
```

If you built from source and are launching the debug binary directly, keep
PFTerminal state separate from stock Codex:

```bash
export CODEX_HOME="${PFTERMINAL_HOME:-$HOME/.pfterminal}"
/path/to/PfTerminal/codex-rs/target/debug/pfterminal
```

On first run, choose a provider account and enter the API key. The key is stored
in the encrypted vault, not in the chat transcript.

## Provider Choices

PFTerminal currently ships these provider paths:

| Use case                          | Provider   | Model                                                                   |
| --------------------------------- | ---------- | ----------------------------------------------------------------------- |
| Ambient coding plan               | Ambient    | `zai-org/GLM-5.2-FP8`                                                   |
| Z.AI coding plan                  | Z.AI       | `glm-5.2`                                                               |
| Metered GLM via OpenRouter        | OpenRouter | `z-ai/glm-5.2`                                                          |
| Metered alternative coding models | OpenRouter | `minimax/minimax-m3`, `openrouter/owl-alpha`, `google/gemini-3.5-flash` |
| Metered GLM via Baseten           | Baseten    | `zai-org/GLM-5.2`                                                       |

Open `/model` to switch models. You can also start with a specific model:

```bash
pfterminal -m glm-5.2
pfterminal -m z-ai/glm-5.2
pfterminal -m minimax/minimax-m3
```

## Vault Basics

Open the vault menu:

```text
/vault
```

Common checks:

```text
/vault list
/vault show provider/ambient_api_key
/vault credential add
```

Provider API keys stored through onboarding use labels such as
`provider/ambient_api_key`, `provider/zai_api_key`,
`provider/openrouter_api_key`, and `provider/baseten_api_key`.

## Useful Slash Commands

| Command    | Purpose                                                                  |
| ---------- | ------------------------------------------------------------------------ |
| `/model`   | Select provider model and reasoning/effort mode                          |
| `/vault`   | Add, inspect, or delete credentials without exposing raw secrets to chat |
| `/skills`  | Browse bundled and installed skills                                      |
| `/status`  | Inspect current model/provider/session state                             |
| `/compact` | Compact a long conversation                                              |

## Verify Setup

After adding a key:

1. Run `/vault` and confirm the provider credential exists.
2. Run `/model` and select the provider model you want.
3. Ask a small repo-local question, such as `summarize this repository`.

If a provider reports a missing environment variable, add the key through the
provider login/onboarding UI or export the relevant env var for that shell:
`AMBIENT_API_KEY`, `ZAI_API_KEY`, `OPENROUTER_API_KEY`, or `BASETEN_API_KEY`.
