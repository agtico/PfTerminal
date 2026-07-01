//! Disk persistence: reading/writing pane metadata, audits, and restoring panes from disk.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use codex_protocol::ThreadId;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::spawn_orchestration::SpawnRole;

use super::pane::ClaudePane;
use super::pane::ClaudePaneStatus;
use super::pane::PaneLayoutState;
use super::pane::PersistedClaudePaneMetadata;
use super::pane::RestoredClaudePane;
use super::progress_summarize::compact_claude_pane_metadata;
use super::provider::ClaudeProviderProfileKind;
use super::turn_types::ClaudePaneTurnAudit;

pub(crate) const CLAUDE_PANE_METADATA_FILE: &str = "pane.json";
pub(crate) const PANE_METADATA_VERSION: u32 = 1;
pub(crate) fn restore_claude_panes_from_disk(
    codex_home: &Path,
    layout: Option<&PaneLayoutState>,
) -> Vec<RestoredClaudePane> {
    let panes_dir = codex_home.join("panes");
    let Ok(entries) = fs::read_dir(&panes_dir) else {
        return Vec::new();
    };
    let Some(layout_ids) = layout
        .filter(|layout| !layout.claude_pane_ids.is_empty())
        .map(|layout| {
            layout
                .claude_pane_ids
                .iter()
                .cloned()
                .collect::<HashSet<_>>()
        })
    else {
        return Vec::new();
    };

    let mut restored = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !id.starts_with("claude-") {
            continue;
        }
        if !layout_ids.contains(id) {
            continue;
        }
        let Some(restored_pane) = restore_claude_pane_from_dir(id, path.clone()) else {
            continue;
        };
        restored.push(restored_pane);
    }
    restored
}

pub(crate) fn restore_claude_pane_from_dir(
    id: &str,
    artifact_dir: PathBuf,
) -> Option<RestoredClaudePane> {
    let persisted = read_claude_pane_metadata(&artifact_dir);
    let latest_audit = latest_claude_pane_audit(&artifact_dir);
    let max_turn_index = max_turn_index_in_dir(&artifact_dir);
    let (audit, audit_path) = latest_audit
        .as_ref()
        .map(|(audit, path)| (Some(audit), Some(path.clone())))
        .unwrap_or((None, None));

    let profile = persisted
        .as_ref()
        .map(|metadata| metadata.profile)
        .or_else(|| audit.and_then(profile_from_audit))
        .or_else(|| profile_from_settings(&artifact_dir))?;
    let title = persisted
        .as_ref()
        .map(|metadata| metadata.title.clone())
        .or_else(|| audit.map(|audit| audit.pane_title.clone()))
        .unwrap_or_else(|| profile.profile().title.to_string());
    let (fallback_role, fallback_nickname) = spawn_identity_from_title(&title, profile);
    let spawn_role = persisted
        .as_ref()
        .and_then(|metadata| metadata.spawn_role.as_deref())
        .and_then(spawn_role_from_persisted)
        .or(fallback_role);
    let spawn_nickname = persisted
        .as_ref()
        .and_then(|metadata| metadata.spawn_nickname.clone())
        .or(fallback_nickname);
    let spawn_thread_id = persisted
        .as_ref()
        .and_then(|metadata| metadata.spawn_thread_id.as_deref())
        .and_then(|thread_id| ThreadId::from_string(thread_id).ok());
    let cwd = persisted
        .as_ref()
        .map(|metadata| metadata.cwd.clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let latest_audit_path = persisted
        .as_ref()
        .and_then(|metadata| metadata.latest_audit_path.clone())
        .or(audit_path);
    let latest_result_message = persisted
        .as_ref()
        .and_then(|metadata| metadata.latest_result_message.clone())
        .or_else(|| latest_result_message_from_artifacts(&artifact_dir));
    let latest_turn_status = persisted
        .as_ref()
        .and_then(|metadata| metadata.latest_turn_status)
        .or_else(|| audit.map(|audit| audit.status));
    let latest_usage_status = persisted
        .as_ref()
        .and_then(|metadata| metadata.latest_usage_status)
        .or_else(|| audit.map(|audit| audit.usage_status));
    let next_turn_index = persisted
        .as_ref()
        .map(|metadata| metadata.next_turn_index)
        .unwrap_or(1)
        .max(max_turn_index.saturating_add(1));
    let sort_key_ms = persisted_sort_key(&persisted)
        .or_else(|| audit.map(|audit| audit.started_at_unix_ms as i64))
        .or_else(|| dir_modified_unix_ms(&artifact_dir))
        .unwrap_or(0);
    let pane = ClaudePane {
        id: id.to_string(),
        title,
        profile,
        spawn_role,
        spawn_nickname,
        spawn_thread_id,
        cwd,
        claude_session_id: persisted
            .as_ref()
            .and_then(|metadata| metadata.claude_session_id.clone())
            .or_else(|| audit.and_then(|audit| audit.session_id.clone()))
            .or_else(|| latest_session_id_from_artifacts(&artifact_dir)),
        status: ClaudePaneStatus::Idle,
        latest_usage_summary: persisted
            .as_ref()
            .and_then(|metadata| metadata.latest_usage_summary.clone()),
        latest_usage_status,
        latest_turn_status,
        latest_audit_path,
        latest_task_message: persisted
            .as_ref()
            .and_then(|metadata| metadata.latest_task_message.clone()),
        latest_result_message,
        artifact_dir,
        live_turn: None,
        cancel_token: None,
        lock: Arc::new(Mutex::new(())),
        next_turn_index,
    };
    Some(RestoredClaudePane { pane, sort_key_ms })
}

pub(crate) fn read_claude_pane_metadata(
    artifact_dir: &Path,
) -> Option<PersistedClaudePaneMetadata> {
    let path = artifact_dir.join(CLAUDE_PANE_METADATA_FILE);
    let contents = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<PersistedClaudePaneMetadata>(&contents) {
        Ok(metadata) => Some(metadata),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "failed to load Claude pane metadata");
            None
        }
    }
}

pub(crate) fn persist_claude_pane_metadata(pane: &ClaudePane) -> Result<()> {
    fs::create_dir_all(&pane.artifact_dir).with_context(|| {
        format!(
            "failed to create Claude pane artifact directory `{}`",
            pane.artifact_dir.display()
        )
    })?;
    let metadata = PersistedClaudePaneMetadata {
        version: PANE_METADATA_VERSION,
        id: pane.id.clone(),
        title: pane.title.clone(),
        profile: pane.profile,
        spawn_role: pane.spawn_role.map(spawn_role_persisted_value),
        spawn_nickname: pane.spawn_nickname.clone(),
        spawn_thread_id: pane.spawn_thread_id.map(|thread_id| thread_id.to_string()),
        cwd: pane.cwd.clone(),
        claude_session_id: pane.claude_session_id.clone(),
        latest_usage_summary: pane.latest_usage_summary.clone(),
        latest_usage_status: pane.latest_usage_status,
        latest_turn_status: pane.latest_turn_status,
        latest_audit_path: pane.latest_audit_path.clone(),
        latest_task_message: pane.latest_task_message.clone(),
        latest_result_message: pane.latest_result_message.clone(),
        next_turn_index: pane.next_turn_index,
    };
    let path = pane.artifact_dir.join(CLAUDE_PANE_METADATA_FILE);
    let contents = serde_json::to_string_pretty(&metadata)
        .context("failed to serialize Claude pane metadata")?;
    fs::write(&path, contents)
        .with_context(|| format!("failed to write Claude pane metadata `{}`", path.display()))
}

pub(crate) fn latest_claude_pane_audit(
    artifact_dir: &Path,
) -> Option<(ClaudePaneTurnAudit, PathBuf)> {
    let entries = fs::read_dir(artifact_dir).ok()?;
    let mut latest: Option<(u64, ClaudePaneTurnAudit, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("turn-") || !name.ends_with(".audit.json") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(audit) = serde_json::from_str::<ClaudePaneTurnAudit>(&contents) else {
            continue;
        };
        if latest
            .as_ref()
            .is_none_or(|(turn_index, _, _)| audit.turn_index > *turn_index)
        {
            latest = Some((audit.turn_index, audit, path));
        }
    }
    latest.map(|(_, audit, path)| (audit, path))
}

pub(crate) fn max_turn_index_in_dir(artifact_dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(artifact_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(turn_index_from_artifact_name)
        })
        .max()
        .unwrap_or(0)
}

pub(crate) fn turn_index_from_artifact_name(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("turn-")?;
    let digits = rest.split('.').next()?;
    digits.parse().ok()
}

pub(crate) fn profile_from_audit(audit: &ClaudePaneTurnAudit) -> Option<ClaudeProviderProfileKind> {
    ClaudeProviderProfileKind::creation_options()
        .iter()
        .copied()
        .find(|kind| {
            let profile = kind.profile();
            profile.title == audit.provider && profile.provider_model == audit.model
        })
        .or_else(|| {
            ClaudeProviderProfileKind::creation_options()
                .iter()
                .copied()
                .find(|kind| kind.profile().title == audit.provider)
        })
        .or_else(|| legacy_profile_from_audit(audit))
}

pub(crate) fn legacy_profile_from_audit(
    audit: &ClaudePaneTurnAudit,
) -> Option<ClaudeProviderProfileKind> {
    match audit.provider.as_str() {
        "Claude Code - Claude Plan" => Some(ClaudeProviderProfileKind::ClaudePlan),
        _ => None,
    }
}

pub(crate) fn profile_from_settings(artifact_dir: &Path) -> Option<ClaudeProviderProfileKind> {
    let contents = fs::read_to_string(artifact_dir.join("settings.json")).ok()?;
    let value: Value = serde_json::from_str(&contents).ok()?;
    let model = value
        .pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .pointer("/env/ANTHROPIC_MODEL")
                .and_then(Value::as_str)
        })?;
    ClaudeProviderProfileKind::creation_options()
        .iter()
        .copied()
        .find(|kind| kind.profile().provider_model == model)
}

pub(crate) fn spawn_identity_from_title(
    title: &str,
    profile: ClaudeProviderProfileKind,
) -> (Option<SpawnRole>, Option<String>) {
    let Some(prefix) = title_prefix_without_profile_suffix(title, profile) else {
        return (None, None);
    };
    let Some(identity) = prefix.strip_prefix("Claude Code ") else {
        return (None, None);
    };
    if let Some(nickname) = identity.strip_suffix(" [troll]") {
        return (
            Some(SpawnRole::Troll),
            non_empty_string(nickname.to_string()),
        );
    }
    if let Some(nickname) = identity.strip_suffix(" [orc]") {
        return (Some(SpawnRole::Orc), non_empty_string(nickname.to_string()));
    }
    if identity == SpawnRole::Troll.label() {
        return (Some(SpawnRole::Troll), None);
    }
    if identity == SpawnRole::Orc.label() {
        return (Some(SpawnRole::Orc), None);
    }
    (None, None)
}

pub(crate) fn title_prefix_without_profile_suffix(
    title: &str,
    profile: ClaudeProviderProfileKind,
) -> Option<&str> {
    let suffix = format!(" - {}", profile.status_model_label());
    if let Some(prefix) = title.strip_suffix(&suffix) {
        return Some(prefix);
    }
    if profile == ClaudeProviderProfileKind::ClaudePlan {
        return title.strip_suffix(" - Claude Plan");
    }
    None
}

pub(crate) fn spawn_role_persisted_value(role: SpawnRole) -> String {
    role.agent_type()
        .unwrap_or_else(|| role.label())
        .to_string()
}

pub(crate) fn spawn_role_from_persisted(value: &str) -> Option<SpawnRole> {
    match value.to_ascii_lowercase().as_str() {
        "troll" => Some(SpawnRole::Troll),
        "orc" => Some(SpawnRole::Orc),
        "nazgul" => Some(SpawnRole::Nazgul),
        _ => None,
    }
}

pub(crate) fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(crate) fn latest_result_message_from_artifacts(artifact_dir: &Path) -> Option<String> {
    let mut artifacts: Vec<(u64, PathBuf)> = fs::read_dir(artifact_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !name.ends_with(".jsonl") {
                return None;
            }
            turn_index_from_artifact_name(&name).map(|turn_index| (turn_index, path))
        })
        .collect();
    artifacts.sort_by(|left, right| right.0.cmp(&left.0));
    artifacts
        .iter()
        .find_map(|(_, path)| latest_result_message_from_artifact(path))
}

pub(crate) fn latest_session_id_from_artifacts(artifact_dir: &Path) -> Option<String> {
    let mut artifacts: Vec<(u64, PathBuf)> = fs::read_dir(artifact_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !name.ends_with(".jsonl") {
                return None;
            }
            turn_index_from_artifact_name(&name).map(|turn_index| (turn_index, path))
        })
        .collect();
    artifacts.sort_by(|left, right| right.0.cmp(&left.0));
    artifacts
        .iter()
        .find_map(|(_, path)| latest_session_id_from_artifact(path))
}

pub(crate) fn latest_session_id_from_artifact(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let mut latest = None;
    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(session_id) = value.get("session_id").and_then(Value::as_str)
            && !session_id.trim().is_empty()
        {
            latest = Some(session_id.to_string());
        }
    }
    latest
}

pub(crate) fn latest_result_message_from_artifact(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let mut latest = None;
    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("result")
            && let Some(result) = value.get("result").and_then(Value::as_str)
            && !result.trim().is_empty()
        {
            latest = Some(compact_claude_pane_metadata(result, 240));
        }
    }
    latest
}

pub(crate) fn persisted_sort_key(metadata: &Option<PersistedClaudePaneMetadata>) -> Option<i64> {
    metadata.as_ref().and_then(|metadata| {
        metadata
            .latest_audit_path
            .as_ref()
            .and_then(|path| dir_modified_unix_ms(path))
    })
}

pub(crate) fn dir_modified_unix_ms(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    unix_ms_i64(modified)
}

#[cfg(test)]
pub(crate) fn current_unix_ms_i64() -> i64 {
    unix_ms_i64(SystemTime::now()).unwrap_or(i64::MAX)
}

pub(crate) fn unix_ms_i64(time: SystemTime) -> Option<i64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_millis().min(i64::MAX as u128) as i64)
}
