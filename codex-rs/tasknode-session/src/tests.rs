use std::sync::Arc;

use codex_keyring_store::tests::MockKeyringStore;
use codex_vault::Vault;
use pretty_assertions::assert_eq;

use super::*;

fn test_vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault = Vault::new_with_keyring_store(
        dir.path().to_path_buf(),
        Arc::new(MockKeyringStore::default()),
    );
    (dir, vault)
}

fn active(token: &str) -> ActiveSession {
    ActiveSession {
        origin: "https://tasknode.example".to_string(),
        account_id: Some("acct_1".to_string()),
        github_username: Some("tester".to_string()),
        terminal_token: token.to_string(),
        expires_at: Some("2027-01-01T00:00:00Z".to_string()),
    }
}

fn pending(request_id: &str) -> PendingLink {
    PendingLink {
        origin: "https://tasknode.example".to_string(),
        request_id: request_id.to_string(),
        poll_token: "poll-secret".to_string(),
        verification_url: format!("https://tasknode.example/auth/{request_id}"),
        started_at: Some("2026-08-07T00:00:00Z".to_string()),
    }
}

/// Regression for the incident class in the failure analysis: beginning a
/// replacement link must not destroy the currently usable credential.
#[test]
fn active_session_survives_link_start() {
    let (_dir, vault) = test_vault();
    promote_active(&vault, &active("tok-live")).expect("seed active");

    save_pending(&vault, &pending("req-1")).expect("start link");

    let state = load(&vault).expect("load");
    assert_eq!(
        state.active.as_ref().map(|s| s.terminal_token.as_str()),
        Some("tok-live"),
        "active token must survive a link start"
    );
    assert_eq!(
        state.pending.as_ref().map(|p| p.request_id.as_str()),
        Some("req-1")
    );
}

#[test]
fn abandoned_link_is_non_destructive() {
    let (_dir, vault) = test_vault();
    promote_active(&vault, &active("tok-live")).expect("seed active");
    save_pending(&vault, &pending("req-1")).expect("start link");

    assert!(clear_pending(&vault).expect("clear pending"));

    let state = load(&vault).expect("load");
    assert_eq!(
        state.active.map(|s| s.terminal_token),
        Some("tok-live".to_string())
    );
    assert_eq!(state.pending, None);
}

#[test]
fn promotion_replaces_active_and_clears_pending() {
    let (_dir, vault) = test_vault();
    promote_active(&vault, &active("tok-old")).expect("seed active");
    save_pending(&vault, &pending("req-2")).expect("start link");

    promote_active(&vault, &active("tok-new")).expect("promote");

    let state = load(&vault).expect("load");
    assert_eq!(
        state.active.map(|s| s.terminal_token),
        Some("tok-new".to_string())
    );
    assert_eq!(
        state.pending, None,
        "promotion consumes the pending attempt"
    );
}

/// Pre-split blobs with a token load as an active session unchanged.
#[test]
fn legacy_active_record_loads() {
    let (_dir, vault) = test_vault();
    let legacy = serde_json::json!({
        "origin": "https://tasknode.example",
        "account_id": "acct_9",
        "github_username": "legacy-user",
        "terminal_token": "tok-legacy",
        "expires_at": null,
        "pending_request_id": null,
        "pending_poll_token": null,
        "pending_verification_url": null,
    });
    vault
        .add(codex_vault::AddCredential {
            label: TASKNODE_ACTIVE_SESSION_LABEL.to_string(),
            credential_type: codex_vault::CredentialType::BearerToken,
            provider: Some("tasknode".to_string()),
            notes: None,
            revocation_notes: None,
            secret: legacy.to_string(),
        })
        .expect("seed legacy");

    let state = load(&vault).expect("load");
    assert_eq!(
        state.active.map(|s| (s.terminal_token, s.account_id)),
        Some(("tok-legacy".to_string(), Some("acct_9".to_string())))
    );
    assert_eq!(state.pending, None);
}

/// A pre-split pending-only blob (the corrupted state the old flow produced)
/// migrates to the pending label and stops occupying the active label.
#[test]
fn legacy_pending_only_record_migrates() {
    let (_dir, vault) = test_vault();
    let legacy = serde_json::json!({
        "origin": "https://tasknode.example",
        "terminal_token": null,
        "pending_request_id": "req-legacy",
        "pending_poll_token": "poll-legacy",
        "pending_verification_url": "https://tasknode.example/auth/req-legacy",
    });
    vault
        .add(codex_vault::AddCredential {
            label: TASKNODE_ACTIVE_SESSION_LABEL.to_string(),
            credential_type: codex_vault::CredentialType::BearerToken,
            provider: Some("tasknode".to_string()),
            notes: None,
            revocation_notes: None,
            secret: legacy.to_string(),
        })
        .expect("seed legacy");

    let state = load(&vault).expect("load");
    assert_eq!(state.active, None);
    assert_eq!(
        state.pending.map(|p| (p.request_id, p.poll_token)),
        Some(("req-legacy".to_string(), "poll-legacy".to_string()))
    );

    // A new active session can now be installed cleanly.
    promote_active(&vault, &active("tok-fresh")).expect("promote");
    let state = load(&vault).expect("reload");
    assert_eq!(
        state.active.map(|s| s.terminal_token),
        Some("tok-fresh".to_string())
    );
    assert_eq!(state.pending, None);
}

#[test]
fn clear_all_unlinks_everything() {
    let (_dir, vault) = test_vault();
    promote_active(&vault, &active("tok")).expect("seed");
    save_pending(&vault, &pending("req")).expect("pending");

    clear_all(&vault).expect("clear");

    assert_eq!(load(&vault).expect("load"), LocalState::default());
}

#[test]
fn state_summary_never_contains_secrets() {
    let (_dir, vault) = test_vault();
    promote_active(&vault, &active("tok-secret-value")).expect("seed");
    save_pending(&vault, &pending("req-1")).expect("pending");

    let summary = state_summary(&load(&vault).expect("load")).to_string();
    assert!(!summary.contains("tok-secret-value"));
    assert!(!summary.contains("poll-secret"));
    assert!(summary.contains("req-1"));
}

/// The field scenario from 2026-08-07: a daily-TTL-expired active session must
/// not be treated as usable, and must not block completing a pending link.
#[test]
fn expired_active_session_is_detected() {
    let mut session = active("tok-expired");
    session.expires_at = Some("2026-08-07T13:07:07.100Z".to_string());
    let after = chrono::DateTime::parse_from_rfc3339("2026-08-07T17:00:00Z")
        .expect("parse")
        .with_timezone(&chrono::Utc);
    let before = chrono::DateTime::parse_from_rfc3339("2026-08-07T10:00:00Z")
        .expect("parse")
        .with_timezone(&chrono::Utc);
    assert!(session.is_expired_at(after));
    assert!(!session.is_expired_at(before));
}

/// Missing or malformed expiry metadata must never lock a user out.
#[test]
fn absent_or_invalid_expiry_counts_as_fresh() {
    let now = chrono::Utc::now();
    let mut session = active("tok");
    session.expires_at = None;
    assert!(!session.is_expired_at(now));
    session.expires_at = Some("not-a-date".to_string());
    assert!(!session.is_expired_at(now));
}

#[test]
fn state_summary_reports_expiry() {
    let (_dir, vault) = test_vault();
    let mut session = active("tok");
    session.expires_at = Some("2000-01-01T00:00:00Z".to_string());
    promote_active(&vault, &session).expect("seed");
    let summary = state_summary(&load(&vault).expect("load"));
    assert_eq!(
        summary
            .get("activeSession")
            .and_then(|active| active.get("expired"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}
