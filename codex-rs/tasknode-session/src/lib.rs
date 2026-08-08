//! Task Node terminal-session state, shared by the TUI (`/tasknode`) and the
//! JSON helper CLI (`pfterminal tasknode …`).
//!
//! # Why this crate exists
//!
//! The link flow was previously implemented twice (TUI and CLI) around a
//! single mutable vault record, and starting a relink wrote a token-less
//! "pending" record over the active credential — destroying a working session
//! before the replacement existed. The full incident write-up lives in
//! `docs/archive/2026/TASKNODE_GITHUB_LINK_FAILURE_ANALYSIS.md`.
//!
//! This crate makes that failure unrepresentable at the storage layer:
//!
//! - **Active and pending state live under different vault labels.** Starting
//!   a link only ever writes [`TASKNODE_PENDING_LINK_LABEL`]. The active
//!   session under [`TASKNODE_ACTIVE_SESSION_LABEL`] is written by exactly one
//!   operation: [`promote_active`], which callers invoke only after the
//!   replacement token has been validated against the server.
//! - **Both surfaces share one resolver.** [`load`] returns the same
//!   [`LocalState`] to the TUI and the CLI, so the two can no longer disagree
//!   about whether a session exists.
//! - **Legacy single-record state migrates transparently.** Older builds may
//!   have left a pending-only record under the active label; [`load`] moves it
//!   to the pending label so a valid active session can be restored without
//!   fighting the old blob.

use codex_vault::AddCredential;
use codex_vault::CredentialType;
use codex_vault::Vault;
use codex_vault::VaultError;
use serde::Deserialize;
use serde::Serialize;

/// Vault label holding the active terminal session (bearer token). Kept at the
/// pre-split value so existing linked installations keep working unchanged.
pub const TASKNODE_ACTIVE_SESSION_LABEL: &str = "tasknode/session";
/// Vault label holding an in-flight GitHub link attempt. Never contains a
/// usable bearer token.
pub const TASKNODE_PENDING_LINK_LABEL: &str = "tasknode/link-pending";

/// A proven, usable terminal session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveSession {
    pub origin: String,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub github_username: Option<String>,
    pub terminal_token: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// An in-flight link attempt: authority to poll, not authority to act.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingLink {
    pub origin: String,
    pub request_id: String,
    pub poll_token: String,
    pub verification_url: String,
    #[serde(default)]
    pub started_at: Option<String>,
}

/// Combined local state as both surfaces must see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalState {
    pub active: Option<ActiveSession>,
    pub pending: Option<PendingLink>,
}

/// Pre-split record shape: one blob that was either an active session or a
/// pending attempt depending on which fields were populated.
#[derive(Debug, Deserialize)]
struct LegacyRecord {
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    github_username: Option<String>,
    #[serde(default)]
    terminal_token: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    pending_request_id: Option<String>,
    #[serde(default)]
    pending_poll_token: Option<String>,
    #[serde(default)]
    pending_verification_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("vault error: {0}")]
    Vault(String),
    #[error("invalid local Task Node session state: {0}")]
    Corrupt(String),
}

impl From<VaultError> for SessionStoreError {
    fn from(err: VaultError) -> Self {
        Self::Vault(err.to_string())
    }
}

/// Load combined state, transparently migrating legacy single-record blobs.
///
/// Migration is deliberately conservative: a legacy pending-only blob under
/// the active label is moved to the pending label (unless a newer pending
/// attempt already exists) and the active label is cleared, because that blob
/// never contained usable authority in the first place.
pub fn load(vault: &Vault) -> Result<LocalState, SessionStoreError> {
    let mut state = LocalState::default();

    if let Some(raw) = reveal_optional(vault, TASKNODE_PENDING_LINK_LABEL)? {
        let pending: PendingLink = serde_json::from_str(&raw)
            .map_err(|err| SessionStoreError::Corrupt(format!("pending link: {err}")))?;
        state.pending = Some(pending);
    }

    if let Some(raw) = reveal_optional(vault, TASKNODE_ACTIVE_SESSION_LABEL)? {
        let record: LegacyRecord = serde_json::from_str(&raw)
            .map_err(|err| SessionStoreError::Corrupt(format!("active session: {err}")))?;
        let origin = record.origin.unwrap_or_default();
        match record.terminal_token {
            Some(token) if !token.trim().is_empty() => {
                state.active = Some(ActiveSession {
                    origin,
                    account_id: record.account_id,
                    github_username: record.github_username,
                    terminal_token: token,
                    expires_at: record.expires_at,
                });
            }
            _ => {
                // Legacy pending-only blob written by the pre-split link flow.
                if state.pending.is_none()
                    && let (Some(request_id), Some(poll_token)) =
                        (record.pending_request_id, record.pending_poll_token)
                {
                    let pending = PendingLink {
                        origin,
                        request_id,
                        poll_token,
                        verification_url: record.pending_verification_url.unwrap_or_default(),
                        started_at: None,
                    };
                    save_pending(vault, &pending)?;
                    state.pending = Some(pending);
                }
                // Either way the active label holds no authority; clear it so
                // a real session can be written cleanly later.
                let _ = vault.delete(TASKNODE_ACTIVE_SESSION_LABEL);
            }
        }
    }

    Ok(state)
}

/// Record a new link attempt. Never touches the active session.
pub fn save_pending(vault: &Vault, pending: &PendingLink) -> Result<(), SessionStoreError> {
    let secret = serde_json::to_string(pending)
        .map_err(|err| SessionStoreError::Corrupt(format!("serialize pending link: {err}")))?;
    upsert(
        vault,
        TASKNODE_PENDING_LINK_LABEL,
        secret,
        "Task Node link attempt in progress; holds no usable token.",
        &pending.origin,
    )
}

/// Atomically install a validated replacement session, then clear the pending
/// attempt. Callers must have proven the token against the server first (a
/// successful authenticated `status` call); this function is the only writer
/// of the active label.
pub fn promote_active(vault: &Vault, session: &ActiveSession) -> Result<(), SessionStoreError> {
    let secret = serde_json::to_string(session)
        .map_err(|err| SessionStoreError::Corrupt(format!("serialize session: {err}")))?;
    upsert(
        vault,
        TASKNODE_ACTIVE_SESSION_LABEL,
        secret,
        "Task Node terminal session; token is not printed to chat.",
        &session.origin,
    )?;
    // Best-effort: a leftover pending record cannot shadow the active session
    // (load prefers active), so a failed delete here is not fatal.
    let _ = vault.delete(TASKNODE_PENDING_LINK_LABEL);
    Ok(())
}

/// Abandon an in-flight link attempt. The active session is untouched.
pub fn clear_pending(vault: &Vault) -> Result<bool, SessionStoreError> {
    Ok(vault.delete(TASKNODE_PENDING_LINK_LABEL)?)
}

/// Remove all Task Node authentication state (unlink).
pub fn clear_all(vault: &Vault) -> Result<(), SessionStoreError> {
    let _ = vault.delete(TASKNODE_PENDING_LINK_LABEL);
    let _ = vault.delete(TASKNODE_ACTIVE_SESSION_LABEL);
    Ok(())
}

/// Non-secret diagnostic view of local state; safe to print anywhere.
pub fn state_summary(state: &LocalState) -> serde_json::Value {
    serde_json::json!({
        "activeSession": state.active.as_ref().map(|active| serde_json::json!({
            "origin": active.origin,
            "accountId": active.account_id,
            "githubUsername": active.github_username,
            "expiresAt": active.expires_at,
            "expired": active.is_expired(),
        })),
        "pendingLink": state.pending.as_ref().map(|pending| serde_json::json!({
            "origin": pending.origin,
            "requestId": pending.request_id,
            "verificationUrl": pending.verification_url,
            "startedAt": pending.started_at,
        })),
    })
}

/// `POST /api/auth/terminal/start/github` response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminalAuthStart {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "pollToken")]
    pub poll_token: String,
    #[serde(rename = "verificationUrl")]
    pub verification_url: String,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<String>,
}

/// `GET /api/auth/terminal/session` success response.
#[derive(Clone, Debug, Deserialize)]
pub struct TerminalSessionIssued {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "githubUsername", default)]
    pub github_username: Option<String>,
    #[serde(rename = "terminalToken")]
    pub terminal_token: String,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<String>,
}

impl ActiveSession {
    /// Whether the server-provided expiry has passed at `now`.
    ///
    /// A missing or unparseable expiry counts as "not expired": the server is
    /// the authority, and guessing a session dead when the metadata is absent
    /// would lock users out of a working session.
    pub fn is_expired_at(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.expires_at
            .as_deref()
            .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
            .is_some_and(|expires_at| expires_at <= now)
    }

    pub fn is_expired(&self) -> bool {
        self.is_expired_at(chrono::Utc::now())
    }

    pub fn from_issued(origin: String, issued: TerminalSessionIssued) -> Self {
        Self {
            origin,
            account_id: Some(issued.account_id),
            github_username: issued.github_username,
            terminal_token: issued.terminal_token,
            expires_at: issued.expires_at,
        }
    }
}

fn reveal_optional(vault: &Vault, label: &str) -> Result<Option<String>, SessionStoreError> {
    match vault.reveal(label) {
        Ok(secret) => Ok(Some(secret)),
        Err(VaultError::NotFound { .. }) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn upsert(
    vault: &Vault,
    label: &str,
    secret: String,
    notes: &str,
    origin: &str,
) -> Result<(), SessionStoreError> {
    match vault.add(AddCredential {
        label: label.to_string(),
        credential_type: CredentialType::BearerToken,
        provider: Some("tasknode".to_string()),
        notes: Some(notes.to_string()),
        revocation_notes: Some(format!("{origin}/settings/accounts")),
        secret: secret.clone(),
    }) {
        Ok(()) => Ok(()),
        Err(VaultError::CredentialExists { .. }) => vault
            .update(
                label,
                Some(secret),
                Some(Some("tasknode".to_string())),
                None,
                None,
            )
            .map(|_| ())
            .map_err(Into::into),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests;
