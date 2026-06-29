# PFTerminal

PFTerminal is a crypto-native AI services terminal based on the open-source
Codex CLI. It defaults to Ambient GLM 5.2 and is being built as one secure
terminal interface for AI-assisted coding and crypto-native workflows.

## Install

### Linux

```bash
curl -fsSL https://github.com/agtico/PfTerminal/releases/latest/download/install.sh | sh
```

### macOS

Download the latest DMG from
[GitHub Releases](https://github.com/agtico/PfTerminal/releases/latest):

- `PFTerminal-aarch64-apple-darwin.dmg` for Apple Silicon
- `PFTerminal-x86_64-apple-darwin.dmg` for Intel Macs

Terminal install also works on macOS:

```bash
curl -fsSL https://github.com/agtico/PfTerminal/releases/latest/download/install.sh | sh
```

The installer creates a `pfterminal` command, leaves any stock `codex` command
alone, and stores PFTerminal state in `$HOME/.pfterminal` by default.

## Key Features

- Ambient GLM 5.2 default model path.
- Provider choices for Ambient, Z.AI, OpenRouter, Baseten, Vercel, and OpenAI
  Codex account auth.
- Encrypted `/vault` storage for provider API keys and user credentials.
- Codex-level coding workflows in a local terminal.
- Native pane orchestration for Sauron → Nazgul → Troll → Orc agent workflows.
- Separate PFTerminal home at `$HOME/.pfterminal`, so it does not collide with
  a stock Codex install.
- Planned crypto-native services: authentication, Hyperliquid, GPU rentals,
  staking, borrowing, and related workflows.

## First Run

Launch PFTerminal from the workspace you want it to inspect:

```bash
cd ~/repos
pfterminal
```

Use:

- `/providers` to add Ambient, Z.AI, OpenRouter, Baseten, Vercel, or OpenAI
  Codex credentials.
- `/vault` to manage encrypted credentials.
- `/model` or `pfterminal -m <model>` to choose a model.
- `/spawn` to create and route multi-agent work.

More setup detail:

- [Install And First Run](docs/install.md)
- [Getting Started](docs/getting-started.md)
- [Authentication And Vault](docs/authentication.md)
- [Configuration](docs/config.md)

## Source Build

```bash
git clone https://github.com/agtico/PfTerminal.git
cd PfTerminal/codex-rs
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo build -p codex-cli --bin pfterminal
```

Then run:

```bash
./target/debug/pfterminal
```

## Upstream

PFTerminal is based on the open-source Codex CLI project. Keep upstream changes
isolated through the `upstream` remote and land PFTerminal changes through this
repository.

This repository is licensed under the [Apache-2.0 License](LICENSE).
