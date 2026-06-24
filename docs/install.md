# Install And First Run

This page is the new-machine setup runbook for PFTerminal. It covers the
binary, provider credentials, the encrypted vault, and model selection.

## System Requirements

| Requirement      | Details                                                        |
| ---------------- | -------------------------------------------------------------- |
| Operating system | macOS 12+, Ubuntu 20.04+/Debian 10+, or Windows 11 via WSL2    |
| Git              | 2.23+ recommended                                              |
| RAM              | 4 GB minimum, 8 GB recommended                                 |
| Rust             | Required only for source builds                                |
| Node.js          | Required only for npm/package development                      |
| Linux sandbox    | `bubblewrap` recommended on Linux                              |
| Linux keyring    | A Secret Service provider such as GNOME Keyring is recommended |

On Ubuntu/Debian hosts, install the common runtime helpers:

```bash
sudo apt-get update
sudo apt-get install -y git curl ca-certificates bubblewrap libsecret-1-0
```

`bubblewrap` is used by the Linux sandbox. If it is missing, PFTerminal can use
its bundled fallback, but installing the OS package removes the startup warning.

On macOS, the release installer only needs the system `curl`, `tar`, and shell
tools that ship with macOS. Source builds also need Apple's command line tools:

```bash
xcode-select --install
```

## Install Options

### Release Installer

The standalone installer downloads a release from `agticorp/PfTerminal` and
verifies the release artifact digest. This is the preferred path for normal
users because it avoids a full Rust source build.

```bash
curl -fsSL https://github.com/agticorp/PfTerminal/releases/latest/download/install.sh | sh
```

The release installer creates a `pfterminal` launcher and leaves any existing
stock `codex` command alone. By default that launcher stores PFTerminal state in
`$HOME/.pfterminal`, separate from a stock Codex install. Override the defaults
only when you need a custom install location:

```bash
curl -fsSL https://github.com/agticorp/PfTerminal/releases/latest/download/install.sh |
  PFTERMINAL_INSTALL_DIR="$HOME/.local/bin" \
  PFTERMINAL_HOME="$HOME/.pfterminal" \
  sh
```

The installer requires a published GitHub release. If a fresh clone has no
release yet, use the source build fallback below.

### Release Build For Maintainers

Release artifacts are built by the manual `pfterminal-release` GitHub Actions
workflow. It does not run on every push. Run it only when you want
installer-ready macOS and Linux artifacts for the current Cargo version.

The workflow builds and smoke-tests these package archives:

```text
pfterminal-package-aarch64-apple-darwin.tar.gz
pfterminal-package-x86_64-apple-darwin.tar.gz
pfterminal-package-x86_64-unknown-linux-gnu.tar.gz
pfterminal-package_SHA256SUMS
```

Leave `publish_release` disabled to do a build-only validation. Enable it to
create or update the matching `rust-vX.Y.Z` GitHub release. Enable
`make_latest` only when that release should become the default target for the
installer's `latest` resolution.

### Source Build

```bash
git clone https://github.com/agticorp/PfTerminal.git
cd PfTerminal/codex-rs

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup component add rustfmt clippy

cargo install --locked just
cargo install --locked dotslash
cargo install --locked cargo-nextest

CARGO_NET_GIT_FETCH_WITH_CLI=true cargo build -p codex-cli --bin pfterminal
```

The first source build can take 10-20 minutes on a fresh Mac because Cargo has
to fetch git dependencies and compile the full workspace. The
`CARGO_NET_GIT_FETCH_WITH_CLI=true` setting avoids intermittent macOS libgit2
fetch stalls seen with nested git dependencies.

Run the source-built binary from the workspace you want PFTerminal to inspect:

```bash
export CODEX_HOME="${PFTERMINAL_HOME:-$HOME/.pfterminal}"
cd ~/repos
/path/to/PfTerminal/codex-rs/target/debug/pfterminal
```

For repeated local use, install a wrapper on your `PATH`:

```bash
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/pfterminal/bin"
install -m 0755 /path/to/PfTerminal/codex-rs/target/debug/pfterminal \
  "$HOME/.local/share/pfterminal/bin/pfterminal"
cat > "$HOME/.local/bin/pfterminal" <<'EOF'
#!/bin/sh
export CODEX_HOME="${CODEX_HOME:-${PFTERMINAL_HOME:-$HOME/.pfterminal}}"
exec "$HOME/.local/share/pfterminal/bin/pfterminal" "$@"
EOF
chmod 0755 "$HOME/.local/bin/pfterminal"
```

Using `CODEX_HOME=$HOME/.pfterminal` keeps PFTerminal credentials, vault data,
sessions, logs, plugins, and skills separate from a stock Codex install.

### npm Package

The npm package is `@agticorp/pfterminal` and exposes both `pfterminal` and
`codex` command aliases. The launcher prefers the bundled `pfterminal` binary
and defaults `CODEX_HOME` to `$HOME/.pfterminal`.

```bash
npm install -g @agticorp/pfterminal
pfterminal --version
```

## Provider Setup

PFTerminal ships built-in providers. You do not need to define these providers
manually in `config.toml`; you only need a credential for the provider you plan
to use.

| Provider   | Provider id  | Key name             | Model(s) shown in `/model`                                                              |
| ---------- | ------------ | -------------------- | --------------------------------------------------------------------------------------- |
| Ambient    | `ambient`    | `AMBIENT_API_KEY`    | `zai-org/GLM-5.2-FP8`                                                                   |
| Z.AI       | `zai`        | `ZAI_API_KEY`        | `glm-5.2`                                                                               |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` | `z-ai/glm-5.2`, `minimax/minimax-m3`, `openrouter/owl-alpha`, `google/gemini-3.5-flash` |
| Baseten    | `baseten`    | `BASETEN_API_KEY`    | `zai-org/GLM-5.2`                                                                       |

The first-run provider picker can accept Ambient, Z.AI, OpenRouter, or Baseten
API keys. Provider keys entered through the PFTerminal UI are stored in the
encrypted vault and are available from any working directory.

You can also provide keys through environment variables:

```bash
export AMBIENT_API_KEY="..."
export ZAI_API_KEY="..."
export OPENROUTER_API_KEY="..."
export BASETEN_API_KEY="..."
```

Environment variables are useful for CI and temporary shells. For a normal
desktop/server setup, prefer the UI or `/vault` so the key is encrypted at rest
and listed in the PFTerminal vault.

## Vault Setup

PFTerminal stores provider API keys in the encrypted vault. Provider keys use
stable labels derived from their key names:

| Provider key         | Vault label                   |
| -------------------- | ----------------------------- |
| `AMBIENT_API_KEY`    | `provider/ambient_api_key`    |
| `ZAI_API_KEY`        | `provider/zai_api_key`        |
| `OPENROUTER_API_KEY` | `provider/openrouter_api_key` |
| `BASETEN_API_KEY`    | `provider/baseten_api_key`    |

The vault backend is the Codex managed-secrets substrate:

- encrypted data: `$CODEX_HOME/secrets/local.age`;
- passphrase storage: the OS keyring when available;
- Linux fallback: a local `0600` keyring fallback file only for the vault
  passphrase when no Secret Service is available;
- legacy fallback: `provider_auth.json` is read for old installs and removed
  after a successful vault write.

Inside the TUI:

```text
/vault
```

opens the vault action menu.

Useful commands:

```text
/vault list
/vault show provider/zai_api_key
/vault credential add
/vault credential delete provider/openrouter_api_key
```

Raw secrets must not be typed into chat. `/vault credential add` opens a masked
entry flow so the secret does not enter the conversation transcript or model
context.

## Model Selection

Use `/model` in the TUI or pass `-m` at startup.

```bash
pfterminal -m zai-org/GLM-5.2-FP8      # Ambient GLM 5.2
pfterminal -m glm-5.2                  # Z.AI GLM 5.2
pfterminal -m z-ai/glm-5.2             # OpenRouter GLM 5.2
pfterminal -m minimax/minimax-m3       # OpenRouter MiniMax M3
pfterminal -m openrouter/owl-alpha     # OpenRouter Owl Alpha
pfterminal -m google/gemini-3.5-flash  # OpenRouter Gemini 3.5 Flash
pfterminal -m zai-org/GLM-5.2          # Baseten GLM 5.2
```

The `/model` picker groups models into:

- `Coding Plans`: Ambient and Z.AI plan-backed models.
- `Pay Per API Call`: OpenRouter and Baseten metered models.

Current visible model metadata:

| Model                     | Provider   | Notes                                                                        |
| ------------------------- | ---------- | ---------------------------------------------------------------------------- |
| `zai-org/GLM-5.2-FP8`     | Ambient    | Ambient default GLM 5.2 coding model                                         |
| `glm-5.2`                 | Z.AI       | Z.AI coding-plan GLM 5.2                                                     |
| `zai-org/GLM-5.2`         | Baseten    | GLM 5.2, listed as `$1.50/M input`, `$0.30/M cached input`, `$4.50/M output` |
| `z-ai/glm-5.2`            | OpenRouter | GLM 5.2, listed as `$0.98/M input`, `$3.08/M output`                         |
| `minimax/minimax-m3`      | OpenRouter | MiniMax M3, listed as `$0.30/M input`, `$1.20/M output`                      |
| `openrouter/owl-alpha`    | OpenRouter | Owl Alpha, listed as `$0/M input`, `$0/M output`                             |
| `google/gemini-3.5-flash` | OpenRouter | Gemini 3.5 Flash, listed as `$1.50/M input`, `$9.00/M output`                |

## Basic Verification

After installing and adding a provider key:

```bash
pfterminal --version
pfterminal
```

In the TUI:

```text
/vault
/model
/skills
```

Expected setup signs:

- `/vault` shows the provider key label you added.
- `/model` shows Coding Plans and Pay Per API Call sections.
- `/skills` includes bundled PFTerminal system skills such as Frontend Design.

## Development Commands

From the repository root:

```bash
cd codex-rs
cargo build -p codex-cli --bin pfterminal
just fmt
just test -p codex-tui
```

Avoid `--all-features` for routine local runs because it increases build time
and `target/` disk usage by compiling additional feature combinations.

## Tracing

The TUI records diagnostics in bounded local stores by default. Set `log_dir`
explicitly to enable a plaintext TUI log for a run:

```bash
pfterminal -c log_dir=./.pfterminal-log
tail -F ./.pfterminal-log/codex-tui.log
```

The non-interactive mode defaults to `RUST_LOG=error`, but messages are printed
inline, so there is no separate log file to monitor unless `log_dir` is set.
