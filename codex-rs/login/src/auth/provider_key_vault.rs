//! Vault-backed resolver/writer for provider API keys (Ambient, Z.AI, OpenRouter, etc.).
//!
//! This is the migration-compatible bridge between the legacy plaintext `provider_auth.json`
//! and the new encrypted [`codex_vault::Vault`] substrate. New provider keys are written to the
//! vault (encrypted at rest via the age-encrypted secrets store keyed by the configured keyring).
//! Reads check the vault first and fall back to `provider_auth.json` so existing installations
//! keep working until the key is re-saved.
//!
//! Provider keys are stored as vault credentials labeled `provider/<provider_key_id>`, where
//! `provider_key_id` is the provider's env-key name (for example `AMBIENT_API_KEY`).
//!
//! # Fallback policy
//!
//! The production vault uses the OS keyring when available. On headless Linux hosts without a
//! FreeDesktop Secret Service, the secrets layer stores only the vault encryption passphrase in a
//! local `0600` keyring-fallback file so provider keys can still live in the encrypted vault and
//! appear under `/vault`. Legacy plaintext `provider_auth.json` is used only if the vault itself
//! fails for some other reason. Those read/write fallbacks emit `tracing::warn!` so they are never
//! silent, and the legacy file retains its `0600` permissions.

use std::path::Path;
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use codex_keyring_store::KeyringStore;
use codex_vault::AddCredential;
use codex_vault::CredentialType;
use codex_vault::Vault;
use codex_vault::VaultError;

/// Prefix used for provider-key vault labels so they are namespaced apart from user-added
/// credentials.
pub(crate) const PROVIDER_LABEL_PREFIX: &str = "provider/";

/// Resolve a provider API key, preferring the encrypted vault over the legacy plaintext store.
///
/// Returns `Ok(None)` when the key is absent from both stores.
pub(crate) fn read_provider_key(
    codex_home: &Path,
    provider_key_id: &str,
) -> std::io::Result<Option<String>> {
    let vault = Vault::new(codex_home.to_path_buf());
    read_provider_key_with_vault(codex_home, provider_key_id, vault)
}

#[cfg(test)]
fn read_provider_key_with_store(
    codex_home: &Path,
    provider_key_id: &str,
    keyring_store: Arc<dyn KeyringStore>,
) -> std::io::Result<Option<String>> {
    let vault = Vault::new_with_keyring_store(codex_home.to_path_buf(), keyring_store);
    read_provider_key_with_vault(codex_home, provider_key_id, vault)
}

fn read_provider_key_with_vault(
    codex_home: &Path,
    provider_key_id: &str,
    vault: Vault,
) -> std::io::Result<Option<String>> {
    let label = provider_label(provider_key_id);
    match vault.reveal(&label) {
        Ok(value) if !value.trim().is_empty() => return Ok(Some(value)),
        Ok(_) => {} // empty value: fall through to legacy store
        Err(VaultError::NotFound { .. }) => {
            tracing::debug!(
                provider_key_id,
                "provider key absent from encrypted vault; checking legacy plaintext provider_auth.json"
            );
        }
        Err(err) => {
            // Vault unavailable (e.g. keyring missing) or decrypt failure: fall back to legacy.
            tracing::warn!(
                ?err,
                "provider key vault read unavailable; reading legacy plaintext provider_auth.json (keyring-less degradation mode)"
            );
        }
    }
    super::manager::legacy_provider_key(codex_home, provider_key_id)
}

/// Write a provider API key to the encrypted vault.
///
/// Falls back to the legacy plaintext store if the vault/keyring is unavailable so callers keep
/// working in keyring-less environments.
pub(crate) fn write_provider_key(
    codex_home: &Path,
    provider_key_id: &str,
    api_key: &str,
) -> std::io::Result<()> {
    let vault = Vault::new(codex_home.to_path_buf());
    write_provider_key_with_vault(codex_home, provider_key_id, api_key, vault)
}

#[cfg(test)]
fn write_provider_key_with_store(
    codex_home: &Path,
    provider_key_id: &str,
    api_key: &str,
    keyring_store: Arc<dyn KeyringStore>,
) -> std::io::Result<()> {
    let vault = Vault::new_with_keyring_store(codex_home.to_path_buf(), keyring_store);
    write_provider_key_with_vault(codex_home, provider_key_id, api_key, vault)
}

fn write_provider_key_with_vault(
    codex_home: &Path,
    provider_key_id: &str,
    api_key: &str,
    vault: Vault,
) -> std::io::Result<()> {
    let label = provider_label(provider_key_id);
    let entry = AddCredential {
        label: label.clone(),
        credential_type: CredentialType::ApiKey,
        provider: Some(provider_key_id.to_string()),
        notes: Some(format!("provider API key for {provider_key_id}")),
        revocation_notes: None,
        secret: api_key.to_string(),
    };

    // Overwrite if a credential for this provider already exists (key rotation); otherwise add.
    let stored = match vault.update(&label, Some(api_key.to_string()), None, None, None) {
        Ok(_) => true,
        Err(codex_vault::VaultError::NotFound { .. }) => match vault.add(entry) {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "provider key vault write failed; storing provider key in legacy plaintext provider_auth.json (keyring-less degradation mode)"
                );
                return super::manager::legacy_save_provider_key(
                    codex_home,
                    provider_key_id,
                    api_key,
                );
            }
        },
        Err(err) => {
            tracing::warn!(
                ?err,
                "provider key vault update failed; storing provider key in legacy plaintext provider_auth.json (keyring-less degradation mode)"
            );
            return super::manager::legacy_save_provider_key(codex_home, provider_key_id, api_key);
        }
    };

    if stored {
        // Migrate the old plaintext key away now that the encrypted copy is durable. Best-effort:
        // a failure here does not undo the successful vault write; it just leaves a stale copy that
        // a future logout or re-save will clean up.
        match super::manager::legacy_delete_provider_key(codex_home, provider_key_id) {
            Ok(true) => {
                tracing::info!(
                    provider_key_id,
                    "migrated provider key off legacy plaintext storage into the encrypted vault"
                );
            }
            Ok(false) => {}
            Err(err) => tracing::warn!(
                ?err,
                provider_key_id,
                "vault write succeeded but failed to remove the legacy plaintext provider key"
            ),
        }
    }
    Ok(())
}

/// Remove all provider keys from the vault (used during logout).
///
/// Returns `true` if any vault provider credential was removed. Best-effort: a vault failure
/// (for example an unavailable keyring) simply returns `Ok(false)`.
pub(crate) fn delete_all_provider_keys(codex_home: &Path) -> std::io::Result<bool> {
    let vault = Vault::new(codex_home.to_path_buf());
    delete_all_provider_keys_with_vault(vault)
}

#[cfg(test)]
fn delete_all_provider_keys_with_store(
    codex_home: &Path,
    keyring_store: Arc<dyn KeyringStore>,
) -> std::io::Result<bool> {
    let vault = Vault::new_with_keyring_store(codex_home.to_path_buf(), keyring_store);
    delete_all_provider_keys_with_vault(vault)
}

fn delete_all_provider_keys_with_vault(vault: Vault) -> std::io::Result<bool> {
    let listing = match vault.list() {
        Ok(listing) => listing,
        Err(err) => {
            tracing::debug!(?err, "provider key vault list unavailable during logout");
            return Ok(false);
        }
    };
    let mut removed_any = false;
    for meta in listing {
        if meta.label.starts_with(PROVIDER_LABEL_PREFIX) {
            match vault.delete(&meta.label) {
                Ok(removed) => removed_any |= removed,
                Err(err) => {
                    tracing::debug!(label = %meta.label, ?err, "vault delete failed during logout")
                }
            }
        }
    }
    Ok(removed_any)
}

fn provider_label(provider_key_id: &str) -> String {
    // Lowercase the env-key id for a stable, readable vault label.
    format!(
        "{PROVIDER_LABEL_PREFIX}{}",
        provider_key_id.trim().to_ascii_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_keyring_store::tests::MockKeyringStore;
    use pretty_assertions::assert_eq;

    #[test]
    fn provider_label_is_namespaced_and_lowercase() {
        assert_eq!(
            provider_label("AMBIENT_API_KEY"),
            "provider/ambient_api_key"
        );
        assert_eq!(provider_label("  ZAI_API_KEY  "), "provider/zai_api_key");
    }

    /// Writes a provider key through the encrypted vault and reads it back via the vault resolver
    /// using the SAME keyring store (mirrors a real OS keyring that persists across calls).
    /// This proves new keys land in encrypted storage (not the plaintext provider_auth.json).
    #[test]
    fn write_then_read_round_trips_through_encrypted_vault() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());
        write_provider_key_with_store(
            codex_home.path(),
            "AMBIENT_API_KEY",
            "ambient-secret",
            keyring.clone(),
        )
        .expect("write should succeed via vault");

        // No plaintext provider_auth.json should have been created: the key lives in the
        // age-encrypted secrets store keyed by the keyring passphrase.
        let legacy_path = codex_home.path().join("provider_auth.json");
        assert!(
            !legacy_path.exists(),
            "vault write must not create a plaintext provider_auth.json"
        );

        let read = read_provider_key_with_store(codex_home.path(), "AMBIENT_API_KEY", keyring)
            .expect("read with same keyring");
        assert_eq!(read.as_deref(), Some("ambient-secret"));
    }

    /// When the vault is empty, the resolver falls back to the legacy plaintext store.
    #[test]
    fn read_falls_back_to_legacy_provider_auth_when_vault_empty() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        // Seed the legacy store directly (no vault involved).
        super::super::manager::legacy_save_provider_key(
            codex_home.path(),
            "OPENROUTER_API_KEY",
            "or-legacy-key",
        )
        .expect("legacy write");

        // Vault is empty (different keyring), so read must come from the legacy store.
        let value = read_provider_key_with_store(
            codex_home.path(),
            "OPENROUTER_API_KEY",
            Arc::new(MockKeyringStore::default()),
        )
        .expect("read should fall back");
        assert_eq!(value.as_deref(), Some("or-legacy-key"));
    }

    /// A successful vault write migrates the old plaintext key off `provider_auth.json`.
    #[test]
    fn vault_write_migrates_legacy_plaintext_key_away() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());

        // Seed the legacy plaintext store with a pre-existing key.
        super::super::manager::legacy_save_provider_key(
            codex_home.path(),
            "AMBIENT_API_KEY",
            "legacy-plaintext",
        )
        .expect("seed legacy key");
        assert!(codex_home.path().join("provider_auth.json").exists());

        // Re-save via the vault (encrypted). The old plaintext copy must be removed.
        write_provider_key_with_store(
            codex_home.path(),
            "AMBIENT_API_KEY",
            "new-encrypted",
            keyring.clone(),
        )
        .expect("vault write");

        // The plaintext file should be gone (it held only this key, so it is deleted when empty).
        assert!(
            !codex_home.path().join("provider_auth.json").exists(),
            "legacy plaintext provider key must be migrated away after a successful vault write"
        );

        // The key now resolves from the encrypted vault.
        let value = read_provider_key_with_store(codex_home.path(), "AMBIENT_API_KEY", keyring)
            .expect("read");
        assert_eq!(value.as_deref(), Some("new-encrypted"));
    }

    /// A successful vault write leaves unrelated legacy keys in `provider_auth.json` intact.
    #[test]
    fn vault_write_preserves_unrelated_legacy_keys() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());
        super::super::manager::legacy_save_provider_key(
            codex_home.path(),
            "AMBIENT_API_KEY",
            "ambient-legacy",
        )
        .expect("seed ambient");
        super::super::manager::legacy_save_provider_key(
            codex_home.path(),
            "OPENROUTER_API_KEY",
            "openrouter-legacy",
        )
        .expect("seed openrouter");

        write_provider_key_with_store(
            codex_home.path(),
            "AMBIENT_API_KEY",
            "ambient-encrypted",
            keyring.clone(),
        )
        .expect("vault write ambient");

        // The file survives because it still holds the unrelated OpenRouter key.
        assert!(codex_home.path().join("provider_auth.json").exists());
        // Ambient is now in the vault...
        assert_eq!(
            read_provider_key_with_store(codex_home.path(), "AMBIENT_API_KEY", keyring)
                .expect("ambient read")
                .as_deref(),
            Some("ambient-encrypted")
        );
        // ...and OpenRouter remains readable from the (now smaller) legacy file.
        assert_eq!(
            super::super::manager::legacy_provider_key(codex_home.path(), "OPENROUTER_API_KEY")
                .expect("openrouter legacy read")
                .as_deref(),
            Some("openrouter-legacy")
        );
    }

    /// Re-writing a provider key rotates the stored secret in place (update, not duplicate).
    #[test]
    fn rewriting_a_provider_key_rotates_in_place() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());
        write_provider_key_with_store(codex_home.path(), "ZAI_API_KEY", "old", keyring.clone())
            .expect("first write");
        write_provider_key_with_store(codex_home.path(), "ZAI_API_KEY", "new", keyring.clone())
            .expect("rotation write");

        let value =
            read_provider_key_with_store(codex_home.path(), "ZAI_API_KEY", keyring).expect("read");
        assert_eq!(value.as_deref(), Some("new"));
    }

    /// `delete_all_provider_keys` removes provider-scoped vault credentials only.
    #[test]
    fn delete_all_provider_keys_clears_provider_entries() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());
        write_provider_key_with_store(codex_home.path(), "AMBIENT_API_KEY", "a", keyring.clone())
            .expect("write ambient");
        write_provider_key_with_store(codex_home.path(), "ZAI_API_KEY", "z", keyring.clone())
            .expect("write zai");

        // Add a non-provider user credential that must survive the provider wipe.
        let vault = Vault::new_with_keyring_store(codex_home.path().to_path_buf(), keyring.clone());
        vault
            .add(AddCredential {
                label: "personal-token".to_string(),
                credential_type: CredentialType::BearerToken,
                provider: None,
                notes: None,
                revocation_notes: None,
                secret: "keep-me".to_string(),
            })
            .expect("user credential");

        let removed =
            super::delete_all_provider_keys_with_store(codex_home.path(), keyring.clone())
                .expect("delete");
        assert!(removed, "provider keys should have been removed");

        // Provider keys are gone...
        assert_eq!(
            read_provider_key_with_store(codex_home.path(), "AMBIENT_API_KEY", keyring)
                .expect("ambient read"),
            None
        );
        // ...but the user credential is untouched.
        assert_eq!(
            vault.reveal("personal-token").expect("user reveal"),
            "keep-me"
        );
    }
}
