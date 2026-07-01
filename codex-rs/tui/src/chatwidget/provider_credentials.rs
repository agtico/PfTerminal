//! Provider API-key picker and masked entry flow.

use super::*;
use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::ViewCompletion;
use codex_model_provider_info::AMBIENT_API_KEY_ENV_VAR;
use codex_model_provider_info::AMBIENT_PROVIDER_ID;
use codex_model_provider_info::ANTHROPIC_API_KEY_ENV_VAR;
use codex_model_provider_info::ANTHROPIC_PROVIDER_ID;
use codex_model_provider_info::BASETEN_API_KEY_ENV_VAR;
use codex_model_provider_info::BASETEN_PROVIDER_ID;
use codex_model_provider_info::OPENROUTER_API_KEY_ENV_VAR;
use codex_model_provider_info::OPENROUTER_PROVIDER_ID;
use codex_model_provider_info::VERCEL_API_KEY_ENV_VAR;
use codex_model_provider_info::VERCEL_PROVIDER_ID;
use codex_model_provider_info::ZAI_API_KEY_ENV_VAR;
use codex_model_provider_info::ZAI_PROVIDER_ID;

const PROVIDER_CREDENTIALS_VIEW_ID: &str = "provider-credentials";
const CODEX_ACCOUNT_DEVICE_LOGIN_VIEW_ID: &str = "codex-account-device-login";

#[derive(Debug, Clone, Copy)]
enum ProviderCredentialOption {
    CodexAccount,
    ClaudeCodePlan,
    ProviderApiKey {
        provider_id: &'static str,
        provider_name: &'static str,
        env_key: &'static str,
    },
}

const PROVIDER_CREDENTIAL_OPTIONS: &[ProviderCredentialOption] = &[
    ProviderCredentialOption::CodexAccount,
    ProviderCredentialOption::ClaudeCodePlan,
    ProviderCredentialOption::ProviderApiKey {
        provider_id: ANTHROPIC_PROVIDER_ID,
        provider_name: "Anthropic",
        env_key: ANTHROPIC_API_KEY_ENV_VAR,
    },
    ProviderCredentialOption::ProviderApiKey {
        provider_id: AMBIENT_PROVIDER_ID,
        provider_name: "Ambient",
        env_key: AMBIENT_API_KEY_ENV_VAR,
    },
    ProviderCredentialOption::ProviderApiKey {
        provider_id: ZAI_PROVIDER_ID,
        provider_name: "Z.AI",
        env_key: ZAI_API_KEY_ENV_VAR,
    },
    ProviderCredentialOption::ProviderApiKey {
        provider_id: OPENROUTER_PROVIDER_ID,
        provider_name: "OpenRouter",
        env_key: OPENROUTER_API_KEY_ENV_VAR,
    },
    ProviderCredentialOption::ProviderApiKey {
        provider_id: BASETEN_PROVIDER_ID,
        provider_name: "Baseten",
        env_key: BASETEN_API_KEY_ENV_VAR,
    },
    ProviderCredentialOption::ProviderApiKey {
        provider_id: VERCEL_PROVIDER_ID,
        provider_name: "Vercel",
        env_key: VERCEL_API_KEY_ENV_VAR,
    },
];

impl ChatWidget {
    pub(crate) fn open_provider_credentials_menu(&mut self) {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Providers".bold()));
        header.push(Line::from(
            "Add or replace provider credentials. API keys are stored in the vault.".dim(),
        ));

        self.show_selection_view(SelectionViewParams {
            view_id: Some(PROVIDER_CREDENTIALS_VIEW_ID),
            footer_hint: Some(standard_popup_hint_line()),
            is_searchable: true,
            search_placeholder: Some("Search providers".to_string()),
            items: provider_credential_items(),
            header: Box::new(header),
            ..Default::default()
        });
    }

    pub(crate) fn open_provider_api_key_add(
        &mut self,
        provider_id: String,
        provider_name: String,
        env_key: String,
    ) {
        let display_name = provider_credential_display_name(&provider_name, &env_key);
        let tx = self.app_event_tx.clone();
        let view = crate::bottom_pane::vault_secret_entry::VaultSecretEntryView::new_fixed_secret(
            provider_vault_label(&env_key),
            format!("Add {display_name}"),
            format!("{env_key} (masked - not shown, not stored in chat)"),
            Box::new(move |_label: String, secret: String| {
                tx.send(AppEvent::SaveProviderApiKey {
                    provider_id,
                    display_name,
                    api_key: crate::app_event::ProviderApiKeySecret::new(secret),
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn open_codex_account_device_login_pending(&mut self) {
        self.pending_provider_codex_login_id = None;
        self.bottom_pane
            .show_view(Box::new(CodexAccountDeviceLoginView::pending(
                self.app_event_tx.clone(),
            )));
    }

    pub(crate) fn open_codex_account_device_login_ready(
        &mut self,
        login_id: String,
        verification_url: String,
        user_code: String,
    ) {
        self.pending_provider_codex_login_id = Some(login_id.clone());
        self.bottom_pane.replace_active_view_by_id(
            CODEX_ACCOUNT_DEVICE_LOGIN_VIEW_ID,
            Box::new(CodexAccountDeviceLoginView::ready(
                self.app_event_tx.clone(),
                login_id,
                verification_url,
                user_code,
            )),
        );
    }

    pub(crate) fn on_codex_account_device_login_failed(&mut self, message: String) {
        self.pending_provider_codex_login_id = None;
        self.bottom_pane
            .dismiss_view_by_id(CODEX_ACCOUNT_DEVICE_LOGIN_VIEW_ID);
        self.add_error_message(format!("OpenAI Codex account login failed: {message}"));
    }

    pub(crate) fn on_codex_account_login_completed(
        &mut self,
        notification: codex_app_server_protocol::AccountLoginCompletedNotification,
    ) {
        let Some(login_id) = notification.login_id else {
            return;
        };
        if self.pending_provider_codex_login_id.as_deref() != Some(login_id.as_str()) {
            return;
        }
        self.pending_provider_codex_login_id = None;
        self.bottom_pane
            .dismiss_view_by_id(CODEX_ACCOUNT_DEVICE_LOGIN_VIEW_ID);

        if notification.success {
            self.add_info_message(
                "OpenAI Codex account login complete.".to_string(),
                /*hint*/ None,
            );
        } else {
            let message = notification
                .error
                .unwrap_or_else(|| "OpenAI Codex account login did not complete.".to_string());
            self.add_error_message(message);
        }
    }
}

fn provider_credential_items() -> Vec<SelectionItem> {
    PROVIDER_CREDENTIAL_OPTIONS
        .iter()
        .map(provider_credential_item)
        .collect()
}

fn provider_credential_item(option: &ProviderCredentialOption) -> SelectionItem {
    match option {
        ProviderCredentialOption::CodexAccount => SelectionItem {
            name: "Provider: OpenAI Codex Account".to_string(),
            description: Some("Sign in with device code".to_string()),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::OpenCodexAccountDeviceLogin);
            })],
            dismiss_on_select: true,
            ..Default::default()
        },
        ProviderCredentialOption::ClaudeCodePlan => SelectionItem {
            name: "Provider: Claude Code Plan".to_string(),
            description: Some("Uses Claude Code OAuth/subscription auth".to_string()),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::InsertHistoryCell(Box::new(
                    history_cell::new_info_event(
                        "Claude Code plan auth is managed by Claude Code. Run `claude auth` or `claude /login`, then choose `claude-opus-4-8-plan` or `claude-fable-5-plan` from /model to use a Claude subscription model through the Codex harness.".to_string(),
                        /*hint*/ None,
                    ),
                )));
            })],
            dismiss_on_select: true,
            ..Default::default()
        },
        ProviderCredentialOption::ProviderApiKey {
            provider_id,
            provider_name,
            env_key,
        } => {
            let provider_id = provider_id.to_string();
            let provider_name = provider_name.to_string();
            let env_key = env_key.to_string();
            SelectionItem {
                name: provider_credential_display_name(&provider_name, &env_key),
                description: Some(format!("Store {env_key} in the vault")),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenProviderApiKeyAdd {
                        provider_id: provider_id.clone(),
                        provider_name: provider_name.clone(),
                        env_key: env_key.clone(),
                    });
                })],
                dismiss_on_select: true,
                ..Default::default()
            }
        }
    }
}

fn provider_credential_display_name(provider_name: &str, env_key: &str) -> String {
    let key_name = match env_key {
        "ANTHROPIC_API_KEY" => "API Key",
        "AMBIENT_API_KEY" => "API Key",
        "ZAI_API_KEY" => "API Key",
        "OPENROUTER_API_KEY" => "API Key",
        "BASETEN_API_KEY" => "API Key",
        "AI_GATEWAY_API_KEY" => "API Key",
        _ => env_key,
    };
    format!("Provider: {provider_name} {key_name}")
}

fn provider_vault_label(env_key: &str) -> String {
    format!("provider/{}", env_key.to_ascii_lowercase())
}

struct CodexAccountDeviceLoginView {
    app_event_tx: AppEventSender,
    login_id: Option<String>,
    verification_url: Option<String>,
    user_code: Option<String>,
    complete: bool,
    completion: Option<ViewCompletion>,
}

impl CodexAccountDeviceLoginView {
    fn pending(app_event_tx: AppEventSender) -> Self {
        Self {
            app_event_tx,
            login_id: None,
            verification_url: None,
            user_code: None,
            complete: false,
            completion: None,
        }
    }

    fn ready(
        app_event_tx: AppEventSender,
        login_id: String,
        verification_url: String,
        user_code: String,
    ) -> Self {
        Self {
            app_event_tx,
            login_id: Some(login_id),
            verification_url: Some(verification_url),
            user_code: Some(user_code),
            complete: false,
            completion: None,
        }
    }

    fn cancel(&mut self) {
        if self.complete {
            return;
        }
        if let Some(login_id) = self.login_id.take() {
            self.app_event_tx
                .send(AppEvent::CancelCodexAccountDeviceLogin { login_id });
        }
        self.complete = true;
        self.completion = Some(ViewCompletion::Cancelled);
    }

    fn accept(&mut self) {
        if self.complete {
            return;
        }
        self.login_id = None;
        self.complete = true;
        self.completion = Some(ViewCompletion::Accepted);
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from("OpenAI Codex Account".bold()), Line::from("")];
        if let (Some(verification_url), Some(user_code)) = (&self.verification_url, &self.user_code)
        {
            lines.push(Line::from("1. Open this link in your browser and sign in"));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                verification_url.clone().cyan().underlined(),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from("2. Enter this one-time code"));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![user_code.clone().cyan().bold()]));
            lines.push(Line::from(""));
            lines.push(
                Line::from("Device codes are a common phishing target. Never share this code.")
                    .dim(),
            );
            lines.push(Line::from(""));
            lines.push(Line::from("Press Esc to cancel").dim());
        } else {
            lines.push(Line::from("Requesting a one-time device code...").dim());
            lines.push(Line::from(""));
            lines.push(Line::from("Press Esc to cancel").dim());
        }
        lines
    }
}

impl BottomPaneView for CodexAccountDeviceLoginView {
    fn handle_key_event(&mut self, key_event: crossterm::event::KeyEvent) {
        match key_event.code {
            KeyCode::Esc => self.cancel(),
            KeyCode::Enter => self.accept(),
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn completion(&self) -> Option<ViewCompletion> {
        self.completion
    }

    fn view_id(&self) -> Option<&'static str> {
        Some(CODEX_ACCOUNT_DEVICE_LOGIN_VIEW_ID)
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.cancel();
        CancellationEvent::Handled
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        true
    }
}

impl Renderable for CodexAccountDeviceLoginView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.lines())
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        if self.verification_url.is_some() && self.user_code.is_some() {
            13
        } else {
            4
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event_sender::AppEventSender;

    #[test]
    fn recommended_provider_rows_are_human_readable() {
        let rows = provider_credential_items();
        let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Provider: OpenAI Codex Account",
                "Provider: Claude Code Plan",
                "Provider: Anthropic API Key",
                "Provider: Ambient API Key",
                "Provider: Z.AI API Key",
                "Provider: OpenRouter API Key",
                "Provider: Baseten API Key",
                "Provider: Vercel API Key",
            ]
        );
        assert_eq!(
            rows[0].description.as_deref(),
            Some("Sign in with device code")
        );
        assert_eq!(
            rows[1].description.as_deref(),
            Some("Uses Claude Code OAuth/subscription auth")
        );
        assert_eq!(
            rows[2].description.as_deref(),
            Some("Store ANTHROPIC_API_KEY in the vault")
        );
        assert_eq!(
            rows[3].description.as_deref(),
            Some("Store AMBIENT_API_KEY in the vault")
        );
    }

    #[test]
    fn provider_rows_dispatch_expected_events() {
        let rows = provider_credential_items();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = AppEventSender::new(tx);

        (rows[0].actions[0])(&sender);
        assert!(matches!(
            rx.try_recv(),
            Ok(AppEvent::OpenCodexAccountDeviceLogin)
        ));

        (rows[1].actions[0])(&sender);
        assert!(matches!(rx.try_recv(), Ok(AppEvent::InsertHistoryCell(_))));

        (rows[2].actions[0])(&sender);
        assert!(matches!(
            rx.try_recv(),
            Ok(AppEvent::OpenProviderApiKeyAdd { provider_id, provider_name, env_key })
                if provider_id == ANTHROPIC_PROVIDER_ID
                    && provider_name == "Anthropic"
                    && env_key == ANTHROPIC_API_KEY_ENV_VAR
        ));

        (rows[3].actions[0])(&sender);
        assert!(matches!(
            rx.try_recv(),
            Ok(AppEvent::OpenProviderApiKeyAdd { provider_id, provider_name, env_key })
                if provider_id == AMBIENT_PROVIDER_ID
                    && provider_name == "Ambient"
                    && env_key == AMBIENT_API_KEY_ENV_VAR
        ));
    }

    #[test]
    fn codex_account_device_login_escape_cancels_only_active_login() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = AppEventSender::new(tx);
        let esc = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );

        let mut pending = CodexAccountDeviceLoginView::pending(sender.clone());
        pending.handle_key_event(esc);
        assert!(pending.is_complete());
        assert!(matches!(
            pending.completion(),
            Some(ViewCompletion::Cancelled)
        ));
        assert!(rx.try_recv().is_err());

        let mut ready = CodexAccountDeviceLoginView::ready(
            sender,
            "login-1".to_string(),
            "https://example.com/device".to_string(),
            "ABCD-EFGH".to_string(),
        );
        ready.handle_key_event(esc);
        assert!(matches!(
            rx.try_recv(),
            Ok(AppEvent::CancelCodexAccountDeviceLogin { login_id }) if login_id == "login-1"
        ));

        ready.handle_key_event(esc);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn codex_account_device_login_accept_disarms_cancel() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = AppEventSender::new(tx);
        let mut view = CodexAccountDeviceLoginView::ready(
            sender,
            "login-1".to_string(),
            "https://example.com/device".to_string(),
            "ABCD-EFGH".to_string(),
        );

        view.handle_key_event(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(view.completion(), Some(ViewCompletion::Accepted)));

        view.handle_key_event(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn provider_vault_label_matches_provider_key_storage() {
        assert_eq!(
            provider_vault_label("ZAI_API_KEY"),
            "provider/zai_api_key".to_string()
        );
    }
}
