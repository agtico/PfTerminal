//! Vault action menu and secret-copy helpers.

use super::*;

const VAULT_MENU_VIEW_ID: &str = "vault-menu";
const VAULT_CREDENTIALS_VIEW_ID: &str = "vault-credentials";
const VAULT_CREDENTIAL_ACTIONS_VIEW_ID: &str = "vault-credential-actions";

impl ChatWidget {
    pub(crate) fn open_vault_menu(&mut self) {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        let credential_result = sorted_vault_credentials(&codex_home);
        let credential_count = credential_result.as_ref().ok().map(Vec::len);

        self.show_selection_view(SelectionViewParams {
            view_id: Some(VAULT_MENU_VIEW_ID),
            footer_hint: Some(standard_popup_hint_line()),
            is_searchable: true,
            search_placeholder: Some("Search vault actions".to_string()),
            items: vault_action_items(codex_home, credential_result),
            header: vault_header(credential_count),
            ..Default::default()
        });
    }

    pub(crate) fn open_vault_credentials_list(&mut self) {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        let credential_result = sorted_vault_credentials(&codex_home);
        let credential_count = credential_result.as_ref().ok().map(Vec::len);

        self.show_selection_view(SelectionViewParams {
            view_id: Some(VAULT_CREDENTIALS_VIEW_ID),
            footer_hint: Some(standard_popup_hint_line()),
            is_searchable: true,
            search_placeholder: Some("Search credentials".to_string()),
            items: vault_credential_items(credential_result),
            header: vault_credentials_header(credential_count),
            ..Default::default()
        });
    }

    pub(crate) fn open_vault_credential_actions(&mut self, label: String) {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        let display_name = credential_display_name_for_label(&label);
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Vault credential".bold()));
        header.push(Line::from(display_name.cyan()));
        header.push(Line::from(
            "Choose an action. Secrets are never printed to chat.".dim(),
        ));

        self.show_selection_view(SelectionViewParams {
            view_id: Some(VAULT_CREDENTIAL_ACTIONS_VIEW_ID),
            footer_hint: Some(standard_popup_hint_line()),
            is_searchable: false,
            items: vault_credential_action_items(codex_home, label),
            header: Box::new(header),
            ..Default::default()
        });
    }

    pub(crate) fn copy_vault_secret_to_clipboard(&mut self, label: String) {
        self.copy_vault_secret_to_clipboard_with(label, crate::clipboard_copy::copy_to_clipboard);
    }

    fn copy_vault_secret_to_clipboard_with(
        &mut self,
        label: String,
        copy_fn: impl FnOnce(&str) -> Result<Option<crate::clipboard_copy::ClipboardLease>, String>,
    ) {
        let vault = codex_vault::Vault::new(self.config.codex_home.as_path().to_path_buf());
        match vault.reveal(&label) {
            Ok(secret) => match copy_fn(&secret) {
                Ok(lease) => {
                    self.clipboard_lease = lease;
                    self.add_info_message(
                        format!("Copied vault credential {label:?} to clipboard."),
                        /*hint*/ None,
                    );
                }
                Err(err) => {
                    self.add_error_message(format!(
                        "Failed to copy vault credential {label:?}: {err}"
                    ));
                }
            },
            Err(err) => {
                self.add_error_message(format!("Failed to read vault credential {label:?}: {err}"));
            }
        }
    }
}

fn sorted_vault_credentials(
    codex_home: &Path,
) -> Result<Vec<codex_vault::VaultCredentialMeta>, codex_vault::VaultError> {
    let mut credentials = codex_vault::Vault::new(codex_home.to_path_buf()).list()?;
    credentials.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(credentials)
}

fn vault_header(credential_count: Option<usize>) -> Box<dyn Renderable> {
    let mut header = ColumnRenderable::new();
    header.push(Line::from("Vault".bold()));
    header.push(Line::from(
        "Add credentials, inspect metadata, or copy secrets without sending them to chat.".dim(),
    ));
    if let Some(count) = credential_count {
        header.push(Line::from(format!("{count} credential(s) stored").dim()));
    }
    Box::new(header)
}

fn vault_credentials_header(credential_count: Option<usize>) -> Box<dyn Renderable> {
    let mut header = ColumnRenderable::new();
    header.push(Line::from("View credentials".bold()));
    header.push(Line::from("Select a credential to inspect or copy.".dim()));
    if let Some(count) = credential_count {
        header.push(Line::from(format!("{count} credential(s) stored").dim()));
    }
    Box::new(header)
}

fn vault_action_items(
    codex_home: PathBuf,
    credential_result: Result<Vec<codex_vault::VaultCredentialMeta>, codex_vault::VaultError>,
) -> Vec<SelectionItem> {
    let view_description = match credential_result {
        Ok(credentials) if credentials.is_empty() => "No credentials stored yet".to_string(),
        Ok(credentials) => format!("View {} stored credential(s)", credentials.len()),
        Err(err) => format!("Credential list unavailable: {err}"),
    };
    vec![
        SelectionItem {
            name: "Add credential".to_string(),
            description: Some("Open masked label and secret entry".to_string()),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::OpenVaultCredentialAdd);
            })],
            dismiss_on_select: true,
            ..Default::default()
        },
        SelectionItem {
            name: "View credentials".to_string(),
            description: Some(view_description),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::OpenVaultCredentialsList);
            })],
            dismiss_on_select: false,
            ..Default::default()
        },
        vault_history_item(
            "Vault status",
            "Show lock state, backend, and credential count",
            codex_home,
            "status".to_string(),
        ),
    ]
}

fn vault_credential_items(
    credential_result: Result<Vec<codex_vault::VaultCredentialMeta>, codex_vault::VaultError>,
) -> Vec<SelectionItem> {
    match credential_result {
        Ok(credentials) if credentials.is_empty() => vec![SelectionItem {
            name: "No credentials stored".to_string(),
            description: Some("Use Add credential from the vault menu.".to_string()),
            is_disabled: true,
            dismiss_on_select: false,
            ..Default::default()
        }],
        Ok(credentials) => credentials.into_iter().map(vault_credential_item).collect(),
        Err(err) => vec![SelectionItem {
            name: "Credential list unavailable".to_string(),
            description: Some(err.to_string()),
            is_disabled: true,
            dismiss_on_select: false,
            ..Default::default()
        }],
    }
}

fn vault_credential_item(credential: codex_vault::VaultCredentialMeta) -> SelectionItem {
    let label = credential.label;
    let name = credential_display_name(&label, credential.provider.as_deref());
    let description = match credential.provider {
        Some(provider) => format!("Stored as {provider}; vault label {label}"),
        None => credential.credential_type.description().to_string(),
    };
    SelectionItem {
        name,
        description: Some(description),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenVaultCredentialActions {
                label: label.clone(),
            });
        })],
        dismiss_on_select: false,
        ..Default::default()
    }
}

fn credential_display_name_for_label(label: &str) -> String {
    credential_display_name(label, None)
}

fn credential_display_name(label: &str, provider: Option<&str>) -> String {
    let key_id = provider
        .or_else(|| label.strip_prefix("provider/"))
        .unwrap_or(label);
    match key_id.to_ascii_uppercase().as_str() {
        "AMBIENT_API_KEY" => "Provider: Ambient API Key".to_string(),
        "ZAI_API_KEY" => "Provider: Z.AI API Key".to_string(),
        "OPENROUTER_API_KEY" => "Provider: OpenRouter API Key".to_string(),
        "BASETEN_API_KEY" => "Provider: Baseten API Key".to_string(),
        _ if label.starts_with("provider/") => format!("Provider: {key_id}"),
        _ => label.to_string(),
    }
}

fn vault_credential_action_items(codex_home: PathBuf, label: String) -> Vec<SelectionItem> {
    vec![
        vault_history_item(
            "Show metadata",
            "Inspect metadata only; secret remains hidden",
            codex_home,
            format!("show {label}"),
        ),
        SelectionItem {
            name: "Copy secret".to_string(),
            description: Some(
                "Copy raw secret to clipboard; it is not printed to chat".to_string(),
            ),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenVaultCopySecret {
                    label: label.clone(),
                });
            })],
            dismiss_on_select: true,
            ..Default::default()
        },
    ]
}

fn vault_history_item(
    name: impl Into<String>,
    description: impl Into<String>,
    codex_home: PathBuf,
    args: String,
) -> SelectionItem {
    SelectionItem {
        name: name.into(),
        description: Some(description.into()),
        actions: vec![Box::new(move |tx| {
            let lines = crate::vault_command::handle_vault_command(&codex_home, &args);
            tx.send(AppEvent::InsertHistoryCell(Box::new(
                PlainHistoryCell::new(lines),
            )));
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_vault::CredentialType;
    use codex_vault::StorageBackend;
    use codex_vault::VaultCredentialMeta;

    #[test]
    fn vault_menu_view_id_is_stable() {
        assert_eq!(VAULT_MENU_VIEW_ID, "vault-menu");
    }

    #[test]
    fn top_level_vault_actions_do_not_include_per_credential_actions() {
        let items = vault_action_items(
            PathBuf::from("/tmp/codex-home"),
            Ok(vec![VaultCredentialMeta {
                label: "provider/ambient_api_key".to_string(),
                credential_type: CredentialType::ApiKey,
                provider: Some("AMBIENT_API_KEY".to_string()),
                notes: None,
                revocation_notes: None,
                created_at: 1,
                updated_at: 1,
                storage_backend: StorageBackend::EncryptedSecrets,
            }]),
        );
        let names = items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec!["Add credential", "View credentials", "Vault status"]
        );
        assert!(!names.iter().any(|name| name.starts_with("Copy secret")));
        assert!(!names.iter().any(|name| name.starts_with("Show provider/")));
    }

    #[test]
    fn credential_tab_shows_one_row_per_credential() {
        let items = vault_credential_items(Ok(vec![VaultCredentialMeta {
            label: "provider/ambient_api_key".to_string(),
            credential_type: CredentialType::ApiKey,
            provider: Some("AMBIENT_API_KEY".to_string()),
            notes: None,
            revocation_notes: None,
            created_at: 1,
            updated_at: 1,
            storage_backend: StorageBackend::EncryptedSecrets,
        }]));

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Provider: Ambient API Key");
        assert!(
            items[0]
                .description
                .as_deref()
                .is_some_and(|description| description.contains("AMBIENT_API_KEY"))
        );
    }

    #[test]
    fn provider_credentials_render_as_human_provider_names() {
        assert_eq!(
            credential_display_name("provider/ambient_api_key", None),
            "Provider: Ambient API Key"
        );
        assert_eq!(
            credential_display_name("provider/zai_api_key", None),
            "Provider: Z.AI API Key"
        );
        assert_eq!(
            credential_display_name("provider/openrouter_api_key", None),
            "Provider: OpenRouter API Key"
        );
        assert_eq!(
            credential_display_name("provider/baseten_api_key", None),
            "Provider: Baseten API Key"
        );
    }

    #[test]
    fn credential_actions_are_scoped_to_selected_credential() {
        let items = vault_credential_action_items(
            PathBuf::from("/tmp/codex-home"),
            "provider/zai_api_key".to_string(),
        );
        let names = items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["Show metadata", "Copy secret"]);
    }
}
