# PFTerminal Auth Break Review

## Summary

The shipped auth experience is broken in two separate places:

1. The first-login OpenAI Codex account device-auth option can be displayed as selectable while the handler refuses to start it.
2. Provider API keys saved from `/providers` can be written to storage without updating the running app's auth state, so the same process can continue behaving as unauthenticated until restart.

These are product-boundary failures, not user input problems. The failed boundaries are:

- UI selection state versus UI action handling.
- Credential persistence versus in-process auth/cache invalidation.

## User-Visible Failures

### First-login OpenAI device auth

The first-login screen shows:

```text
Provider: OpenAI Codex Account
```

But selecting it can do nothing when the app is in forced API/provider-picker mode. From the user's perspective, the login option is bricked: the UI presents a valid auth path, but the click/Enter path does not start device auth.

### Provider key save requires restart

The `/providers` flow can display:

```text
Stored Provider: Z.AI API Key in the vault.
```

Then the next model request can still fail with:

```text
Missing environment variable: `ZAI_API_KEY`
```

After quitting and restarting `pfterminal`, the same saved credential works. That means the key was persisted, but the running process did not observe it correctly.

## Root Cause 1: First-Login Device Auth Handler Uses The Wrong Gate

The first-login option list is built in:

```text
codex-rs/tui/src/onboarding/auth.rs
```

When provider picker mode is enabled, `displayed_sign_in_options()` includes `SignInOption::DeviceCode` before the provider API-key options:

```rust
if self.provider_picker_enabled() {
    let mut options = vec![SignInOption::DeviceCode];
    options
        .extend((0..self.api_key_provider_options.len()).map(SignInOption::ProviderApiKey));
    return options;
}
```

The render path labels that same `DeviceCode` option as the OpenAI Codex account login:

```rust
SignInOption::DeviceCode => {
    let (text, description) = if self.provider_picker_enabled() {
        ("Provider: OpenAI Codex Account", "Sign in with device code")
    } else {
        ("Sign in with Device Code", device_code_description)
    };
    lines.extend(create_mode_item(idx, option, text, description));
}
```

But the action handler still uses the old ChatGPT-only gate:

```rust
SignInOption::DeviceCode => {
    if self.is_chatgpt_login_allowed() {
        self.start_device_code_login();
    }
}
```

In forced API/provider-picker mode, `is_chatgpt_login_allowed()` is false:

```rust
fn is_chatgpt_login_allowed(&self) -> bool {
    !matches!(self.forced_login_method, Some(ForcedLoginMethod::Api))
}
```

So the UI displays a selectable device-auth option, but the handler refuses to start it.

### Why the backend did not save this

The app-server has the right operation:

```rust
LoginAccountParams::OpenaiProviderDeviceCode => {
    self.login_chatgpt_device_code_v2(
        request_id,
        /*allow_forced_api_for_provider_login*/ true,
    )
    .await;
}
```

The provider-device-code backend can allow forced API mode. The failure is that the first-login TUI path never calls it because the handler blocks before dispatch.

## Root Cause 2: `/providers` Writes Credentials Outside The App-Server Auth Boundary

The `/providers` API-key flow is in:

```text
codex-rs/tui/src/chatwidget/provider_credentials.rs
```

It currently writes the provider key directly from the TUI:

```rust
match codex_login::login_with_provider_api_key(
    &codex_home,
    &env_key,
    &secret,
    auth_credentials_store_mode,
    keyring_backend_kind,
) {
    Ok(()) => {
        tx.send(AppEvent::InsertHistoryCell(Box::new(
            history_cell::new_info_event(
                format!("Stored {display_name} in the vault."),
                /*hint*/ None,
            ),
        )));
    }
    ...
}
```

That persists the key, but it bypasses the app-server account login path.

The app-server path does more than write storage:

```rust
match login_with_provider_api_key(...) {
    Ok(()) => {
        self.auth_manager.reload().await;
        Ok(())
    }
    ...
}
```

And after a successful app-server provider-key login, it sends login/account notifications:

```rust
if logged_in {
    self.send_login_success_notifications(/*login_id*/ None)
        .await;
}
```

Those notifications include `account/login/completed` and `account/updated`.

The TUI direct-write path skips that entire boundary. It tells the user the key was stored, but it does not make the running auth state consistent.

## Root Cause 3: Provider Auth Can Cache Missing Credentials

The model-provider layer has another in-process cache:

```text
codex-rs/model-provider/src/provider.rs
```

`ConfiguredModelProvider` stores:

```rust
cached_provider_env_auth: Arc<OnceLock<Option<CodexAuth>>>,
```

Provider auth lookup checks the environment first, then uses that `OnceLock` to cache the stored provider key:

```rust
self.cached_provider_env_auth
    .get_or_init(|| {
        self.auth_manager
            .as_ref()
            .and_then(|auth_manager| auth_manager.provider_api_key(provider_key_id).ok())
            .flatten()
            .map(|api_key| CodexAuth::from_api_key(&api_key))
    })
    .clone()
```

This can cache `None`. If the app looked for `ZAI_API_KEY` before the user stored it, the provider object can keep returning no auth even after storage changes.

Restarting works because it creates fresh provider objects and fresh auth caches. That is why the user's restart made the saved key take effect.

## Why This Regressed

The provider-picker/device-auth flow was added across the provider auth cleanup work. The UI option and backend operation were added, but the existing action guard still treated `DeviceCode` as normal ChatGPT login only.

The provider key menu was also separated into a TUI vault-writing path. That made storage succeed on headless Linux, but it crossed around the app-server login boundary. Later provider-key cache invalidation improved the auth manager storage revision behavior, but it did not fix the direct TUI write bypass or the model-provider `OnceLock` cache.

## Required Fixes

### 1. Fix first-login device auth selection

Change the `SignInOption::DeviceCode` handler so provider-picker device auth is allowed.

The handler should use a predicate equivalent to:

```rust
fn is_device_code_login_allowed(&self) -> bool {
    self.provider_picker_enabled() || self.is_chatgpt_login_allowed()
}
```

Then:

```rust
SignInOption::DeviceCode => {
    if self.is_device_code_login_allowed() {
        self.start_device_code_login();
    }
}
```

This makes the visible `Provider: OpenAI Codex Account` option actually dispatch the existing `OpenaiProviderDeviceCode` app-server request.

### 2. Route `/providers` API-key saves through app-server login

Stop calling `codex_login::login_with_provider_api_key()` directly from the TUI.

Instead, the provider key entry should send an app event containing:

- provider id, for example `zai`
- provider display name, for user-facing success/error text
- env key, for labels
- secret API key

The event dispatcher should call:

```rust
ClientRequest::LoginAccount {
    request_id,
    params: LoginAccountParams::ProviderApiKey {
        provider,
        api_key,
    },
}
```

That keeps all credential changes behind the same boundary:

- persist credential
- reload `AuthManager`
- emit `account/login/completed`
- emit `account/updated`
- let running app state observe the change

### 3. Fix provider auth cache invalidation

The model-provider `OnceLock<Option<CodexAuth>>` is unsafe for mutable provider credentials.

Acceptable repairs:

- Remove `cached_provider_env_auth` and query `AuthManager::provider_api_key()` each time provider auth is needed.
- Or replace the `OnceLock` with a cache keyed by the provider credential storage revision.
- Or add a provider/auth refresh path that rebuilds model-provider instances after `account/updated`.

The simplest correct fix is to stop caching missing provider auth with `OnceLock`.

### 4. Add regression tests

Required tests:

- First-login TUI test: in forced API/provider-picker mode, selecting `Provider: OpenAI Codex Account` transitions into pending device-code login instead of doing nothing.
- App-server/TUI boundary test: `/providers` provider API-key entry dispatches `LoginAccountParams::ProviderApiKey`, not direct storage.
- Same-process provider auth test: first request sees missing provider key, key is saved, second request in the same process sees the key without restart.
- Existing app-server provider device-code test should remain: forced API login can still use `OpenaiProviderDeviceCode`.

## Release Requirement

Do not relaunch a release until the fixed build has been tested in-process:

1. Launch a fresh `pfterminal --yolo` with no provider key for the selected provider.
2. Use `/providers` to store the selected provider key.
3. Send a model request without restarting.
4. Confirm the request uses the newly stored credential.
5. Test first-login `Provider: OpenAI Codex Account` from a clean home/profile and confirm device auth begins.

The release should only be cut after those pass.

## Implemented Fixes

The release fix repairs the failed boundaries directly:

- `codex-rs/tui/src/onboarding/auth.rs`: first-login `Provider: OpenAI Codex Account` now uses an explicit provider-picker-aware device-code gate, so the visible row starts the existing app-server device-code login.
- `codex-rs/tui/src/chatwidget/provider_credentials.rs`: provider rows carry the canonical provider id, not only display/env labels.
- `codex-rs/tui/src/app_event.rs`: provider key save events now carry a redacted secret wrapper so API keys are not printed through `Debug`.
- `codex-rs/tui/src/app/event_dispatch.rs`: `/providers` API-key saves now call app-server `account/login/start` with `LoginAccountParams::ProviderApiKey`, keeping persistence, `AuthManager.reload()`, and account update notifications behind one boundary.
- `codex-rs/model-provider/src/provider.rs`: provider API-key auth no longer caches a missing stored credential with `OnceLock<Option<CodexAuth>>`; the provider checks current auth storage on each provider-auth lookup.
- `codex-rs/tui/src/updates.rs`, `codex-rs/tui/src/update_action.rs`, `codex-rs/tui/src/npm_registry.rs`, `codex-rs/cli/src/doctor.rs`, `codex-rs/cli/src/doctor/updates.rs`, and `codex-rs/app-server-daemon/src/update_loop.rs`: update/version/daemon paths now target PFTerminal release/install locations instead of upstream OpenAI Codex.

## Verification Run

Local tests run before release:

```text
just test -p codex-model-provider
Result: 51 passed

just test -p codex-tui provider_key_picker_device_code_selection_starts_login provider_key_picker_shows_codex_account_and_provider_keys provider_rows_dispatch_expected_events standalone_update_commands_rerun_latest_installer ready_version_requires_latest_dist_tag_and_root_dist ready_version_rejects_stale_latest_dist_tag ready_version_rejects_missing_root_dist
Result: 7 passed

just test -p codex-app-server login_account_openai_provider_device_code_works_when_api_login_is_forced login_account_openrouter_provider_key_succeeds_without_configured_provider
Result: 2 passed

just test -p codex-cli update_action_labels_install_contexts compare_npm_package_roots_detects_match compare_npm_package_roots_detects_mismatch
Result: 6 passed

just test -p codex-app-server-daemon
Result: 29 passed

just fix -p codex-model-provider
just fix -p codex-tui
just fix -p codex-cli
just fix -p codex-app-server-daemon
Result: completed
```

Live TUI checks run with local credentials:

```text
First-run onboarding, clean temporary PFTerminal home, no ZAI_API_KEY environment variable:
Selected Z.AI provider key entry, pasted key through masked TUI field, then the same process answered:
PFTTUI_ONBOARDOK

/providers same-process recovery, clean temporary PFTerminal home:
1. Started on Z.AI with no ZAI_API_KEY and no stored Z.AI key.
2. Sent a request and confirmed the expected missing ZAI_API_KEY failure.
3. Opened /providers, selected Z.AI, pasted the key through the masked TUI field.
4. Confirmed "Stored Provider: Z.AI API Key in the vault."
5. Sent another request in the same running process and received:
PFTPROVIDERS_AFTERSAVEOK
```

Device-code auth was regression-tested at the TUI state-machine and app-server boundary. Full live browser/device completion still requires a user-controlled device auth flow.
