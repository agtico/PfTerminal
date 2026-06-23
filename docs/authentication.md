# Authentication And Vault

PFTerminal has two authentication surfaces:

1. provider API keys for model access; and
2. the encrypted `/vault` credential store for provider keys and other
   user-managed secrets.

Provider keys entered through PFTerminal onboarding are written to the vault.
The inherited OpenAI/ChatGPT auth path still exists for upstream Codex
compatibility, but PFTerminal's default provider setup is API-key based.

## Provider Keys

Built-in providers use these key names:

| Provider   | Provider id  | Key name             | Vault label                   |
| ---------- | ------------ | -------------------- | ----------------------------- |
| Ambient    | `ambient`    | `AMBIENT_API_KEY`    | `provider/ambient_api_key`    |
| Z.AI       | `zai`        | `ZAI_API_KEY`        | `provider/zai_api_key`        |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` | `provider/openrouter_api_key` |
| Baseten    | `baseten`    | `BASETEN_API_KEY`    | `provider/baseten_api_key`    |

Provider key resolution checks the encrypted vault first. Legacy
`provider_auth.json` is still read for migration compatibility, and a successful
vault write removes the migrated plaintext key when possible.

Environment variables are still supported for temporary shells and automation:

```bash
export AMBIENT_API_KEY="..."
export ZAI_API_KEY="..."
export OPENROUTER_API_KEY="..."
export BASETEN_API_KEY="..."
```

For normal interactive use, store keys through onboarding or `/vault` so they
are encrypted at rest.

## Vault Storage

The vault is backed by the Codex managed-secrets store:

- encrypted file: `$CODEX_HOME/secrets/local.age`;
- passphrase storage: OS keyring when available;
- fallback: local `0600` keyring fallback file only for the vault passphrase on
  keyring-less hosts;
- metadata: labels, types, providers, and timestamps are listable without
  revealing raw secrets.

The vault is global to the PFTerminal home directory, so stored credentials are
available from any working directory that uses the same `CODEX_HOME`.

## Using `/vault`

Open the vault action menu:

```text
/vault
```

Useful commands:

```text
/vault list
/vault show provider/zai_api_key
/vault credential add
/vault credential delete provider/openrouter_api_key
```

`/vault credential add` opens a masked entry view. Do not type raw secrets as
chat text. The secure entry path keeps secrets out of prompt history, transcript
history, and model context.

`/vault show <label>` displays metadata only. Raw reveal/export is intentionally
handled through secure UI, not chat output.

## OpenAI/ChatGPT Compatibility

PFTerminal still includes inherited Codex login modes:

```bash
pfterminal login
pfterminal login --with-api-key
pfterminal login status
pfterminal logout
```

Those commands are primarily for OpenAI/ChatGPT-compatible flows. For Ambient,
Z.AI, OpenRouter, and Baseten, use the provider onboarding picker, `/vault`, or
the provider env vars above.
