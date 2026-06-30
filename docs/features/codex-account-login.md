# Codex Account Login

Status: implemented locally.

## Goal

Reintegrate OpenAI Codex account login into PFTerminal as a first-class
provider credential, next to Ambient, Z.AI, OpenRouter, Baseten, and Vercel
provider keys.

The implemented user outcome is:

- `/providers` shows an OpenAI Codex account login row;
- selecting that row starts device-code login;
- successful login stores Codex/OpenAI account auth through the inherited Codex
  auth path;
- the model picker exposes only `gpt-5.5` for the OpenAI provider;
- logout does not erase provider API keys from the vault unless the user asks
  for a destructive all-credentials logout.

## Current State

The upstream Codex login machinery is still present in PFTerminal:

- CLI login/logout flows live in `codex-rs/cli/src/login.rs`.
- App-server account login supports `Chatgpt`, `ChatgptDeviceCode`, and
  `ChatgptAuthTokens` in `codex-rs/app-server-protocol/src/protocol/v2/account.rs`.
- Server-side account handling already starts browser and device-code login in
  `codex-rs/app-server/src/request_processors/account_processor.rs`.
- The OpenAI provider exists with `requires_openai_auth = true` in
  `codex-rs/model-provider-info/src/lib.rs`.

The PFTerminal-facing integration is now implemented in these surfaces:

- `/providers` includes an OpenAI Codex account device-login row in
  `codex-rs/tui/src/chatwidget/provider_credentials.rs`.
- onboarding shows OpenAI Codex account device login alongside provider keys in
  `codex-rs/tui/src/onboarding/auth.rs`, even when the active API-key provider
  forces API login for generic ChatGPT auth.
- `/model` maps `gpt-*` models to provider `openai`, but only exposes
  `gpt-5.5` in the PFTerminal picker through
  `codex-rs/tui/src/chatwidget/model_popups.rs`.
- `gpt-5.5` is picker-visible in `codex-rs/models-manager/models.json`.
- default logout removes Codex/OpenAI account auth without deleting provider
  vault keys; `pfterminal logout --all` performs destructive cleanup.
- direct `pfterminal` binaries default their state home to `$HOME/.pfterminal`
  when `CODEX_HOME` is unset, keeping account auth separate from stock Codex.

## Decisions

- Treat OpenAI Codex as a provider credential, but not as an API key.
- Prefer device-code login for the `/providers` OpenAI Codex row.
- Keep browser login available where it already exists, but do not make it the
  primary `/providers` path.
- Keep Ambient, Z.AI, OpenRouter, Baseten, and Vercel API-key storage in the
  encrypted provider vault.
- Make default logout preserve provider API keys.
- Add an explicit destructive logout option for users who want to remove all
  auth and provider credentials.
- Expose only `gpt-5.5` for OpenAI in PFTerminal's Coding Plans section.

## Implemented Changes

### Providers Credential Menu

`codex-rs/tui/src/chatwidget/provider_credentials.rs` now uses credential rows
with different actions instead of API-key-only rows.

The `/providers` menu:

```text
Providers
  Add or replace provider credentials. API keys are stored in the vault.

  Search providers
> Provider: OpenAI Codex Account  Sign in with device code
  Provider: Ambient API Key       Store AMBIENT_API_KEY in the vault
  Provider: Z.AI API Key          Store ZAI_API_KEY in the vault
  Provider: OpenRouter API Key    Store OPENROUTER_API_KEY in the vault
  Provider: Baseten API Key       Store BASETEN_API_KEY in the vault
  Provider: Vercel API Key        Store AI_GATEWAY_API_KEY in the vault
```

Implemented details:

- Replaced `ProviderCredentialOption { provider_name, env_key }` with an enum
  representing `CodexAccount` and `ProviderApiKey`.
- Kept `ProviderApiKey` rows dispatching `AppEvent::OpenProviderApiKeyAdd`.
- Added `AppEvent::OpenCodexAccountDeviceLogin` and related ready, failed, and
  cancel events.
- Handled those events in `codex-rs/tui/src/app/event_dispatch.rs`.
- Reused the app-server account endpoint with provider-specific
  `LoginAccountParams::OpenaiProviderDeviceCode`, so the `/providers` OpenAI
  row is not blocked by Ambient/Z.AI/OpenRouter/Baseten/Vercel forced-API settings.
- Showed the verification URL and user code in a bottom-pane view instead of
  inserting the device code into chat history.

### Device-Code UI

The `/providers` path owns a small `CodexAccountDeviceLoginView` bottom-pane
view.

- It starts a `ChatgptDeviceCode` login request through the app server.
- It stores the pending login id in `ChatWidget`.
- It renders the verification URL and one-time user code.
- Esc or Ctrl-C cancels the active login through `CancelLoginAccount`.
- `AccountLoginCompleted` produces success or error history messages only for
  the matching pending login id.

### Onboarding

Onboarding is aligned with `/providers`:

- it no longer suppresses Codex/OpenAI account login when provider-key options
  exist;
- it shows Codex account login as another provider credential choice;
- it uses device-code login for the provider-picker Codex account row; and
- it keeps existing API-key entry for Ambient, Z.AI, OpenRouter, Baseten, and
  Vercel.
The provider-picker Codex account row also uses
`LoginAccountParams::OpenaiProviderDeviceCode`.

This keeps first-run setup and later `/providers` maintenance consistent.

### Logout Semantics

Default logout now removes only Codex/OpenAI account auth and related
first-party auth state.

Implemented behavior:

- `pfterminal logout` revokes/removes Codex/OpenAI account auth.
- `pfterminal logout` does not delete provider API keys from the encrypted
  vault.
- `pfterminal logout` does not delete legacy `provider_auth.json` unless the
  user requests an all-provider cleanup.
- `pfterminal logout --all` removes provider API keys from the vault and legacy
  provider auth storage too.
- app-server `account/logout` uses the non-destructive behavior through
  `AuthManager::logout_with_revoke()`.

`codex-rs/login/src/auth/manager.rs` now separates normal account logout from
all-credential cleanup through `logout_all_credentials()` and
`logout_with_revoke_all_credentials()`.

### Model Picker

OpenAI Codex is layered into the Coding Plans section without exposing the whole
OpenAI catalog.

Implemented details:

- Added an OpenAI allowlist helper in
  `codex-rs/tui/src/chatwidget/model_popups.rs`, initially allowing only
  `gpt-5.5`.
- Included OpenAI `gpt-5.5` in the Coding Plans section.
- Kept older OpenAI/Codex models hidden even if they exist in
  `models-manager/models.json`.
- Kept selecting `gpt-5.5` on the existing
  `UpdateModelSelection` and `PersistModelSelection` paths.
- Made `gpt-5.5` picker-visible in the bundled model catalog while the
  PFTerminal picker allowlist prevents older OpenAI models from showing.

The Coding Plans section copy includes OpenAI Codex alongside Ambient and
Z.AI.

## Verification

Focused automated coverage:

- `/providers` rows include `Provider: OpenAI Codex Account` before API-key
  providers.
- selecting the OpenAI Codex row dispatches the device-code login event.
- selecting provider API-key rows still dispatches masked vault entry events.
- default logout preserves provider vault keys and legacy provider auth.
- destructive logout removes provider vault keys.
- `/model` shows OpenAI `gpt-5.5` in Coding Plans.
- `/model` does not show older OpenAI/Codex models.
- existing model-provider selection tests keep `gpt-5.5` mapped to provider
  `openai`.
- direct `pfterminal` runs default to `.pfterminal` instead of `.codex` when
  `CODEX_HOME` is unset.
- OpenAI provider device-code login starts even when forced API login is active
  because another provider, such as Ambient, is currently selected.

Focused commands:

```bash
cargo test -p codex-tui --lib provider
cargo test -p codex-tui --lib pfterminal_picker_allows_only_gpt_5_5_for_openai
cargo test -p codex-login --lib provider_api_key_login_is_provider_scoped_and_not_primary_auth
cargo test -p codex-login --lib logout
cargo test -p codex-utils-home-dir
cargo check -p codex-tui -p codex-cli -p codex-login
```

Manual smoke test:

```text
1. Start PFTerminal.
2. Run /providers.
3. Select Provider: OpenAI Codex Account.
4. Complete device-code login in the browser.
5. Open /model.
6. Select GPT-5.5 under Coding Plans.
7. Send a small prompt and verify the OpenAI provider uses Codex account auth.
8. Run pfterminal logout and verify Ambient/Z.AI/OpenRouter/Baseten/Vercel keys remain.
```

## Docs

Update:

- `docs/authentication.md` to explain Codex account login and provider-key
  vault separation.
- `docs/install.md` to include OpenAI Codex account in provider setup.
- `docs/config.md` to document `openai` plus `gpt-5.5` as the only exposed
  OpenAI Coding Plan model.
- `docs/integrations/index.md` to list OpenAI Codex account under Coding Plans.

## Acceptance Criteria

- A user can log into their Codex/OpenAI account from `/providers` using device
  auth.
- A user can still add or replace Ambient, Z.AI, OpenRouter, Baseten, and
  Vercel API keys from the same screen.
- `/model` exposes `gpt-5.5` for provider `openai` in Coding Plans.
- No other OpenAI models are exposed in PFTerminal's picker.
- Default logout removes Codex/OpenAI account auth without wiping provider API
  keys.
- Direct `pfterminal` runs use PFTerminal state storage by default, avoiding
  stock Codex auth-state collisions.
- Tests cover the login menu, logout boundary, model allowlist, and provider
  mapping behavior.
