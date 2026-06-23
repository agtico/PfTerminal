# PFTerminal

PFTerminal is a crypto-native AI services terminal based on the open-source
Codex CLI. It keeps Codex's local coding-agent runtime and adds first-class
Ambient, Z.AI GLM 5.2, and Post Fiat product integration surfaces.

This site is the engineering front door. It is not a dump of internal notes. It
points to the current code paths, docs, and packaging surfaces that define what
has been integrated.

## What Exists Now

| Area | Current State | Where To Read |
| --- | --- | --- |
| Ambient | Built-in provider, API-key onboarding, Ambient GLM 5.2 default model, encrypted vault storage, and GLM request shaping. | [Ambient](integrations/ambient.md) |
| Z.AI GLM 5.2 | Built-in Z.AI coding-plan provider, direct `glm-5.2` model selection, and vault-backed provider keys. | [Z.AI GLM 5.2](integrations/zai-glm-52.md) |
| OpenRouter | Built-in metered provider for GLM 5.2, MiniMax M3, Owl Alpha, and Gemini 3.5 Flash. | [OpenRouter](integrations/openrouter.md) |
| Baseten | Built-in metered provider for GLM 5.2 through Baseten. | [Baseten](integrations/baseten.md) |
| Vault | Encrypted credential store for provider keys and manually-added secrets. | [Authentication And Vault](authentication.md) |
| Codex fork | Product command aliases, npm packages, installer names, TUI branding, and model picker behavior. | [Codex Fork](integrations/codex-fork.md) |
| Runtime | Codex-derived local coding agent with tools, approvals, sandboxing, MCP, exec, and review modes. | [Runtime](exec.md) |
| Configuration | Provider and model defaults normalize Ambient/Z.AI sessions onto compatible GLM models. | [Configuration](config.md) |

## Fast Reading Path

1. Read [Install And First Run](install.md) for binary setup, provider keys,
   vault setup, and model selection.
2. Read [Ambient](integrations/ambient.md) and
   [Z.AI GLM 5.2](integrations/zai-glm-52.md) for the coding-plan provider
   integrations.
3. Read [OpenRouter](integrations/openrouter.md) and
   [Baseten](integrations/baseten.md) for metered provider integrations.
4. Read [Codex Fork](integrations/codex-fork.md) for product-specific changes
   from upstream Codex.
5. Read [Configuration](config.md), [Authentication](authentication.md), and
   [Slash Commands](slash_commands.md) for operator-facing behavior.
6. Read [Exec](exec.md), [Sandbox](sandbox.md), and [Skills](skills.md) for the
   inherited runtime surfaces.

## Core Claim

PFTerminal is currently a Codex-derived terminal agent with Ambient, Z.AI,
OpenRouter, Baseten, and encrypted vault storage made first-class. The
implementation changes are code-level provider, model, request-shaping,
onboarding, credential-storage, packaging, and branding changes, not just prompt
text.

## Repository layout

The main implementation is under `codex-rs/`, inherited from the open-source Codex CLI Rust workspace.

Product-facing packaging lives in:

- `codex-cli/` for the npm CLI package.
- `scripts/install/` for standalone installers.
- `sdk/` for SDK surfaces.

User-facing docs live in this `docs/` directory and are built by MkDocs from the repository root:

```bash
mkdocs serve
mkdocs build
```

## Self-Hosted URL

The docs can be built and served as a standalone site:

```bash
python3 -m venv .venv-docs
. .venv-docs/bin/activate
pip install -r requirements-docs.txt
scripts/docs-site-build
scripts/docs-site-serve --host 127.0.0.1 --port 8089
```

On the current shared docs host, the static PFTerminal build is also published
under the existing authenticated L1 docs server:

```text
http://5.223.45.94:8088/pfterminal/
```
