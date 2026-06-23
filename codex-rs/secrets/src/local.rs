use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::atomic::compiler_fence;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use age::decrypt;
use age::encrypt;
use age::scrypt::Identity as ScryptIdentity;
use age::scrypt::Recipient as ScryptRecipient;
use age::secrecy::ExposeSecret;
use age::secrecy::SecretString;
use anyhow::Context;
use anyhow::Result;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_keyring_store::CredentialStoreError;
use codex_keyring_store::DefaultKeyringStore;
use codex_keyring_store::KeyringStore;
use rand::TryRngCore;
use rand::rngs::OsRng;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use tracing::warn;

use super::SecretListEntry;
use super::SecretName;
use super::SecretScope;
use super::SecretsBackend;
use super::compute_keyring_account;
use super::keyring_service;

const SECRETS_VERSION: u8 = 1;
const LOCAL_SECRETS_FILENAME: &str = "local.age";
const CODEX_AUTH_SECRETS_FILENAME: &str = "codex_auth.age";
const MCP_OAUTH_SECRETS_FILENAME: &str = "mcp_oauth.age";

/// Selects the local encrypted file used by a `LocalSecretsBackend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalSecretsNamespace {
    /// General managed secrets stored in `local.age`.
    #[default]
    ManagedSecrets,
    /// Codex authentication credentials used by the CLI, TUI, app server, and other clients.
    CodexAuth,
    /// OAuth credentials for external MCP servers.
    McpOAuth,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct SecretsFile {
    version: u8,
    secrets: BTreeMap<String, String>,
}

impl SecretsFile {
    fn new_empty() -> Self {
        Self {
            version: SECRETS_VERSION,
            secrets: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalSecretsBackend {
    codex_home: PathBuf,
    keyring_store: Arc<dyn KeyringStore>,
    namespace: LocalSecretsNamespace,
}

impl LocalSecretsBackend {
    pub(crate) fn new_with_default_keyring(codex_home: PathBuf) -> Self {
        let keyring_store: Arc<dyn KeyringStore> =
            Arc::new(LocalFallbackKeyringStore::new(codex_home.clone()));
        Self::new(codex_home, keyring_store)
    }

    pub fn new(codex_home: PathBuf, keyring_store: Arc<dyn KeyringStore>) -> Self {
        Self::new_with_namespace(
            codex_home,
            keyring_store,
            LocalSecretsNamespace::ManagedSecrets,
        )
    }

    pub fn new_with_namespace(
        codex_home: PathBuf,
        keyring_store: Arc<dyn KeyringStore>,
        namespace: LocalSecretsNamespace,
    ) -> Self {
        Self {
            codex_home,
            keyring_store,
            namespace,
        }
    }

    pub fn set(&self, scope: &SecretScope, name: &SecretName, value: &str) -> Result<()> {
        anyhow::ensure!(!value.is_empty(), "secret value must not be empty");
        let canonical_key = scope.canonical_key(name);
        let mut file = self.load_file()?;
        file.secrets.insert(canonical_key, value.to_string());
        self.save_file(&file)
    }

    pub fn get(&self, scope: &SecretScope, name: &SecretName) -> Result<Option<String>> {
        let canonical_key = scope.canonical_key(name);
        let file = self.load_file()?;
        Ok(file.secrets.get(&canonical_key).cloned())
    }

    pub fn delete(&self, scope: &SecretScope, name: &SecretName) -> Result<bool> {
        let canonical_key = scope.canonical_key(name);
        let mut file = self.load_file()?;
        let removed = file.secrets.remove(&canonical_key).is_some();
        if removed {
            self.save_file(&file)?;
        }
        Ok(removed)
    }

    pub fn list(&self, scope_filter: Option<&SecretScope>) -> Result<Vec<SecretListEntry>> {
        let file = self.load_file()?;
        let mut entries = Vec::new();
        for canonical_key in file.secrets.keys() {
            let Some(entry) = parse_canonical_key(canonical_key) else {
                warn!("skipping invalid canonical secret key: {canonical_key}");
                continue;
            };
            if let Some(scope) = scope_filter
                && entry.scope != *scope
            {
                continue;
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    fn secrets_dir(&self) -> PathBuf {
        self.codex_home.join("secrets")
    }

    fn secrets_path(&self) -> PathBuf {
        let filename = match self.namespace {
            LocalSecretsNamespace::ManagedSecrets => LOCAL_SECRETS_FILENAME,
            LocalSecretsNamespace::CodexAuth => CODEX_AUTH_SECRETS_FILENAME,
            LocalSecretsNamespace::McpOAuth => MCP_OAUTH_SECRETS_FILENAME,
        };
        self.secrets_dir().join(filename)
    }

    fn load_file(&self) -> Result<SecretsFile> {
        let path = self.secrets_path();
        if !path.exists() {
            return Ok(SecretsFile::new_empty());
        }
        set_private_file_permissions(&path)
            .map_err(|err| anyhow::anyhow!(err.message()))
            .with_context(|| format!("failed to harden permissions on {}", path.display()))?;

        let ciphertext = fs::read(&path)
            .with_context(|| format!("failed to read secrets file at {}", path.display()))?;
        let passphrase = self.load_or_create_passphrase()?;
        let plaintext = decrypt_with_passphrase(&ciphertext, &passphrase)?;
        let mut parsed: SecretsFile = serde_json::from_slice(&plaintext).with_context(|| {
            format!(
                "failed to deserialize decrypted secrets file at {}",
                path.display()
            )
        })?;
        if parsed.version == 0 {
            parsed.version = SECRETS_VERSION;
        }
        anyhow::ensure!(
            parsed.version <= SECRETS_VERSION,
            "secrets file version {} is newer than supported version {}",
            parsed.version,
            SECRETS_VERSION
        );
        Ok(parsed)
    }

    fn save_file(&self, file: &SecretsFile) -> Result<()> {
        let dir = self.secrets_dir();
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create secrets dir {}", dir.display()))?;
        set_private_dir_permissions(&dir)
            .map_err(|err| anyhow::anyhow!(err.message()))
            .with_context(|| format!("failed to harden permissions on {}", dir.display()))?;

        let passphrase = self.load_or_create_passphrase()?;
        let plaintext = serde_json::to_vec(file).context("failed to serialize secrets file")?;
        let ciphertext = encrypt_with_passphrase(&plaintext, &passphrase)?;
        let path = self.secrets_path();
        write_file_atomically(&path, &ciphertext)?;
        Ok(())
    }

    fn load_or_create_passphrase(&self) -> Result<SecretString> {
        let account = compute_keyring_account(&self.codex_home);
        let loaded = self
            .keyring_store
            .load(keyring_service(), &account)
            .map_err(|err| anyhow::anyhow!(err.message()))
            .with_context(|| format!("failed to load secrets key from keyring for {account}"))?;
        match loaded {
            Some(existing) => Ok(SecretString::from(existing)),
            None => {
                // Generate a high-entropy key and persist it in the OS keyring.
                // This keeps secrets out of plaintext config while remaining
                // fully local/offline for the MVP.
                let generated = generate_passphrase()?;
                self.keyring_store
                    .save(keyring_service(), &account, generated.expose_secret())
                    .map_err(|err| anyhow::anyhow!(err.message()))
                    .context("failed to persist secrets key in keyring")?;
                Ok(generated)
            }
        }
    }
}

#[derive(Debug)]
struct LocalFallbackKeyringStore {
    codex_home: PathBuf,
    primary: Arc<dyn KeyringStore>,
}

impl LocalFallbackKeyringStore {
    fn new(codex_home: PathBuf) -> Self {
        Self::new_with_primary(codex_home, Arc::new(DefaultKeyringStore))
    }

    fn new_with_primary(codex_home: PathBuf, primary: Arc<dyn KeyringStore>) -> Self {
        Self {
            codex_home,
            primary,
        }
    }

    fn fallback_dir(&self) -> PathBuf {
        self.codex_home.join("secrets").join("keyring-fallback")
    }

    fn fallback_path(&self, service: &str, account: &str) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(service.as_bytes());
        hasher.update([0]);
        hasher.update(account.as_bytes());
        let digest = hasher.finalize();
        let filename = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.fallback_dir().join(format!("{filename}.key"))
    }

    fn load_fallback(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<String>, CredentialStoreError> {
        let path = self.fallback_path(service, account);
        match fs::read_to_string(&path) {
            Ok(value) => Ok(Some(value)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(CredentialStoreError::from_message(format!(
                "failed to read local keyring fallback {}: {err}",
                path.display()
            ))),
        }
    }

    fn save_fallback(
        &self,
        service: &str,
        account: &str,
        value: &str,
    ) -> Result<(), CredentialStoreError> {
        let dir = self.fallback_dir();
        fs::create_dir_all(&dir).map_err(|err| {
            CredentialStoreError::from_message(format!(
                "failed to create local keyring fallback dir {}: {err}",
                dir.display()
            ))
        })?;
        set_private_dir_permissions(&dir)?;
        let path = self.fallback_path(service, account);
        write_keyring_fallback_file_atomically(&path, value.as_bytes())
    }

    fn delete_fallback(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        let path = self.fallback_path(service, account);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(CredentialStoreError::from_message(format!(
                "failed to delete local keyring fallback {}: {err}",
                path.display()
            ))),
        }
    }
}

impl KeyringStore for LocalFallbackKeyringStore {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        match self.primary.load(service, account) {
            Ok(Some(value)) => Ok(Some(value)),
            Ok(None) => self.load_fallback(service, account),
            Err(err) => {
                warn!(
                    service,
                    account,
                    error = %err,
                    "OS keyring unavailable; using local file-backed keyring fallback"
                );
                self.load_fallback(service, account)
            }
        }
    }

    fn save(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        match self.primary.save(service, account, value) {
            Ok(()) => {
                let _ = self.delete_fallback(service, account);
                Ok(())
            }
            Err(err) => {
                warn!(
                    service,
                    account,
                    error = %err,
                    "OS keyring unavailable; storing local encrypted-secrets passphrase in file-backed fallback"
                );
                self.save_fallback(service, account, value)
            }
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        let primary_removed = match self.primary.delete(service, account) {
            Ok(removed) => removed,
            Err(err) => {
                warn!(
                    service,
                    account,
                    error = %err,
                    "OS keyring unavailable while deleting keyring entry; deleting local fallback"
                );
                false
            }
        };
        Ok(self.delete_fallback(service, account)? || primary_removed)
    }
}

impl SecretsBackend for LocalSecretsBackend {
    fn set(&self, scope: &SecretScope, name: &SecretName, value: &str) -> Result<()> {
        LocalSecretsBackend::set(self, scope, name, value)
    }

    fn get(&self, scope: &SecretScope, name: &SecretName) -> Result<Option<String>> {
        LocalSecretsBackend::get(self, scope, name)
    }

    fn delete(&self, scope: &SecretScope, name: &SecretName) -> Result<bool> {
        LocalSecretsBackend::delete(self, scope, name)
    }

    fn list(&self, scope_filter: Option<&SecretScope>) -> Result<Vec<SecretListEntry>> {
        LocalSecretsBackend::list(self, scope_filter)
    }
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path.parent().with_context(|| {
        format!(
            "failed to compute parent directory for secrets file at {}",
            path.display()
        )
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let filename = path.file_name().with_context(|| {
        format!(
            "failed to compute filename for secrets file at {}",
            path.display()
        )
    })?;
    let tmp_path = dir.join(format!(
        ".{}.tmp-{}-{nonce}",
        filename.to_string_lossy(),
        std::process::id()
    ));

    {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut tmp_file = options.open(&tmp_path).with_context(|| {
            format!(
                "failed to create temp secrets file at {}",
                tmp_path.display()
            )
        })?;
        tmp_file.write_all(contents).with_context(|| {
            format!(
                "failed to write temp secrets file at {}",
                tmp_path.display()
            )
        })?;
        tmp_file.sync_all().with_context(|| {
            format!("failed to sync temp secrets file at {}", tmp_path.display())
        })?;
    }

    match fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(initial_error) => {
            #[cfg(target_os = "windows")]
            {
                if path.exists() {
                    fs::remove_file(path).with_context(|| {
                        format!(
                            "failed to remove existing secrets file at {} before replace",
                            path.display()
                        )
                    })?;
                    fs::rename(&tmp_path, path).with_context(|| {
                        format!(
                            "failed to replace secrets file at {} with {}",
                            path.display(),
                            tmp_path.display()
                        )
                    })?;
                    return Ok(());
                }
            }

            let _ = fs::remove_file(&tmp_path);
            Err(initial_error).with_context(|| {
                format!(
                    "failed to atomically replace secrets file at {} with {}",
                    path.display(),
                    tmp_path.display()
                )
            })
        }
    }
}

fn set_private_file_permissions(path: &Path) -> Result<(), CredentialStoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            CredentialStoreError::from_message(format!(
                "failed to set private permissions on {}: {err}",
                path.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn set_private_dir_permissions(path: &Path) -> Result<(), CredentialStoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
            CredentialStoreError::from_message(format!(
                "failed to set private permissions on {}: {err}",
                path.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn write_keyring_fallback_file_atomically(
    path: &Path,
    contents: &[u8],
) -> Result<(), CredentialStoreError> {
    let dir = path.parent().ok_or_else(|| {
        CredentialStoreError::from_message(format!(
            "failed to compute parent directory for fallback key at {}",
            path.display()
        ))
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let filename = path.file_name().ok_or_else(|| {
        CredentialStoreError::from_message(format!(
            "failed to compute filename for fallback key at {}",
            path.display()
        ))
    })?;
    let tmp_path = dir.join(format!(
        ".{}.tmp-{}-{nonce}",
        filename.to_string_lossy(),
        std::process::id()
    ));

    {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut tmp_file = options.open(&tmp_path).map_err(|err| {
            CredentialStoreError::from_message(format!(
                "failed to create temp fallback key file {}: {err}",
                tmp_path.display()
            ))
        })?;
        tmp_file.write_all(contents).map_err(|err| {
            CredentialStoreError::from_message(format!(
                "failed to write temp fallback key file {}: {err}",
                tmp_path.display()
            ))
        })?;
        tmp_file.sync_all().map_err(|err| {
            CredentialStoreError::from_message(format!(
                "failed to sync temp fallback key file {}: {err}",
                tmp_path.display()
            ))
        })?;
    }

    fs::rename(&tmp_path, path).map_err(|err| {
        let _ = fs::remove_file(&tmp_path);
        CredentialStoreError::from_message(format!(
            "failed to replace fallback key file {} with {}: {err}",
            path.display(),
            tmp_path.display()
        ))
    })
}

fn generate_passphrase() -> Result<SecretString> {
    let mut bytes = [0_u8; 32];
    let mut rng = OsRng;
    rng.try_fill_bytes(&mut bytes)
        .context("failed to generate random secrets key")?;
    // Base64 keeps the keyring payload ASCII-safe without reducing entropy.
    let encoded = BASE64_STANDARD.encode(bytes);
    wipe_bytes(&mut bytes);
    Ok(SecretString::from(encoded))
}

fn wipe_bytes(bytes: &mut [u8]) {
    for byte in bytes {
        // Volatile writes make it much harder for the compiler to elide the wipe.
        // SAFETY: `byte` is a valid mutable reference into `bytes`.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

fn encrypt_with_passphrase(plaintext: &[u8], passphrase: &SecretString) -> Result<Vec<u8>> {
    let recipient = ScryptRecipient::new(passphrase.clone());
    encrypt(&recipient, plaintext).context("failed to encrypt secrets file")
}

fn decrypt_with_passphrase(ciphertext: &[u8], passphrase: &SecretString) -> Result<Vec<u8>> {
    let identity = ScryptIdentity::new(passphrase.clone());
    decrypt(&identity, ciphertext).context("failed to decrypt secrets file")
}

fn parse_canonical_key(canonical_key: &str) -> Option<SecretListEntry> {
    let mut parts = canonical_key.split('/');
    let scope_kind = parts.next()?;
    match scope_kind {
        "global" => {
            let name = parts.next()?;
            if parts.next().is_some() {
                return None;
            }
            let name = SecretName::new(name).ok()?;
            Some(SecretListEntry {
                scope: SecretScope::Global,
                name,
            })
        }
        "env" => {
            let environment_id = parts.next()?;
            let name = parts.next()?;
            if parts.next().is_some() {
                return None;
            }
            let name = SecretName::new(name).ok()?;
            let scope = SecretScope::environment(environment_id.to_string()).ok()?;
            Some(SecretListEntry { scope, name })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_keyring_store::tests::MockKeyringStore;
    use keyring::Error as KeyringError;
    use pretty_assertions::assert_eq;

    #[derive(Debug)]
    struct AlwaysFailingKeyringStore;

    impl KeyringStore for AlwaysFailingKeyringStore {
        fn load(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<Option<String>, CredentialStoreError> {
            Err(CredentialStoreError::from_message("forced load failure"))
        }

        fn save(
            &self,
            _service: &str,
            _account: &str,
            _value: &str,
        ) -> Result<(), CredentialStoreError> {
            Err(CredentialStoreError::from_message("forced save failure"))
        }

        fn delete(&self, _service: &str, _account: &str) -> Result<bool, CredentialStoreError> {
            Err(CredentialStoreError::from_message("forced delete failure"))
        }
    }

    #[test]
    fn load_file_rejects_newer_schema_versions() -> Result<()> {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());
        let backend = LocalSecretsBackend::new(codex_home.path().to_path_buf(), keyring);

        let file = SecretsFile {
            version: SECRETS_VERSION + 1,
            secrets: BTreeMap::new(),
        };
        backend.save_file(&file)?;

        let error = backend
            .load_file()
            .expect_err("must reject newer schema version");
        assert!(
            error.to_string().contains("newer than supported version"),
            "unexpected error: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn set_fails_when_keyring_is_unavailable() -> Result<()> {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());
        let account = compute_keyring_account(codex_home.path());
        keyring.set_error(
            &account,
            KeyringError::Invalid("error".into(), "load".into()),
        );

        let backend = LocalSecretsBackend::new(codex_home.path().to_path_buf(), keyring);
        let scope = SecretScope::Global;
        let name = SecretName::new("TEST_SECRET")?;
        let error = backend
            .set(&scope, &name, "secret-value")
            .expect_err("must fail when keyring load fails");
        assert!(
            error
                .to_string()
                .contains("failed to load secrets key from keyring"),
            "unexpected error: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn default_keyring_fallback_persists_when_os_keyring_is_unavailable() -> Result<()> {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let fallback = Arc::new(LocalFallbackKeyringStore::new_with_primary(
            codex_home.path().to_path_buf(),
            Arc::new(AlwaysFailingKeyringStore),
        ));
        let backend = LocalSecretsBackend::new(codex_home.path().to_path_buf(), fallback);
        let scope = SecretScope::Global;
        let name = SecretName::new("TEST_SECRET")?;

        backend.set(&scope, &name, "secret-value")?;
        assert_eq!(
            backend.get(&scope, &name)?,
            Some("secret-value".to_string())
        );

        let secrets_path = codex_home
            .path()
            .join("secrets")
            .join(LOCAL_SECRETS_FILENAME);
        assert!(secrets_path.exists(), "encrypted secrets file should exist");

        let fallback_dir = codex_home.path().join("secrets").join("keyring-fallback");
        let fallback_files = fs::read_dir(&fallback_dir)
            .with_context(|| format!("failed to read {}", fallback_dir.display()))?
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("failed to enumerate {}", fallback_dir.display()))?;
        assert_eq!(fallback_files.len(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(secrets_path.metadata()?.permissions().mode() & 0o777, 0o600);
            assert_eq!(fallback_dir.metadata()?.permissions().mode() & 0o777, 0o700);
            assert_eq!(
                fallback_files[0].metadata()?.permissions().mode() & 0o777,
                0o600
            );
        }

        Ok(())
    }

    #[test]
    fn save_file_does_not_leave_temp_files() -> Result<()> {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());
        let backend = LocalSecretsBackend::new(codex_home.path().to_path_buf(), keyring);

        let scope = SecretScope::Global;
        let name = SecretName::new("TEST_SECRET")?;
        backend.set(&scope, &name, "one")?;
        backend.set(&scope, &name, "two")?;

        let secrets_dir = backend.secrets_dir();
        let entries = fs::read_dir(&secrets_dir)
            .with_context(|| format!("failed to read {}", secrets_dir.display()))?
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("failed to enumerate {}", secrets_dir.display()))?;

        let filenames: Vec<String> = entries
            .into_iter()
            .filter_map(|entry| entry.file_name().to_str().map(ToString::to_string))
            .collect();
        assert_eq!(filenames, vec![LOCAL_SECRETS_FILENAME.to_string()]);
        assert_eq!(backend.get(&scope, &name)?, Some("two".to_string()));
        Ok(())
    }

    #[test]
    fn local_namespaces_write_separate_files() -> Result<()> {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MockKeyringStore::default());
        let codex_auth_backend = LocalSecretsBackend::new_with_namespace(
            codex_home.path().to_path_buf(),
            keyring.clone(),
            LocalSecretsNamespace::CodexAuth,
        );
        let mcp_backend = LocalSecretsBackend::new_with_namespace(
            codex_home.path().to_path_buf(),
            keyring,
            LocalSecretsNamespace::McpOAuth,
        );
        let scope = SecretScope::Global;
        let name = SecretName::new("TEST_SECRET")?;

        codex_auth_backend.set(&scope, &name, "codex-auth-value")?;
        mcp_backend.set(&scope, &name, "mcp-value")?;

        assert_eq!(
            codex_auth_backend.get(&scope, &name)?,
            Some("codex-auth-value".to_string())
        );
        assert_eq!(
            mcp_backend.get(&scope, &name)?,
            Some("mcp-value".to_string())
        );
        assert!(
            codex_home
                .path()
                .join("secrets")
                .join("codex_auth.age")
                .exists()
        );
        assert!(
            codex_home
                .path()
                .join("secrets")
                .join("mcp_oauth.age")
                .exists()
        );
        assert!(!codex_home.path().join("secrets").join("local.age").exists());
        Ok(())
    }
}
