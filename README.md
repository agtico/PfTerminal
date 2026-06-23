# PFTerminal

PFTerminal is a crypto-native AI services terminal based on the open-source Codex CLI. It defaults to Ambient's GLM 5.2 model and is intended to become one secure interface for crypto-native AI workflows.

## Current Focus

- Ambient API-key onboarding by default
- Ambient GLM 5.2 as the default model
- Z.AI, OpenRouter, and Baseten provider choices
- encrypted `/vault` storage for provider API keys and user credentials
- Codex-level coding workflows in a local terminal
- Future crypto-native services such as authentication, Hyperliquid, GPU rentals, staking, borrowing, and related workflows

## Install And Setup

For a new machine, read the repo docs:

- [Install And First Run](docs/install.md)
- [Getting Started](docs/getting-started.md)
- [Authentication And Vault](docs/authentication.md)
- [Configuration](docs/config.md)

Those pages cover provider keys for Ambient, Z.AI, OpenRouter, and Baseten,
vault labels such as `provider/zai_api_key`, and model selection through
`/model` or `pfterminal -m <model>`.

## Running Locally From Source

From this repository:

```shell
cd codex-rs
cargo build -p codex-cli --bin pfterminal
```

Launch it from the workspace you want PFTerminal to inspect:

```shell
export CODEX_HOME="${PFTERMINAL_HOME:-$HOME/.pfterminal}"
cd /home/postfiat/repos
/home/postfiat/repos/PfTerminal/codex-rs/target/debug/pfterminal
```

Some release paths still expose the upstream-compatible `codex` command. The
npm package and source build expose the product-facing `pfterminal` command.

## Upstream

PFTerminal is based on the open-source Codex CLI project. Keep upstream changes isolated through the `upstream` remote and land PFTerminal changes through this repository.

This repository is licensed under the [Apache-2.0 License](LICENSE).
