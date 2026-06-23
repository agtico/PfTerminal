//! Provider API-key picker and masked entry flow.

use super::*;

const PROVIDER_CREDENTIALS_VIEW_ID: &str = "provider-credentials";

#[derive(Debug, Clone, Copy)]
struct ProviderCredentialOption {
    provider_name: &'static str,
    env_key: &'static str,
}

const PROVIDER_CREDENTIAL_OPTIONS: &[ProviderCredentialOption] = &[
    ProviderCredentialOption {
        provider_name: "Ambient",
        env_key: "AMBIENT_API_KEY",
    },
    ProviderCredentialOption {
        provider_name: "Z.AI",
        env_key: "ZAI_API_KEY",
    },
    ProviderCredentialOption {
        provider_name: "OpenRouter",
        env_key: "OPENROUTER_API_KEY",
    },
    ProviderCredentialOption {
        provider_name: "Baseten",
        env_key: "BASETEN_API_KEY",
    },
];

impl ChatWidget {
    pub(crate) fn open_provider_credentials_menu(&mut self) {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Providers".bold()));
        header.push(Line::from(
            "Add or replace provider API keys. Keys are stored in the vault.".dim(),
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

    pub(crate) fn open_provider_api_key_add(&mut self, provider_name: String, env_key: String) {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        let auth_credentials_store_mode = self.config.cli_auth_credentials_store_mode;
        let keyring_backend_kind = self.config.auth_keyring_backend_kind();
        let display_name = provider_credential_display_name(&provider_name, &env_key);
        let tx = self.app_event_tx.clone();
        let view = crate::bottom_pane::vault_secret_entry::VaultSecretEntryView::new_fixed_secret(
            provider_vault_label(&env_key),
            format!("Add {display_name}"),
            format!("{env_key} (masked - not shown, not stored in chat)"),
            Box::new(move |_label: String, secret: String| {
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
                    Err(err) => {
                        tx.send(AppEvent::InsertHistoryCell(Box::new(
                            history_cell::new_error_event(format!(
                                "Failed to store {display_name}: {err}"
                            )),
                        )));
                    }
                }
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }
}

fn provider_credential_items() -> Vec<SelectionItem> {
    PROVIDER_CREDENTIAL_OPTIONS
        .iter()
        .map(provider_credential_item)
        .collect()
}

fn provider_credential_item(option: &ProviderCredentialOption) -> SelectionItem {
    let provider_name = option.provider_name.to_string();
    let env_key = option.env_key.to_string();
    SelectionItem {
        name: provider_credential_display_name(option.provider_name, option.env_key),
        description: Some(format!("Store {env_key} in the vault")),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenProviderApiKeyAdd {
                provider_name: provider_name.clone(),
                env_key: env_key.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn provider_credential_display_name(provider_name: &str, env_key: &str) -> String {
    let key_name = match env_key {
        "AMBIENT_API_KEY" => "API Key",
        "ZAI_API_KEY" => "API Key",
        "OPENROUTER_API_KEY" => "API Key",
        "BASETEN_API_KEY" => "API Key",
        _ => env_key,
    };
    format!("Provider: {provider_name} {key_name}")
}

fn provider_vault_label(env_key: &str) -> String {
    format!("provider/{}", env_key.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_provider_rows_are_human_readable() {
        let rows = provider_credential_items();
        let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Provider: Ambient API Key",
                "Provider: Z.AI API Key",
                "Provider: OpenRouter API Key",
                "Provider: Baseten API Key",
            ]
        );
        assert_eq!(
            rows[0].description.as_deref(),
            Some("Store AMBIENT_API_KEY in the vault")
        );
    }

    #[test]
    fn provider_vault_label_matches_provider_key_storage() {
        assert_eq!(
            provider_vault_label("ZAI_API_KEY"),
            "provider/zai_api_key".to_string()
        );
    }
}
