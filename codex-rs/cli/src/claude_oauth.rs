//! Refresh-capable access to Claude Code subscription credentials.

use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use serde::Deserialize;
use tokio::process::Command;

const CLAUDE_CREDENTIALS_FILE: &str = ".credentials.json";
const CLAUDE_REFRESH_LOCK_FILE: &str = ".pfterminal-oauth-refresh.lock";
// Avoid beginning a potentially long streaming request with a token that is about to expire.
const MIN_TOKEN_VALIDITY_MS: u64 = 5 * 60_000;
const CLAUDE_REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize)]
struct ClaudeCodeCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeCodeOauthCredentials>,
}

#[derive(Clone, Debug, Deserialize)]
struct ClaudeCodeOauthCredentials {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<u64>,
    #[serde(default)]
    scopes: Vec<String>,
}

pub(crate) async fn resolve_claude_oauth_access_token() -> Result<String> {
    if let Some(token) = nonempty_env("CLAUDE_CODE_OAUTH_TOKEN") {
        return Ok(token);
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set; cannot read Claude Code credentials"))?;
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));

    let force_refresh =
        nonempty_env("PFTERMINAL_PROVIDER_AUTH_FORCE_REFRESH").as_deref() == Some("1");
    resolve_stored_claude_oauth_access_token(&config_dir, None, current_time_ms(), force_refresh)
        .await
}

async fn resolve_stored_claude_oauth_access_token(
    config_dir: &Path,
    claude_executable: Option<&Path>,
    now_ms: u64,
    force_refresh: bool,
) -> Result<String> {
    let credentials_path = config_dir.join(CLAUDE_CREDENTIALS_FILE);
    let credentials = read_credentials(&credentials_path).await?;
    if !force_refresh && let Some(access_token) = usable_access_token(&credentials, now_ms) {
        return Ok(access_token);
    }
    let original_access_token = credentials
        .claude_ai_oauth
        .as_ref()
        .and_then(|oauth| oauth.access_token.clone());

    let _refresh_lock = acquire_refresh_lock(config_dir).await?;

    // Another PFTerminal process may have refreshed while this process waited.
    let credentials = read_credentials(&credentials_path).await?;
    if let Some(access_token) = usable_access_token(&credentials, current_time_ms().max(now_ms))
        && (!force_refresh || Some(access_token.clone()) != original_access_token)
    {
        return Ok(access_token);
    }

    let credentials_before_refresh = tokio::fs::read(&credentials_path)
        .await
        .with_context(|| format!("failed to snapshot {}", credentials_path.display()))?;
    let refresh_status =
        refresh_with_claude_cli(config_dir, &credentials, claude_executable).await?;

    let refreshed = read_credentials(&credentials_path).await;
    if let Ok(refreshed) = &refreshed
        && let Some(access_token) = usable_access_token(refreshed, current_time_ms().max(now_ms))
        && Some(access_token.clone()) != original_access_token
    {
        return Ok(access_token);
    }
    restore_credentials_if_changed(&credentials_path, &credentials_before_refresh).await?;
    if !refresh_status.success() {
        return Err(anyhow!(
            "Claude Code OAuth refresh failed with status {refresh_status}. Run `claude /login` again."
        ));
    }
    if let Err(err) = refreshed {
        return Err(err.context(
            "Claude Code produced invalid credentials during refresh; PFTerminal restored the previous credential file",
        ));
    }
    Err(anyhow!(
        "Claude Code completed OAuth refresh but did not persist a new usable access token. Run `claude /login` again."
    ))
}

async fn read_credentials(path: &Path) -> Result<ClaudeCodeCredentials> {
    let contents = tokio::fs::read_to_string(path).await.map_err(|err| {
        anyhow!(
            "Claude Code credentials not found at {} ({err}). Run `claude /login` with your Claude subscription.",
            path.display()
        )
    })?;
    serde_json::from_str(&contents).map_err(|err| {
        anyhow!(
            "Claude Code credentials at {} are not valid JSON ({err}). Run `claude /login` again.",
            path.display()
        )
    })
}

fn usable_access_token(credentials: &ClaudeCodeCredentials, now_ms: u64) -> Option<String> {
    let oauth = credentials.claude_ai_oauth.as_ref()?;
    let expires_at = oauth.expires_at?;
    if expires_at <= now_ms.saturating_add(MIN_TOKEN_VALIDITY_MS) {
        return None;
    }
    oauth
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
}

async fn refresh_with_claude_cli(
    config_dir: &Path,
    credentials: &ClaudeCodeCredentials,
    claude_executable: Option<&Path>,
) -> Result<std::process::ExitStatus> {
    let oauth = credentials.claude_ai_oauth.as_ref().ok_or_else(|| {
        anyhow!(
            "Claude Code OAuth credentials are missing. Run `claude /login` and choose a Claude subscription account."
        )
    })?;
    oauth
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            anyhow!("Claude Code OAuth refresh token is missing. Run `claude /login` again.")
        })?;
    if oauth.scopes.is_empty() {
        return Err(anyhow!(
            "Claude Code OAuth scopes are missing. Run `claude /login` again."
        ));
    }

    let executable = match claude_executable {
        Some(path) => path.to_path_buf(),
        None => which::which("claude")
            .context("Claude Code executable was not found on PATH; cannot refresh OAuth token")?,
    };
    let mut command = Command::new(&executable);
    command
        .args([
            "-p",
            "--safe-mode",
            "--no-session-persistence",
            "--model",
            "haiku",
            "--tools",
            "",
            "--max-budget-usd",
            "0.01",
            "Reply with OK.",
        ])
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env_remove("CLAUDE_CODE_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_REFRESH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_SCOPES")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_BASE_URL")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    tokio::time::timeout(CLAUDE_REFRESH_TIMEOUT, command.status())
        .await
        .map_err(|_| anyhow!("Claude Code OAuth refresh timed out after 30 seconds"))?
        .with_context(|| {
            format!(
                "failed to start `{}` for OAuth refresh",
                executable.display()
            )
        })
}

async fn restore_credentials_if_changed(path: &Path, original: &[u8]) -> Result<()> {
    if matches!(tokio::fs::read(path).await, Ok(current) if current == original) {
        return Ok(());
    }

    let path = path.to_path_buf();
    let original = original.to_vec();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("Claude credentials path has no parent: {}", path.display()))?;
        let mut replacement = tempfile::NamedTempFile::new_in(parent).with_context(|| {
            format!(
                "failed to create credential recovery file in {}",
                parent.display()
            )
        })?;
        replacement
            .write_all(&original)
            .context("failed to write credential recovery file")?;
        replacement
            .as_file()
            .sync_all()
            .context("failed to sync credential recovery file")?;
        replacement.persist(&path).map_err(|err| {
            anyhow!(
                "failed to restore Claude credentials at {}: {}",
                path.display(),
                err.error
            )
        })?;
        Ok(())
    })
    .await
    .context("Claude credential recovery task failed")??;
    Ok(())
}

async fn acquire_refresh_lock(config_dir: &Path) -> Result<File> {
    let lock_path = config_dir.join(CLAUDE_REFRESH_LOCK_FILE);
    tokio::fs::create_dir_all(config_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create Claude Code config directory {}",
                config_dir.display()
            )
        })?;
    tokio::task::spawn_blocking(move || -> io::Result<File> {
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock_file.lock()?;
        Ok(lock_file)
    })
    .await
    .context("Claude OAuth refresh lock task failed")?
    .context("failed to acquire Claude OAuth refresh lock")
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn valid_access_token_does_not_require_claude_cli() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let now_ms = current_time_ms();
        write_credentials(
            temp_dir.path(),
            "valid-access",
            "valid-refresh",
            now_ms + 600_000,
        );

        let token = resolve_stored_claude_oauth_access_token(
            temp_dir.path(),
            Some(Path::new("/does/not/exist")),
            now_ms,
            false,
        )
        .await
        .expect("valid token should be returned without starting Claude");

        assert_eq!(token, "valid-access");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn expiring_access_token_is_refreshed_by_claude_cli() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let now_ms = current_time_ms();
        write_credentials(
            temp_dir.path(),
            "expiring-access",
            "rotating-refresh",
            now_ms + 4 * 60_000,
        );
        let claude = fake_refreshing_claude(temp_dir.path(), now_ms + 600_000, 0, 0);

        let token =
            resolve_stored_claude_oauth_access_token(temp_dir.path(), Some(&claude), now_ms, false)
                .await
                .expect("Claude CLI should refresh expiring credentials");

        assert_eq!(token, "refreshed-access");
        let refresh_log =
            std::fs::read_to_string(temp_dir.path().join("refresh.log")).expect("refresh log");
        assert_eq!(refresh_log, "official-cli-refresh\n");
        let persisted = read_credentials(&temp_dir.path().join(CLAUDE_CREDENTIALS_FILE))
            .await
            .expect("persisted credentials");
        assert_eq!(
            persisted
                .claude_ai_oauth
                .and_then(|oauth| oauth.refresh_token),
            Some("refreshed-refresh".to_string())
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_refreshes_share_one_claude_exchange() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let now_ms = current_time_ms();
        write_credentials(
            temp_dir.path(),
            "expired-access",
            "single-use-refresh",
            now_ms.saturating_sub(1),
        );
        let claude = fake_refreshing_claude(temp_dir.path(), now_ms + 600_000, 1, 0);
        let config_a = temp_dir.path().to_path_buf();
        let config_b = config_a.clone();
        let claude_a = claude.clone();
        let claude_b = claude.clone();

        let (first, second) = tokio::join!(
            resolve_stored_claude_oauth_access_token(&config_a, Some(&claude_a), now_ms, false,),
            resolve_stored_claude_oauth_access_token(&config_b, Some(&claude_b), now_ms, false),
        );

        assert_eq!(first.expect("first refresh"), "refreshed-access");
        assert_eq!(second.expect("second refresh"), "refreshed-access");
        let refresh_log =
            std::fs::read_to_string(temp_dir.path().join("refresh.log")).expect("refresh log");
        assert_eq!(refresh_log.lines().count(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn forced_refresh_replaces_an_unexpired_rejected_token() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let now_ms = current_time_ms();
        write_credentials(
            temp_dir.path(),
            "rejected-but-unexpired-access",
            "refresh-after-401",
            now_ms + 600_000,
        );
        let claude = fake_refreshing_claude(temp_dir.path(), now_ms + 900_000, 0, 0);

        let token =
            resolve_stored_claude_oauth_access_token(temp_dir.path(), Some(&claude), now_ms, true)
                .await
                .expect("401 recovery should force Claude credential rotation");

        assert_eq!(token, "refreshed-access");
        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("refresh.log"))
                .expect("refresh log")
                .lines()
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rotated_credentials_win_over_nonzero_claude_exit() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let now_ms = current_time_ms();
        write_credentials(
            temp_dir.path(),
            "expired-access",
            "refresh-before-nonzero-exit",
            now_ms.saturating_sub(1),
        );
        let claude = fake_refreshing_claude(temp_dir.path(), now_ms + 600_000, 0, 1);

        let token =
            resolve_stored_claude_oauth_access_token(temp_dir.path(), Some(&claude), now_ms, false)
                .await
                .expect("persisted credential rotation is the refresh authority");

        assert_eq!(token, "refreshed-access");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_refresh_preserves_credentials_and_redacts_refresh_token() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let now_ms = current_time_ms();
        write_credentials(
            temp_dir.path(),
            "expired-access",
            "never-print-this-refresh-token",
            now_ms.saturating_sub(1),
        );
        let credentials_path = temp_dir.path().join(CLAUDE_CREDENTIALS_FILE);
        let before = std::fs::read(&credentials_path).expect("credentials before refresh");
        let claude = fake_failing_claude(temp_dir.path());

        let error =
            resolve_stored_claude_oauth_access_token(temp_dir.path(), Some(&claude), now_ms, false)
                .await
                .expect_err("failed Claude exchange should surface");

        assert!(error.to_string().contains("OAuth refresh failed"));
        assert!(!error.to_string().contains("never-print-this-refresh-token"));
        assert_eq!(
            std::fs::read(credentials_path).expect("credentials after refresh"),
            before
        );
    }

    fn write_credentials(
        config_dir: &Path,
        access_token: &str,
        refresh_token: &str,
        expires_at: u64,
    ) {
        let credentials = json!({
            "claudeAiOauth": {
                "accessToken": access_token,
                "refreshToken": refresh_token,
                "expiresAt": expires_at,
                "scopes": ["user:profile", "user:inference"]
            }
        });
        std::fs::write(
            config_dir.join(CLAUDE_CREDENTIALS_FILE),
            serde_json::to_vec(&credentials).expect("serialize credentials"),
        )
        .expect("write credentials");
    }

    #[cfg(unix)]
    fn fake_refreshing_claude(
        config_dir: &Path,
        expires_at: u64,
        sleep_seconds: u64,
        exit_code: i32,
    ) -> PathBuf {
        let executable = config_dir.join("fake-claude");
        let script = format!(
            r#"#!/bin/sh
set -eu
[ "$1" = "-p" ]
[ -z "${{CLAUDE_CODE_OAUTH_REFRESH_TOKEN:-}}" ]
[ -z "${{CLAUDE_CODE_OAUTH_SCOPES:-}}" ]
printf '%s\n' 'official-cli-refresh' >> "$CLAUDE_CONFIG_DIR/refresh.log"
sleep {sleep_seconds}
printf '%s\n' '{{"claudeAiOauth":{{"accessToken":"refreshed-access","refreshToken":"refreshed-refresh","expiresAt":{expires_at},"scopes":["user:profile","user:inference"]}}}}' > "$CLAUDE_CONFIG_DIR/{CLAUDE_CREDENTIALS_FILE}"
exit {exit_code}
"#
        );
        std::fs::write(&executable, script).expect("write fake Claude");
        let mut permissions = std::fs::metadata(&executable)
            .expect("fake Claude metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).expect("make fake Claude executable");
        executable
    }

    #[cfg(unix)]
    fn fake_failing_claude(config_dir: &Path) -> PathBuf {
        let executable = config_dir.join("failing-claude");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"claudeAiOauth\":{{\"accessToken\":\"\",\"refreshToken\":\"\",\"expiresAt\":0,\"scopes\":[]}}}}' > \"$CLAUDE_CONFIG_DIR/{CLAUDE_CREDENTIALS_FILE}\"\nexit 7\n"
        );
        std::fs::write(&executable, script).expect("write failing Claude");
        let mut permissions = std::fs::metadata(&executable)
            .expect("failing Claude metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).expect("make fake Claude executable");
        executable
    }
}
