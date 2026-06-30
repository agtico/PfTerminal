use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use codex_app_server_protocol::UserInput;
use codex_model_provider_info::AMBIENT_DEFAULT_MODEL;
use codex_model_provider_info::AMBIENT_KIMI_K2_7_CODE_MODEL;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app_command::AppCommand;
use crate::spawn_orchestration::SpawnRole;

use super::app_integration::new_pane_items;
use super::bridge::ambient_retry_after_delay;
use super::bridge_translate::ambient_chat_messages_from_claude_request;
use super::bridge_translate::ambient_chat_tools_from_claude_request;
use super::bridge_translate::anthropic_stream_error_event;
use super::bridge_translate::anthropic_stream_start_event;
use super::bridge_translate::anthropic_stream_stop_event;
use super::bridge_translate::anthropic_tool_use_response;
use super::bridge_translate::bridge_tool_calls_from_ambient_response;
use super::command_plan::allowed_provider_vault_label;
use super::command_plan::build_claude_command_plan;
use super::command_plan::claude_pane_title;
use super::command_plan::compose_claude_pane_prompt;
use super::command_plan::prompt_from_user_turn;
use super::command_plan::settings_json_with_base_url;
#[cfg(unix)]
use super::execution::ClaudeSecretRedactor;
use super::execution::failed_turn_output;
use super::execution::partial_failed_turn_output;
use super::execution::run_claude_command_plan;
use super::execution::stop_claude_child;
use super::execution::write_turn_audit;
use super::output_parse::parse_claude_output;
use super::output_parse::parsed_from_value;
use super::pane::ClaudeCommandMode;
use super::pane::ClaudePane;
use super::pane::ClaudePaneLiveTurn;
use super::pane::ClaudePaneStatus;
use super::pane::ClaudePaneTurnStatus;
use super::pane::ClaudePaneUsageStatus;
use super::pane::PaneLayoutState;
use super::persistence::CLAUDE_PANE_METADATA_FILE;
use super::persistence::current_unix_ms_i64;
use super::progress::progress_from_claude_value;
use super::progress::progress_status_text;
use super::progress::progresses_from_claude_value;
use super::progress::usage_status_from_summary;
use super::progress_summarize::summarize_tool_call_input;
use super::provider::ClaudeProviderProfileKind;
use super::registry::CODEX_MAIN_PANE_ID;
use super::registry::ClaudePaneRegistry;
use super::registry::PANE_LAYOUT_VERSION;
use super::registry::load_pane_layout;
use super::registry::persist_pane_layout;
use super::smoke_workflows::smoke_provider_profile;
use super::turn_types::ClaudeBridgeKind;
#[cfg(unix)]
use super::turn_types::ClaudeBridgePlan;
use super::turn_types::ClaudeCommandPlan;
use super::turn_types::ClaudePaneReasoningEvent;
use super::turn_types::ClaudePaneToolEvent;
use super::turn_types::ClaudePaneTurnOutput;
use super::turn_types::ClaudePaneTurnProgress;

use std::path::PathBuf;
use tokio::process::Command;

// Re-export items the test helpers use with their original unqualified names.

fn pane(profile: ClaudeProviderProfileKind) -> (tempfile::TempDir, ClaudePane) {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = Uuid::new_v4().to_string();
    let artifact_dir = dir.path().join("panes").join(&id);
    std::fs::create_dir_all(&artifact_dir).expect("artifact dir");
    (
        dir,
        ClaudePane {
            id: format!("claude-{id}"),
            title: profile.profile().title.to_string(),
            profile,
            spawn_role: None,
            spawn_nickname: None,
            cwd: std::env::current_dir().expect("cwd"),
            claude_session_id: None,
            status: ClaudePaneStatus::Idle,
            latest_usage_summary: None,
            latest_usage_status: None,
            latest_turn_status: None,
            latest_audit_path: None,
            latest_task_message: None,
            latest_result_message: None,
            artifact_dir,
            live_turn: None,
            cancel_token: None,
            lock: Arc::new(Mutex::new(())),
            next_turn_index: 1,
        },
    )
}

#[test]
fn registry_restores_persisted_pane_metadata() {
    let codex_home = tempfile::tempdir().expect("codex home");
    let cwd = std::env::current_dir().expect("cwd");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane_with_role(
            ClaudeProviderProfileKind::ClaudePlan,
            cwd.clone(),
            codex_home.path(),
            Some(SpawnRole::Troll),
            Some("Burzum".to_string()),
        )
        .expect("create pane");
    let pane = registry
        .panes()
        .iter()
        .find(|pane| pane.id == pane_id)
        .expect("pane");
    assert!(pane.artifact_dir.join(CLAUDE_PANE_METADATA_FILE).exists());

    let restored = ClaudePaneRegistry::restore_from_disk(codex_home.path(), None);
    assert!(
        restored.panes().is_empty(),
        "fresh starts should not restore persisted panes without an explicit layout"
    );

    let layout = PaneLayoutState {
        version: PANE_LAYOUT_VERSION,
        codex_thread_id: Some("019f0657-1d67-7103-9d65-89e71587347d".to_string()),
        active_user_pane_id: None,
        spawn_nazgul_pane_id: None,
        claude_pane_ids: vec![pane_id.clone()],
        spawn_parent_by_node: BTreeMap::new(),
    };
    let restored = ClaudePaneRegistry::restore_from_disk(codex_home.path(), Some(&layout));
    assert_eq!(restored.panes().len(), 1);
    let restored_pane = &restored.panes()[0];
    assert_eq!(restored_pane.id, pane_id);
    assert_eq!(restored_pane.profile, ClaudeProviderProfileKind::ClaudePlan);
    assert_eq!(restored_pane.spawn_role, Some(SpawnRole::Troll));
    assert_eq!(restored_pane.spawn_nickname.as_deref(), Some("Burzum"));
    assert_eq!(restored_pane.cwd, cwd);
    assert_eq!(restored.active_user_pane_id(), CODEX_MAIN_PANE_ID);

    let unlisted_pane_id = registry
        .create_pane_with_role(
            ClaudeProviderProfileKind::ClaudePlan,
            cwd,
            codex_home.path(),
            Some(SpawnRole::Orc),
            Some("Snaga".to_string()),
        )
        .expect("create unlisted pane");
    let layout = PaneLayoutState {
        version: PANE_LAYOUT_VERSION,
        codex_thread_id: Some("019f0657-1d67-7103-9d65-89e71587347d".to_string()),
        active_user_pane_id: Some(pane_id.clone()),
        spawn_nazgul_pane_id: None,
        claude_pane_ids: vec![pane_id.clone()],
        spawn_parent_by_node: BTreeMap::new(),
    };
    let restored = ClaudePaneRegistry::restore_from_disk(codex_home.path(), Some(&layout));
    assert_eq!(restored.active_user_pane_id(), pane_id);
    assert_eq!(restored.panes().len(), 1);
    assert!(restored.panes().iter().any(|pane| pane.id == pane_id));
    assert!(
        !restored
            .panes()
            .iter()
            .any(|pane| pane.id == unlisted_pane_id)
    );
}

#[test]
fn registry_restores_legacy_pane_from_latest_audit() {
    let codex_home = tempfile::tempdir().expect("codex home");
    let pane_id = "claude-legacy-pane";
    let artifact_dir = codex_home.path().join("panes").join(pane_id);
    std::fs::create_dir_all(&artifact_dir).expect("artifact dir");
    let artifact_path = artifact_dir.join("turn-0002.jsonl");
    let audit_path = artifact_dir.join("turn-0002.audit.json");
    std::fs::write(
        &artifact_path,
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "result": "legacy pane result text",
            "session_id": "11111111-2222-4333-8444-555555555555"
        })
        .to_string(),
    )
    .expect("artifact");
    std::fs::write(artifact_dir.join("turn-0003.jsonl"), "{}\n").expect("next artifact");
    std::fs::write(
        &audit_path,
        serde_json::json!({
            "pane_id": pane_id,
            "pane_title": "Claude Code Snaga [orc] - GLM 5.2 Fast Vercel",
            "provider": "Claude Code - GLM 5.2 Fast Vercel",
            "model": "zai/glm-5.2-fast",
            "session_id": "11111111-2222-4333-8444-555555555555",
            "turn_index": 2,
            "command_mode": "resume",
            "max_turns": null,
            "artifact_path": artifact_path,
            "audit_path": audit_path,
            "timeout_ms": null,
            "started_at_unix_ms": current_unix_ms_i64(),
            "ended_at_unix_ms": current_unix_ms_i64(),
            "last_progress_elapsed_ms": null,
            "duration_ms": 123,
            "usage": null,
            "usage_status": "untrusted",
            "terminal_reason": null,
            "status": "success",
            "error_summary": null,
            "reasoning_event_count": 0,
            "reasoning_events": [],
            "tool_use_count": 0,
            "tool_names": [],
            "tool_events": []
        })
        .to_string(),
    )
    .expect("audit");

    let layout = PaneLayoutState {
        version: PANE_LAYOUT_VERSION,
        codex_thread_id: Some("019f0657-1d67-7103-9d65-89e71587347d".to_string()),
        active_user_pane_id: None,
        spawn_nazgul_pane_id: None,
        claude_pane_ids: vec![pane_id.to_string()],
        spawn_parent_by_node: BTreeMap::new(),
    };
    let restored = ClaudePaneRegistry::restore_from_disk(codex_home.path(), Some(&layout));
    assert_eq!(restored.panes().len(), 1);
    let pane = &restored.panes()[0];
    assert_eq!(pane.id, pane_id);
    assert_eq!(pane.profile, ClaudeProviderProfileKind::VercelGlm52Fast);
    assert_eq!(pane.spawn_role, Some(SpawnRole::Orc));
    assert_eq!(pane.spawn_nickname.as_deref(), Some("Snaga"));
    assert_eq!(
        pane.claude_session_id.as_deref(),
        Some("11111111-2222-4333-8444-555555555555")
    );
    assert_eq!(pane.latest_turn_status, Some(ClaudePaneTurnStatus::Success));
    assert_eq!(
        pane.latest_usage_status,
        Some(ClaudePaneUsageStatus::Untrusted)
    );
    assert_eq!(
        pane.latest_result_message.as_deref(),
        Some("legacy pane result text")
    );
    assert_eq!(pane.next_turn_index, 4);
}

#[test]
fn registry_restores_session_id_from_artifact_when_interrupted_audit_lost_it() {
    let codex_home = tempfile::tempdir().expect("codex home");
    let pane_id = "claude-interrupted-pane";
    let artifact_dir = codex_home.path().join("panes").join(pane_id);
    std::fs::create_dir_all(&artifact_dir).expect("artifact dir");
    let artifact_path = artifact_dir.join("turn-0001.jsonl");
    let audit_path = artifact_dir.join("turn-0001.audit.json");
    std::fs::write(
        &artifact_path,
        r#"{"type":"system","subtype":"init","session_id":"33333333-3333-4333-8333-333333333333"}
{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]},"session_id":"33333333-3333-4333-8333-333333333333"}"#,
    )
    .expect("artifact");
    std::fs::write(
        &audit_path,
        serde_json::json!({
            "pane_id": pane_id,
            "pane_title": "Claude Code - GLM 5.2 Fast Vercel",
            "provider": "Claude Code - GLM 5.2 Fast Vercel",
            "model": "zai/glm-5.2-fast",
            "session_id": null,
            "turn_index": 1,
            "command_mode": "new-session",
            "max_turns": null,
            "artifact_path": artifact_path,
            "audit_path": audit_path,
            "timeout_ms": null,
            "started_at_unix_ms": current_unix_ms_i64(),
            "ended_at_unix_ms": current_unix_ms_i64(),
            "last_progress_elapsed_ms": null,
            "duration_ms": 123,
            "usage": null,
            "usage_status": "missing",
            "terminal_reason": "interrupted",
            "status": "interrupted",
            "error_summary": "Claude pane turn interrupted by user.",
            "reasoning_event_count": 0,
            "reasoning_events": [],
            "tool_use_count": 0,
            "tool_names": [],
            "tool_events": []
        })
        .to_string(),
    )
    .expect("audit");

    let layout = PaneLayoutState {
        version: PANE_LAYOUT_VERSION,
        codex_thread_id: Some("019f0657-1d67-7103-9d65-89e71587347d".to_string()),
        active_user_pane_id: None,
        spawn_nazgul_pane_id: None,
        claude_pane_ids: vec![pane_id.to_string()],
        spawn_parent_by_node: BTreeMap::new(),
    };
    let mut restored = ClaudePaneRegistry::restore_from_disk(codex_home.path(), Some(&layout));
    assert_eq!(restored.panes().len(), 1);
    let pane = &restored.panes()[0];
    assert_eq!(
        pane.claude_session_id.as_deref(),
        Some("33333333-3333-4333-8333-333333333333")
    );

    codex_vault::Vault::new(codex_home.path().to_path_buf())
        .add(codex_vault::AddCredential {
            label: "provider/ai_gateway_api_key".to_string(),
            credential_type: codex_vault::CredentialType::ApiKey,
            provider: Some("vercel".to_string()),
            notes: None,
            revocation_notes: None,
            secret: "vercel-test-key".to_string(),
        })
        .expect("store test Vercel key");

    let next = restored
        .prepare_turn(pane_id, "continue".to_string(), codex_home.path())
        .expect("next turn");
    assert!(
        next.plan.args.windows(2).any(|window| {
            window[0] == "--resume" && window[1] == "33333333-3333-4333-8333-333333333333"
        }),
        "restored pane should resume the session recovered from the JSONL artifact"
    );
}

#[test]
fn registry_restores_legacy_claude_plan_pane_from_old_audit_title() {
    let codex_home = tempfile::tempdir().expect("codex home");
    let pane_id = "claude-legacy-plan-pane";
    let artifact_dir = codex_home.path().join("panes").join(pane_id);
    std::fs::create_dir_all(&artifact_dir).expect("artifact dir");
    let artifact_path = artifact_dir.join("turn-0001.jsonl");
    let audit_path = artifact_dir.join("turn-0001.audit.json");
    std::fs::write(
        &artifact_path,
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "result": "legacy Claude Plan result",
            "session_id": "22222222-3333-4444-8555-666666666666"
        })
        .to_string(),
    )
    .expect("artifact");
    std::fs::write(
        &audit_path,
        serde_json::json!({
            "pane_id": pane_id,
            "pane_title": "Claude Code Burzum [troll] - Claude Plan",
            "provider": "Claude Code - Claude Plan",
            "model": "sonnet",
            "session_id": "22222222-3333-4444-8555-666666666666",
            "turn_index": 1,
            "command_mode": "new-session",
            "max_turns": null,
            "artifact_path": artifact_path,
            "audit_path": audit_path,
            "timeout_ms": null,
            "started_at_unix_ms": current_unix_ms_i64(),
            "ended_at_unix_ms": current_unix_ms_i64(),
            "last_progress_elapsed_ms": null,
            "duration_ms": 123,
            "usage": null,
            "usage_status": "untrusted",
            "terminal_reason": null,
            "status": "success",
            "error_summary": null,
            "reasoning_event_count": 0,
            "reasoning_events": [],
            "tool_use_count": 0,
            "tool_names": [],
            "tool_events": []
        })
        .to_string(),
    )
    .expect("audit");

    let layout = PaneLayoutState {
        version: PANE_LAYOUT_VERSION,
        codex_thread_id: Some("019f0657-1d67-7103-9d65-89e71587347d".to_string()),
        active_user_pane_id: None,
        spawn_nazgul_pane_id: None,
        claude_pane_ids: vec![pane_id.to_string()],
        spawn_parent_by_node: BTreeMap::new(),
    };
    let restored = ClaudePaneRegistry::restore_from_disk(codex_home.path(), Some(&layout));
    assert_eq!(restored.panes().len(), 1);
    let pane = &restored.panes()[0];
    assert_eq!(pane.id, pane_id);
    assert_eq!(pane.profile, ClaudeProviderProfileKind::ClaudePlan);
    assert_eq!(pane.spawn_role, Some(SpawnRole::Troll));
    assert_eq!(pane.spawn_nickname.as_deref(), Some("Burzum"));
    assert_eq!(
        pane.claude_session_id.as_deref(),
        Some("22222222-3333-4444-8555-666666666666")
    );
    assert_eq!(
        pane.latest_result_message.as_deref(),
        Some("legacy Claude Plan result")
    );
    assert_eq!(pane.next_turn_index, 2);
}

#[test]
fn pane_layout_persistence_round_trips_root_binding_and_parent_map() {
    let codex_home = tempfile::tempdir().expect("codex home");
    let mut parents = BTreeMap::new();
    parents.insert("pane:orc".to_string(), "pane:troll".to_string());
    let layout = PaneLayoutState {
        version: 0,
        codex_thread_id: Some("019f0657-1d67-7103-9d65-89e71587347d".to_string()),
        active_user_pane_id: Some("claude-active".to_string()),
        spawn_nazgul_pane_id: Some("claude-root".to_string()),
        claude_pane_ids: vec!["claude-root".to_string(), "claude-active".to_string()],
        spawn_parent_by_node: parents.clone(),
    };

    persist_pane_layout(codex_home.path(), &layout).expect("persist layout");
    let restored = load_pane_layout(
        codex_home.path(),
        Some("019f0657-1d67-7103-9d65-89e71587347d"),
    )
    .expect("layout");
    assert_eq!(restored.version, PANE_LAYOUT_VERSION);
    assert_eq!(restored.codex_thread_id, layout.codex_thread_id);
    assert_eq!(
        restored.active_user_pane_id.as_deref(),
        Some("claude-active")
    );
    assert_eq!(
        restored.spawn_nazgul_pane_id.as_deref(),
        Some("claude-root")
    );
    assert_eq!(restored.claude_pane_ids, layout.claude_pane_ids);
    assert_eq!(restored.spawn_parent_by_node, parents);
}

#[test]
fn pane_layout_persistence_is_thread_scoped() {
    let codex_home = tempfile::tempdir().expect("codex home");
    let first_thread = "019f0657-1d67-7103-9d65-89e71587347d";
    let second_thread = "019f0e22-e6e9-7e02-9cca-9dc18667b3e5";
    let first_layout = PaneLayoutState {
        version: 0,
        codex_thread_id: Some(first_thread.to_string()),
        active_user_pane_id: Some("claude-first".to_string()),
        spawn_nazgul_pane_id: None,
        claude_pane_ids: vec!["claude-first".to_string()],
        spawn_parent_by_node: BTreeMap::new(),
    };
    let second_layout = PaneLayoutState {
        version: 0,
        codex_thread_id: Some(second_thread.to_string()),
        active_user_pane_id: Some("claude-second".to_string()),
        spawn_nazgul_pane_id: None,
        claude_pane_ids: vec!["claude-second".to_string()],
        spawn_parent_by_node: BTreeMap::new(),
    };

    persist_pane_layout(codex_home.path(), &first_layout).expect("persist first layout");
    persist_pane_layout(codex_home.path(), &second_layout).expect("persist second layout");

    let first_restored = load_pane_layout(codex_home.path(), Some(first_thread)).expect("first");
    let second_restored = load_pane_layout(codex_home.path(), Some(second_thread)).expect("second");
    assert_eq!(first_restored.claude_pane_ids, vec!["claude-first"]);
    assert_eq!(second_restored.claude_pane_ids, vec!["claude-second"]);
    assert!(load_pane_layout(codex_home.path(), None).is_none());
}

#[test]
fn settings_json_uses_helper_without_secret_material() {
    let profile = ClaudeProviderProfileKind::ZaiGlm52.profile();
    let settings = settings_json_with_base_url(profile, Some("pfterminal"), None);
    let rendered = settings.to_string();

    assert!(rendered.contains("https://api.z.ai/api/anthropic"));
    assert!(rendered.contains("glm-5.2[1m]"));
    assert!(rendered.contains("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS"));
    assert!(rendered.contains("pfterminal vault auth-helper provider/zai_api_key"));
    assert!(!rendered.contains("zai-secret"));
}

#[test]
fn direct_provider_plan_uses_auth_helper_without_secret_env() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ZaiGlm52);
    codex_vault::Vault::new(dir.path().to_path_buf())
        .add(codex_vault::AddCredential {
            label: "provider/zai_api_key".to_string(),
            credential_type: codex_vault::CredentialType::ApiKey,
            provider: Some("zai".to_string()),
            notes: None,
            revocation_notes: None,
            secret: "zai-test-key".to_string(),
        })
        .expect("store test Z.AI key");

    let plan = build_claude_command_plan(&pane, "hello".to_string(), dir.path()).expect("plan");
    let settings = std::fs::read_to_string(pane.artifact_dir.join("settings.json"))
        .expect("settings should be written");
    let settings: Value = serde_json::from_str(&settings).expect("settings json");

    assert!(plan.bridge.is_none());
    assert_eq!(
        settings.pointer("/apiKeyHelper"),
        Some(&json!("pfterminal vault auth-helper provider/zai_api_key"))
    );
    assert!(plan.env_remove.iter().any(|key| key == "ANTHROPIC_API_KEY"));
    assert!(
        plan.env_remove
            .iter()
            .any(|key| key == "ANTHROPIC_AUTH_TOKEN")
    );
    assert_eq!(
        plan.env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("")
    );
    assert!(!plan.env.contains_key("ANTHROPIC_AUTH_TOKEN"));
    assert!(
        !plan
            .env
            .values()
            .any(|value| value.contains("zai-test-key"))
    );
    assert!(!plan.args.iter().any(|arg| arg.contains("zai-test-key")));
    assert!(!settings.to_string().contains("zai-test-key"));
}

#[test]
fn tool_call_previews_are_readable_blurbs() {
    let bash_with_description = json!({
        "command": "mkdir -p /tmp/gemology-mock && echo ok",
        "description": "Create directory for gemology website mock"
    });
    assert_eq!(
        summarize_tool_call_input("Bash", &bash_with_description),
        "Create directory for gemology website mock"
    );

    let bash_heredoc_without_description = json!({
        "command": "cat > /tmp/gemology-mock/index.html <<'HTMLEOF'\n<!DOCTYPE html>\n<html><body>large page body</body></html>\nHTMLEOF"
    });
    assert_eq!(
        summarize_tool_call_input("Bash", &bash_heredoc_without_description),
        "writing index.html"
    );

    let bash_redirect_with_fd_dup = json!({
        "command": "npm test > /tmp/test-output.log 2>&1"
    });
    assert_eq!(
        summarize_tool_call_input("Bash", &bash_redirect_with_fd_dup),
        "writing test-output.log"
    );

    let bash_dev_null_redirect = json!({
        "command": "npm test > /dev/null 2>&1"
    });
    assert_eq!(
        summarize_tool_call_input("Bash", &bash_dev_null_redirect),
        "npm test > /dev/null 2>&1"
    );

    let edit = json!({
        "file_path": "src/app.rs",
        "old_string": "before",
        "new_string": "after"
    });
    assert_eq!(
        summarize_tool_call_input("Edit", &edit),
        "editing src/app.rs"
    );

    let read = json!({ "file_path": "README.md" });
    assert_eq!(
        summarize_tool_call_input("Read", &read),
        "reading README.md"
    );
}

#[test]
fn tool_call_progress_uses_blurb_not_raw_json_preview() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan =
        build_claude_command_plan(&pane, "make a mock".to_string(), dir.path()).expect("plan");
    let started_at = Instant::now();
    let value = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": {
                    "command": "cat > /tmp/gemology-mock/index.html <<'HTMLEOF'\n<!DOCTYPE html>\n<html>blob</html>\nHTMLEOF"
                }
            }]
        }
    });

    let progress = progress_from_claude_value(&plan, &started_at, &value).expect("tool progress");
    assert_eq!(
        progress.summary,
        "Claude tool call: Bash: writing index.html"
    );
    assert_eq!(progress.hint, None);
    assert!(!progress.summary.contains("{\"command\""));
    assert!(!progress.summary.contains("<!DOCTYPE html>"));
}

#[test]
fn reasoning_progress_uses_thinking_blocks() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan = build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
    let started_at = Instant::now();
    let value = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "thinking",
                "thinking": "I need to inspect the Troll and Orc hierarchy before assigning work."
            }]
        }
    });

    let progress =
        progress_from_claude_value(&plan, &started_at, &value).expect("reasoning progress");
    assert_eq!(progress.phase, "reasoning");
    assert_eq!(
        progress.summary,
        "Claude reasoning: I need to inspect the Troll and Orc hierarchy before assigning work."
    );
    assert_eq!(progress.hint, None);
}

#[test]
fn thinking_token_system_events_render_as_reasoning_progress() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan = build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
    let started_at = Instant::now();
    let value = json!({
        "type": "system",
        "subtype": "thinking_tokens",
        "estimated_tokens": 3136,
        "estimated_tokens_delta": 1,
        "session_id": "11111111-2222-4333-8444-555555555555"
    });

    let progress =
        progress_from_claude_value(&plan, &started_at, &value).expect("thinking progress");
    assert_eq!(progress.phase, "reasoning-tokens");
    assert_eq!(
        progress.summary,
        "Claude reasoning: thinking: 3.1K reasoning tokens"
    );
    assert_eq!(progress.hint.as_deref(), Some("thinking-token-bucket:31"));
    assert_ne!(progress_status_text(&progress), "session initialized");
}

#[test]
fn assistant_text_progress_carries_streaming_delta() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan = build_claude_command_plan(&pane, "dispatch".to_string(), dir.path()).expect("plan");
    let started_at = Instant::now();
    let value = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "text",
                "text": "```pfterminal-send-task\ntarget: Snaga\ntask:\nbuild site\n```"
            }]
        }
    });

    let progress = progresses_from_claude_value(&plan, &started_at, &value)
        .into_iter()
        .find(|progress| progress.phase == "assistant-text")
        .expect("assistant text progress");
    assert_eq!(
        progress.assistant_text_delta.as_deref(),
        Some("```pfterminal-send-task\ntarget: Snaga\ntask:\nbuild site\n```")
    );
}

#[test]
fn streaming_assistant_dispatch_blocks_are_collected_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("pane");

    let first = registry.collect_spawn_dispatches_from_assistant_delta(
        &pane_id,
        "Before ```pfterminal-send-task\ntarget: Snaga\ntask:\nbuild",
    );
    assert!(first.is_empty());

    let second =
        registry.collect_spawn_dispatches_from_assistant_delta(&pane_id, " site\n``` after");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].target, "Snaga");
    assert_eq!(second[0].task, "build site");

    let duplicate = registry.collect_spawn_dispatches_from_assistant_delta(
        &pane_id,
        "```pfterminal-send-task\ntarget: Snaga\ntask:\nbuild site\n```",
    );
    assert!(duplicate.is_empty());
}

#[test]
fn streaming_xmlish_dispatch_preserves_code_fences_until_close_tag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("pane");

    let first = registry.collect_spawn_dispatches_from_assistant_delta(
            &pane_id,
            "Before <pfterminal_send_task target=\"Burzum\">\nProblem A:\n```systemd\nExecStart=/bin/postfiat",
        );
    assert!(first.is_empty());

    let second = registry.collect_spawn_dispatches_from_assistant_delta(
        &pane_id,
        "\n```\nProblem B: verify writes.\n</pfterminal_send_task> after",
    );
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].target, "Burzum");
    assert!(second[0].task.contains("Problem A:"));
    assert!(second[0].task.contains("```systemd"));
    assert!(second[0].task.contains("ExecStart=/bin/postfiat"));
    assert!(second[0].task.contains("Problem B: verify writes."));

    let duplicate = registry.collect_spawn_dispatches_from_assistant_delta(
            &pane_id,
            "<pfterminal_send_task target=\"Burzum\">\nProblem A:\n```systemd\nExecStart=/bin/postfiat\n```\nProblem B: verify writes.\n</pfterminal_send_task>",
        );
    assert!(duplicate.is_empty());
}

#[test]
fn visible_assistant_transcript_delta_hides_dispatch_payloads() {
    let mut live_turn = ClaudePaneLiveTurn::starting();
    live_turn
        .assistant_commentary_buffer
        .push_str("I reviewed the failure and will assign it now.\n");
    assert_eq!(
        live_turn.take_visible_assistant_transcript_delta(),
        Some("I reviewed the failure and will assign it now.".to_string())
    );

    live_turn.assistant_commentary_buffer.push_str(
        "<pfterminal_send_task target=\"Ghash\">\nfix the proxy comment\n</pfterminal_send_task>\n",
    );
    assert_eq!(live_turn.take_visible_assistant_transcript_delta(), None);

    live_turn
        .assistant_commentary_buffer
        .push_str("Task queued; I am waiting for the report.");
    let delta = live_turn
        .take_visible_assistant_transcript_delta()
        .expect("post-dispatch commentary delta");
    assert!(delta.contains("Task queued; I am waiting for the report."));
    assert!(!delta.contains("pfterminal_send_task"));
    assert!(!delta.contains("fix the proxy comment"));
}

#[test]
fn live_status_panel_tracks_assistant_commentary_without_dispatch_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("pane");
    let artifact_path = dir.path().join("turn-0001.jsonl");
    let audit_path = dir.path().join("turn-0001.audit.json");

    let commentary = ClaudePaneTurnProgress {
        pane_id: pane_id.clone(),
        phase: "assistant-text".to_string(),
        summary: "Claude assistant text.".to_string(),
        assistant_text_delta: Some(
            "Let me trace the allow flags and wrap_owned relationship.".to_string(),
        ),
        hint: None,
        elapsed_ms: 25_000,
        artifact_path: artifact_path.clone(),
        audit_path: audit_path.clone(),
    };
    let status = registry.update_live_progress(&commentary).expect("status");
    let details = status.details.expect("details");
    assert!(details.contains(
        "Current: Claude note: Let me trace the allow flags and wrap_owned relationship."
    ));
    assert!(details.contains("Claude notes:"));
    assert!(details.contains("Let me trace the allow flags and wrap_owned relationship."));
    assert!(!details.contains("artifact:"));
    assert!(!details.contains("audit:"));

    let partial_dispatch = ClaudePaneTurnProgress {
        pane_id: pane_id.clone(),
        phase: "assistant-text".to_string(),
        summary: "Claude assistant text.".to_string(),
        assistant_text_delta: Some(
            "\nDispatching to Snaga.\n```pfterminal-send-task\ntarget: Snaga\n".to_string(),
        ),
        hint: None,
        elapsed_ms: 30_000,
        artifact_path: artifact_path.clone(),
        audit_path: audit_path.clone(),
    };
    let status = registry
        .update_live_progress(&partial_dispatch)
        .expect("partial dispatch status");
    let details = status.details.expect("details");
    assert!(details.contains("Dispatching to Snaga."));
    assert!(!details.contains("pfterminal-send-task"));
    assert!(!details.contains("target: Snaga"));

    let complete_dispatch = ClaudePaneTurnProgress {
        pane_id,
        phase: "assistant-text".to_string(),
        summary: "Claude assistant text.".to_string(),
        assistant_text_delta: Some(
            "task:\nbuild site\n```\nBack to reviewing the result.".to_string(),
        ),
        hint: None,
        elapsed_ms: 35_000,
        artifact_path,
        audit_path,
    };
    let status = registry
        .update_live_progress(&complete_dispatch)
        .expect("complete dispatch status");
    let details = status.details.expect("details");
    assert!(details.contains("Back to reviewing the result."));
    assert!(!details.contains("pfterminal-send-task"));
    assert!(!details.contains("target: Snaga"));
    assert!(!details.contains("build site"));
}

#[test]
fn live_status_panel_does_not_slice_assistant_notes_mid_sentence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("pane");
    let artifact_path = dir.path().join("turn-0001.jsonl");
    let audit_path = dir.path().join("turn-0001.audit.json");
    let repeated = "I checked the RPC allow flags; it succeeded. ".repeat(12);
    let commentary = format!(
        "{repeated}Now npm run build failed because the command ran from the wrong directory. \
             JS tests passed. Python tests failed to find the path, so I am switching cwd before retrying."
    );

    let progress = ClaudePaneTurnProgress {
        pane_id,
        phase: "assistant-text".to_string(),
        summary: "Claude assistant text.".to_string(),
        assistant_text_delta: Some(commentary),
        hint: None,
        elapsed_ms: 69_000,
        artifact_path,
        audit_path,
    };
    let status = registry.update_live_progress(&progress).expect("status");
    assert_eq!(status.header, "Claude running · 1m09s");
    let details = status.details.expect("details");
    assert!(details.contains("Claude notes:"));
    assert!(
        details
            .contains("Now npm run build failed because the command ran from the wrong directory.")
    );
    assert!(details.contains("Python tests failed to find the path"));
    assert!(!details.contains("Current: Claude: s; it succeeded"));
    assert!(!details.contains("\n  s; it succeeded"));
    assert!(!details.contains("artifact:"));
    assert!(!details.contains("audit:"));
}

#[test]
fn live_status_panel_tracks_tools_without_artifact_log_spam() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("pane");
    let artifact_path = dir.path().join("turn-0001.jsonl");
    let audit_path = dir.path().join("turn-0001.audit.json");

    let first_tool = ClaudePaneTurnProgress {
        pane_id: pane_id.clone(),
        phase: "tool-call".to_string(),
        summary:
            "Claude tool call: Bash: Create directory for the mock donkey riding course website"
                .to_string(),
        assistant_text_delta: None,
        hint: None,
        elapsed_ms: 30_000,
        artifact_path: artifact_path.clone(),
        audit_path: audit_path.clone(),
    };
    let status = registry.update_live_progress(&first_tool).expect("status");
    assert_eq!(status.header, "Claude running · 30s");
    let details = status.details.expect("details");
    assert!(
        details
            .contains("Current: Bash: Create directory for the mock donkey riding course website")
    );
    assert!(
        details
            .contains("running Bash: Create directory for the mock donkey riding course website")
    );
    assert!(!details.contains("artifact:"));
    assert!(!details.contains("audit:"));

    let heartbeat = ClaudePaneTurnProgress {
        pane_id: pane_id.clone(),
        phase: "waiting".to_string(),
        summary: "Claude running.".to_string(),
        assistant_text_delta: None,
        hint: None,
        elapsed_ms: 90_000,
        artifact_path: artifact_path.clone(),
        audit_path: audit_path.clone(),
    };
    let status = registry.update_live_progress(&heartbeat).expect("status");
    assert_eq!(status.header, "Claude running · 1m30s");
    let details = status.details.expect("details");
    assert!(
        details
            .contains("Current: Bash: Create directory for the mock donkey riding course website")
    );
    assert!(!details.contains("Claude pane still running"));

    let second_tool = ClaudePaneTurnProgress {
        pane_id: pane_id.clone(),
        phase: "tool-call".to_string(),
        summary: "Claude tool call: Bash: Write the donkey riding course mock website HTML file"
            .to_string(),
        assistant_text_delta: None,
        hint: None,
        elapsed_ms: 150_000,
        artifact_path: artifact_path.clone(),
        audit_path: audit_path.clone(),
    };
    let status = registry.update_live_progress(&second_tool).expect("status");
    let details = status.details.expect("details");
    assert!(
        details
            .contains("done    Bash: Create directory for the mock donkey riding course website")
    );
    assert!(
        details.contains("running Bash: Write the donkey riding course mock website HTML file")
    );

    let result = ClaudePaneTurnProgress {
        pane_id,
        phase: "assistant-result".to_string(),
        summary: "Claude returned a result.".to_string(),
        assistant_text_delta: None,
        hint: None,
        elapsed_ms: 180_000,
        artifact_path,
        audit_path,
    };
    let status = registry.update_live_progress(&result).expect("status");
    let details = status.details.expect("details");
    assert!(details.contains("Current: finalizing result"));
    assert!(
        details.contains("done    Bash: Write the donkey riding course mock website HTML file")
    );
}

#[test]
fn live_status_panel_tracks_reasoning_without_artifact_log_spam() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("pane");
    let artifact_path = dir.path().join("turn-0001.jsonl");
    let audit_path = dir.path().join("turn-0001.audit.json");

    let reasoning = ClaudePaneTurnProgress {
        pane_id,
        phase: "reasoning".to_string(),
        summary: "Claude reasoning: Inspect the hierarchy before asking Orcs to execute."
            .to_string(),
        assistant_text_delta: None,
        hint: None,
        elapsed_ms: 12_000,
        artifact_path,
        audit_path,
    };
    let status = registry.update_live_progress(&reasoning).expect("status");
    assert_eq!(status.header, "Claude running · 12s");
    let details = status.details.expect("details");
    assert!(
        details.contains("Current: thinking: Inspect the hierarchy before asking Orcs to execute.")
    );
    assert!(details.contains("Thinking:"));
    assert!(details.contains("Inspect the hierarchy before asking Orcs to execute."));
    assert!(!details.contains("artifact:"));
    assert!(!details.contains("audit:"));
}

#[test]
fn live_status_panel_shows_thinking_tokens_and_marks_prior_tool_done() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("pane");
    let artifact_path = dir.path().join("turn-0001.jsonl");
    let audit_path = dir.path().join("turn-0001.audit.json");

    let tool = ClaudePaneTurnProgress {
        pane_id: pane_id.clone(),
        phase: "tool-call".to_string(),
        summary: "Claude tool call: Bash: Run all Rust tests".to_string(),
        assistant_text_delta: None,
        hint: None,
        elapsed_ms: 30_000,
        artifact_path: artifact_path.clone(),
        audit_path: audit_path.clone(),
    };
    registry.update_live_progress(&tool).expect("tool status");

    let thinking = ClaudePaneTurnProgress {
        pane_id,
        phase: "reasoning-tokens".to_string(),
        summary: "Claude reasoning: thinking: 3.1K reasoning tokens".to_string(),
        assistant_text_delta: None,
        hint: Some("thinking-token-bucket:31".to_string()),
        elapsed_ms: 90_000,
        artifact_path,
        audit_path,
    };
    let status = registry
        .update_live_progress(&thinking)
        .expect("thinking status");

    assert_eq!(status.header, "Claude running · 1m30s");
    let details = status.details.expect("details");
    assert!(details.contains("Current: thinking: 3.1K reasoning tokens"));
    assert!(details.contains("Thinking:"));
    assert!(details.contains("thinking: 3.1K reasoning tokens"));
    assert!(details.contains("done    Bash: Run all Rust tests"));
    assert!(!details.contains("running Bash: Run all Rust tests"));
    assert!(!details.contains("session initialized"));
}

#[test]
fn vercel_profiles_are_creation_options() {
    assert!(
        ClaudeProviderProfileKind::creation_options()
            .contains(&ClaudeProviderProfileKind::VercelGlm52)
    );
    assert!(
        ClaudeProviderProfileKind::creation_options()
            .contains(&ClaudeProviderProfileKind::VercelGlm52Fast)
    );
}

#[test]
fn top_level_new_pane_items_are_collapsed() {
    let items = new_pane_items();
    let names = items
        .iter()
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["+ Codex Pane", "+ Claude Pane"]);
    assert!(
        names
            .iter()
            .all(|name| !name.contains("GLM") && !name.contains("Vercel")),
        "top-level /panes must not list provider-specific Claude rows"
    );
}

#[test]
fn vercel_profile_settings_use_ai_gateway_anthropic_endpoint() {
    let profile = ClaudeProviderProfileKind::VercelGlm52.profile();
    let settings = settings_json_with_base_url(profile, Some("pfterminal"), None);

    assert_eq!(
        settings.pointer("/env/ANTHROPIC_BASE_URL"),
        Some(&json!("https://ai-gateway.vercel.sh"))
    );
    assert_eq!(
        settings.pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL"),
        Some(&json!("zai/glm-5.2"))
    );
    assert_eq!(
        settings.pointer("/env/ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        Some(&json!("zai/glm-5.2-fast"))
    );
    assert_eq!(
        settings.pointer("/apiKeyHelper"),
        Some(&json!(
            "pfterminal vault auth-helper provider/ai_gateway_api_key"
        ))
    );
}

#[test]
fn vercel_fast_profile_uses_fast_model_for_all_claude_aliases() {
    let profile = ClaudeProviderProfileKind::VercelGlm52Fast.profile();
    let settings = settings_json_with_base_url(profile, Some("pfterminal"), None);

    assert_eq!(
        settings.pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL"),
        Some(&json!("zai/glm-5.2-fast"))
    );
    assert_eq!(
        settings.pointer("/env/ANTHROPIC_DEFAULT_SONNET_MODEL"),
        Some(&json!("zai/glm-5.2-fast"))
    );
    assert_eq!(
        settings.pointer("/env/ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        Some(&json!("zai/glm-5.2-fast"))
    );
}

#[test]
fn vercel_fast_command_plan_uses_count_tokens_passthrough_bridge() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::VercelGlm52Fast);
    codex_vault::Vault::new(dir.path().to_path_buf())
        .add(codex_vault::AddCredential {
            label: "provider/ai_gateway_api_key".to_string(),
            credential_type: codex_vault::CredentialType::ApiKey,
            provider: Some("vercel".to_string()),
            notes: None,
            revocation_notes: None,
            secret: "vercel-test-key".to_string(),
        })
        .expect("store test Vercel key");

    let plan = build_claude_command_plan(&pane, "hello".to_string(), dir.path()).expect("plan");
    let settings = std::fs::read_to_string(pane.artifact_dir.join("settings.json"))
        .expect("settings should be written");
    let settings: Value = serde_json::from_str(&settings).expect("settings json");
    let bridge = plan.bridge.as_ref().expect("Vercel should use bridge");

    assert_eq!(bridge.kind, ClaudeBridgeKind::AnthropicPassthrough);
    assert_eq!(bridge.upstream_base_url, "https://ai-gateway.vercel.sh");
    assert_eq!(bridge.upstream_api_key, "vercel-test-key");
    assert_eq!(
        plan.env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
        Some("pfterminal-local-bridge")
    );
    assert!(
        plan.env_remove
            .iter()
            .any(|key| key == "ANTHROPIC_AUTH_TOKEN")
    );
    assert!(
        settings
            .pointer("/env/ANTHROPIC_BASE_URL")
            .and_then(Value::as_str)
            .is_some_and(|base_url| base_url.starts_with("http://127.0.0.1:"))
    );
    assert!(
        !plan
            .env
            .values()
            .any(|value| value.contains("vercel-test-key"))
    );
    assert!(!plan.args.iter().any(|arg| arg.contains("vercel-test-key")));
    assert!(!settings.to_string().contains("vercel-test-key"));
}

#[test]
fn ambient_kimi_profile_uses_ambient_bridge_model() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::AmbientKimiK27);
    codex_vault::Vault::new(dir.path().to_path_buf())
        .add(codex_vault::AddCredential {
            label: "provider/ambient_api_key".to_string(),
            credential_type: codex_vault::CredentialType::ApiKey,
            provider: Some("ambient".to_string()),
            notes: None,
            revocation_notes: None,
            secret: "ambient-test-key".to_string(),
        })
        .expect("store test Ambient key");

    let plan = build_claude_command_plan(&pane, "hello".to_string(), dir.path()).expect("plan");
    let settings = std::fs::read_to_string(pane.artifact_dir.join("settings.json"))
        .expect("settings should be written");
    let settings: Value = serde_json::from_str(&settings).expect("settings json");
    let bridge = plan.bridge.as_ref().expect("Ambient should use bridge");

    assert_eq!(bridge.kind, ClaudeBridgeKind::AmbientChat);
    assert_eq!(bridge.upstream_model, AMBIENT_KIMI_K2_7_CODE_MODEL);
    assert_eq!(bridge.upstream_api_key, "ambient-test-key");
    assert_eq!(
        settings.pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL"),
        Some(&json!(AMBIENT_KIMI_K2_7_CODE_MODEL))
    );
    assert_eq!(
        settings.pointer("/env/ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        Some(&json!(AMBIENT_KIMI_K2_7_CODE_MODEL))
    );
    assert!(!plan.args.iter().any(|arg| arg.contains("ambient-test-key")));
    assert!(!settings.to_string().contains("ambient-test-key"));
}

#[test]
fn ambient_glm_profile_uses_native_ambient_model_slug() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::AmbientGlm52);
    codex_vault::Vault::new(dir.path().to_path_buf())
        .add(codex_vault::AddCredential {
            label: "provider/ambient_api_key".to_string(),
            credential_type: codex_vault::CredentialType::ApiKey,
            provider: Some("ambient".to_string()),
            notes: None,
            revocation_notes: None,
            secret: "ambient-test-key".to_string(),
        })
        .expect("store test Ambient key");

    let plan = build_claude_command_plan(&pane, "hello".to_string(), dir.path()).expect("plan");
    let bridge = plan.bridge.as_ref().expect("Ambient should use bridge");

    assert_eq!(plan.provider_model, AMBIENT_DEFAULT_MODEL);
    assert_eq!(bridge.upstream_model, AMBIENT_DEFAULT_MODEL);
}

#[cfg(unix)]
fn bridge_redaction_plan(
    dir: &tempfile::TempDir,
    command: String,
    secret: &str,
) -> ClaudeCommandPlan {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let bind_addr = listener.local_addr().expect("listener address");
    ClaudeCommandPlan {
        executable: "sh".to_string(),
        args: vec!["-c".to_string(), command],
        env: BTreeMap::new(),
        env_remove: Vec::new(),
        cwd: dir.path().to_path_buf(),
        pane_id: "claude-redaction-test".to_string(),
        pane_title: "Claude Redaction Test".to_string(),
        profile_title: "Claude Redaction Test".to_string(),
        provider_model: "test-model".to_string(),
        turn_index: 1,
        command_mode: ClaudeCommandMode::NewSession,
        command_session_id: "11111111-1111-4111-8111-111111111111".to_string(),
        max_turns: None,
        artifact_path: dir.path().join("turn-0001.jsonl"),
        audit_path: dir.path().join("turn-0001.audit.json"),
        timeout_ms: None,
        bridge: Some(ClaudeBridgePlan {
            kind: ClaudeBridgeKind::AnthropicPassthrough,
            listener,
            bind_addr,
            upstream_base_url: "https://example.invalid".to_string(),
            upstream_api_key: secret.to_string(),
            upstream_model: "test-model".to_string(),
        }),
    }
}

#[cfg(unix)]
#[test]
fn claude_secret_redactor_redacts_bridge_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let plan = bridge_redaction_plan(&dir, "true".to_string(), "bridge-secret-for-redaction-test");

    let redacted =
        ClaudeSecretRedactor::from_plan(&plan).redact("leaked bridge-secret-for-redaction-test");

    assert_eq!(redacted, "leaked [REDACTED_SECRET]");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_secret_is_redacted_from_stdout_artifact_and_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let secret = "vercel-test-key-for-redaction";
    let line = json!({
        "type": "result",
        "subtype": "success",
        "session_id": "11111111-1111-4111-8111-111111111111",
        "result": format!("leaked {secret}"),
    })
    .to_string();
    let plan = bridge_redaction_plan(&dir, format!("printf '%s\\n' '{line}'"), secret);
    let artifact_path = plan.artifact_path.clone();

    let output = run_claude_command_plan(plan, CancellationToken::new(), /*progress_tx*/ None)
        .await
        .expect("turn output");
    let artifact = std::fs::read_to_string(artifact_path).expect("artifact");

    assert_eq!(output.status, ClaudePaneTurnStatus::Success);
    assert_eq!(output.text, "leaked [REDACTED_SECRET]");
    assert!(!artifact.contains(secret));
    assert!(artifact.contains("[REDACTED_SECRET]"));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_secret_is_redacted_from_stderr_failure_and_audit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let secret = "vercel-test-key-for-stderr-redaction";
    let plan = bridge_redaction_plan(
        &dir,
        format!("printf '%s\\n' 'upstream leaked {secret}' >&2; exit 42"),
        secret,
    );
    let audit_path = plan.audit_path.clone();

    let output = run_claude_command_plan(plan, CancellationToken::new(), /*progress_tx*/ None)
        .await
        .expect("turn output");
    let audit = std::fs::read_to_string(audit_path).expect("audit");

    assert_eq!(output.status, ClaudePaneTurnStatus::ProviderError);
    assert_eq!(
        output.error_summary.as_deref(),
        Some("Claude exited with status exit status: 42: upstream leaked [REDACTED_SECRET]")
    );
    assert!(!audit.contains(secret));
    assert!(audit.contains("[REDACTED_SECRET]"));
}

#[test]
fn claude_provider_picker_labels_are_compact() {
    assert_eq!(
        ClaudeProviderProfileKind::AmbientGlm52.status_model_label(),
        "GLM 5.2 Ambient"
    );
    assert_eq!(
        ClaudeProviderProfileKind::AmbientKimiK27.status_model_label(),
        "Kimi K2.7 Ambient"
    );
    assert_eq!(
        ClaudeProviderProfileKind::ClaudePlan.status_model_label(),
        "Opus 4.8 Claude Plan"
    );
}

#[test]
fn smoke_provider_profile_accepts_vercel_aliases() {
    assert_eq!(
        smoke_provider_profile("vercel"),
        Some(ClaudeProviderProfileKind::VercelGlm52)
    );
    assert_eq!(
        smoke_provider_profile("vercel-glm-52-fast"),
        Some(ClaudeProviderProfileKind::VercelGlm52Fast)
    );
}

#[test]
fn smoke_provider_profile_accepts_kimi_aliases() {
    assert_eq!(
        smoke_provider_profile("ambient-kimi-k2-7"),
        Some(ClaudeProviderProfileKind::AmbientKimiK27)
    );
    assert_eq!(
        smoke_provider_profile("kimi-k27"),
        Some(ClaudeProviderProfileKind::AmbientKimiK27)
    );
}

#[test]
fn ambient_profile_is_first_creation_option() {
    assert_eq!(
        ClaudeProviderProfileKind::creation_options()
            .first()
            .copied(),
        Some(ClaudeProviderProfileKind::AmbientGlm52)
    );
    assert!(
        ClaudeProviderProfileKind::creation_options()
            .contains(&ClaudeProviderProfileKind::AmbientKimiK27)
    );
}

#[test]
fn parse_single_json_output() {
    let parsed = parse_claude_output(
            r#"{"type":"result","result":"stored.","session_id":"11111111-2222-4333-8444-555555555555","usage":{"input_tokens":12,"output_tokens":3}}"#,
        )
        .expect("parse");

    assert_eq!(parsed.text, "stored.");
    assert_eq!(parsed.status, ClaudePaneTurnStatus::Success);
    assert_eq!(
        parsed.session_id.as_deref(),
        Some("11111111-2222-4333-8444-555555555555")
    );
    assert_eq!(
        parsed.usage_summary.as_deref(),
        Some(r#"{"input_tokens":12,"output_tokens":3}"#)
    );
}

#[test]
fn parse_stream_json_output_prefers_final_result() {
    let parsed = parse_claude_output(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"he"}],"usage":{"input_tokens":1}}}
{"type":"assistant","message":{"content":[{"type":"text","text":"llo"}]}}
{"type":"result","result":"hello","session_id":"22222222-2222-4222-8222-222222222222"}"#,
        )
        .expect("parse");

    assert_eq!(parsed.text, "hello");
    assert_eq!(parsed.status, ClaudePaneTurnStatus::Success);
    assert_eq!(
        parsed.session_id.as_deref(),
        Some("22222222-2222-4222-8222-222222222222")
    );
    assert_eq!(
        parsed.usage_summary.as_deref(),
        Some(r#"{"input_tokens":1}"#)
    );
}

#[test]
fn parse_stream_json_without_final_result_is_incomplete() {
    let error = parse_claude_output(
            r#"{"type":"system","subtype":"init","session_id":"22222222-2222-4222-8222-222222222222"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Now let me restart the dev server and run the full test:"}]},"session_id":"22222222-2222-4222-8222-222222222222"}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"call_restart","name":"Bash","input":{"command":"pkill -f vite","description":"Kill old vite processes"}}]},"session_id":"22222222-2222-4222-8222-222222222222"}"#,
        )
        .expect_err("dangling tool call without final result should not be success");

    assert!(
        error
            .to_string()
            .contains("ended before a final result event")
    );
}

#[test]
fn parse_stream_json_provider_error_is_structured() {
    let parsed = parse_claude_output(
            r#"{"type":"system","subtype":"init","session_id":"22222222-2222-4222-8222-222222222222"}
{"type":"result","subtype":"success","is_error":true,"result":"API Error: [1305][temporarily overloaded]","session_id":"22222222-2222-4222-8222-222222222222"}"#,
        )
        .expect("provider error should still produce a structured pane result");

    assert_eq!(parsed.status, ClaudePaneTurnStatus::ProviderError);
    assert_eq!(
        parsed.session_id.as_deref(),
        Some("22222222-2222-4222-8222-222222222222")
    );
    assert!(
        parsed
            .error_summary
            .as_deref()
            .unwrap_or_default()
            .contains("temporarily overloaded")
    );
}

#[test]
fn parse_stream_json_max_turns_is_resumable_pause() {
    let parsed = parse_claude_output(
            r#"{"type":"system","subtype":"init","session_id":"33333333-3333-4333-8333-333333333333"}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"README.md"}}],"usage":{"input_tokens":42}}}
{"type":"result","subtype":"error_max_turns","is_error":true,"terminal_reason":"max_turns","result":"Reached maximum number of turns (8)","session_id":"33333333-3333-4333-8333-333333333333"}"#,
        )
        .expect("max-turn should be parsed as a structured pause");

    assert_eq!(parsed.status, ClaudePaneTurnStatus::MaxTurnsPause);
    assert_eq!(
        parsed.session_id.as_deref(),
        Some("33333333-3333-4333-8333-333333333333")
    );
    assert_eq!(parsed.terminal_reason.as_deref(), Some("max_turns"));
    assert_eq!(parsed.tool_names, vec!["Read"]);
}

#[test]
fn zero_usage_summary_is_untrusted_not_reported() {
    assert_eq!(
        usage_status_from_summary(Some(r#"{"input_tokens":0,"output_tokens":0}"#)),
        ClaudePaneUsageStatus::Untrusted
    );
    assert_eq!(
        usage_status_from_summary(Some(r#"{"input_tokens":10,"output_tokens":0}"#)),
        ClaudePaneUsageStatus::Untrusted
    );
    assert_eq!(
        usage_status_from_summary(Some(r#"{"input_tokens":10,"output_tokens":1}"#)),
        ClaudePaneUsageStatus::Reported
    );
}

#[test]
fn timeout_pause_failure_message_is_not_provider_error() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan = build_claude_command_plan(&pane, "hello".to_string(), dir.path()).expect("plan");
    let output = failed_turn_output(
        &plan,
        150_000,
        ClaudePaneTurnStatus::TimeoutPause,
        Some("timeout".to_string()),
        "local timeout".to_string(),
    );

    assert!(output.failure_message().contains("timed out locally"));
    assert!(!output.failure_message().contains("provider error"));
}

#[test]
fn interrupt_turn_cancels_prepared_claude_token_and_finishes_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("pane");

    let prepared = registry
        .prepare_turn(&pane_id, "long running task".to_string(), dir.path())
        .expect("prepared");
    let cancel_token = prepared.cancel_token.clone();
    assert!(!cancel_token.is_cancelled());
    assert!(registry.interrupt_turn(&pane_id).is_ok());
    assert!(cancel_token.is_cancelled());
    assert!(
        registry
            .prepare_turn(&pane_id, "overlap".to_string(), dir.path())
            .is_err(),
        "interrupted turns remain running until the child process exits"
    );
    drop(prepared);

    let result = Ok(ClaudePaneTurnOutput {
        text: String::new(),
        status: ClaudePaneTurnStatus::Interrupted,
        session_id: None,
        usage_summary: None,
        usage_status: ClaudePaneUsageStatus::Missing,
        artifact_path: dir.path().join("turn-0001.jsonl"),
        audit_path: dir.path().join("turn-0001.audit.json"),
        duration_ms: 1,
        terminal_reason: Some("interrupted".to_string()),
        error_summary: Some("Claude pane turn interrupted by user.".to_string()),
        tool_names: Vec::new(),
        tool_events: Vec::new(),
        reasoning_events: Vec::new(),
        command_mode: ClaudeCommandMode::NewSession,
    });
    registry.finish_turn(&pane_id, &result);

    let pane = registry
        .panes()
        .iter()
        .find(|pane| pane.id == pane_id)
        .expect("pane exists");
    assert_eq!(pane.status, ClaudePaneStatus::Idle);
    assert!(pane.cancel_token.is_none());
    assert_eq!(
        pane.latest_turn_status,
        Some(ClaudePaneTurnStatus::Interrupted)
    );

    let next = registry
        .prepare_turn(&pane_id, "next task".to_string(), dir.path())
        .expect("next turn");
    assert!(!next.cancel_token.is_cancelled());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_claude_child_reaps_running_process() {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 60"]);
    command.kill_on_drop(true);
    command.process_group(0);
    let mut child = command.spawn().expect("spawn sleep");

    stop_claude_child(&mut child)
        .await
        .expect("child should be killed and reaped");

    assert!(child.try_wait().expect("query child status").is_some());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_running_command_returns_interrupted_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact_path = dir.path().join("turn-0001.jsonl");
    let audit_path = dir.path().join("turn-0001.audit.json");
    let plan = ClaudeCommandPlan {
            executable: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"55555555-5555-4555-8555-555555555555\"}'; sleep 60".to_string(),
            ],
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            cwd: dir.path().to_path_buf(),
            pane_id: "claude-test".to_string(),
            pane_title: "Claude Test".to_string(),
            profile_title: "Claude Test".to_string(),
            provider_model: "test-model".to_string(),
            turn_index: 1,
            command_mode: ClaudeCommandMode::NewSession,
            command_session_id: "55555555-5555-4555-8555-555555555555".to_string(),
            max_turns: None,
            artifact_path: artifact_path.clone(),
            audit_path: audit_path.clone(),
            timeout_ms: None,
            bridge: None,
        };
    let cancel_token = CancellationToken::new();
    let cancel_handle = cancel_token.clone();
    let runner = tokio::spawn(run_claude_command_plan(plan, cancel_token, None));

    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel_handle.cancel();
    let output = tokio::time::timeout(Duration::from_secs(5), runner)
        .await
        .expect("runner should stop promptly after cancellation")
        .expect("runner join")
        .expect("turn output");

    assert_eq!(output.status, ClaudePaneTurnStatus::Interrupted);
    assert_eq!(output.terminal_reason.as_deref(), Some("interrupted"));
    assert_eq!(
        output.session_id.as_deref(),
        Some("55555555-5555-4555-8555-555555555555")
    );
    assert!(artifact_path.exists());
    assert!(audit_path.exists());
}

#[test]
fn interrupted_partial_output_keeps_planned_session_id_without_stdout_session() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan =
        build_claude_command_plan(&pane, "start task".to_string(), dir.path()).expect("plan");
    let planned_session_id = plan.command_session_id.clone();

    let output = partial_failed_turn_output(
        &plan,
        10,
        ClaudePaneTurnStatus::Interrupted,
        Some("interrupted".to_string()),
        "interrupted by user".to_string(),
        "",
    );

    assert_eq!(output.status, ClaudePaneTurnStatus::Interrupted);
    assert_eq!(
        output.session_id.as_deref(),
        Some(planned_session_id.as_str())
    );
}

#[test]
fn interrupted_partial_output_prefers_stdout_session_id() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan =
        build_claude_command_plan(&pane, "start task".to_string(), dir.path()).expect("plan");
    let stdout =
        r#"{"type":"system","subtype":"init","session_id":"66666666-6666-4666-8666-666666666666"}"#;

    let output = partial_failed_turn_output(
        &plan,
        10,
        ClaudePaneTurnStatus::Interrupted,
        Some("interrupted".to_string()),
        "interrupted by user".to_string(),
        stdout,
    );

    assert_eq!(
        output.session_id.as_deref(),
        Some("66666666-6666-4666-8666-666666666666")
    );
}

#[test]
fn prompt_from_user_turn_rejects_images() {
    let op = AppCommand::UserTurn {
        items: vec![UserInput::Image {
            url: "data:image/png;base64,abc".to_string(),
            detail: None,
        }],
        cwd: PathBuf::from("/tmp"),
        approval_policy: codex_app_server_protocol::AskForApproval::Never,
        approvals_reviewer: None,
        active_permission_profile: None,
        model: "glm-5.2".to_string(),
        effort: None,
        summary: None,
        service_tier: None,
        final_output_json_schema: None,
        collaboration_mode: None,
        personality: None,
    };

    assert!(prompt_from_user_turn(&op).is_err());
}

#[test]
fn compose_claude_pane_prompt_prepends_spawn_context() {
    let prompt = compose_claude_pane_prompt(
        "who are your trolls and orcs".to_string(),
        Some("<pfterminal_spawn_context>\nTrolls: none spawned yet.\n</pfterminal_spawn_context>"),
    );

    assert!(prompt.starts_with("<pfterminal_spawn_context>"));
    assert!(prompt.contains("Trolls: none spawned yet."));
    assert!(prompt.ends_with("User message:\nwho are your trolls and orcs"));
    assert_eq!(
        compose_claude_pane_prompt("hello".to_string(), Some("   ")),
        "hello"
    );
}

#[test]
fn claude_spawn_pane_title_includes_role() {
    assert_eq!(
        claude_pane_title(
            ClaudeProviderProfileKind::VercelGlm52Fast,
            Some(SpawnRole::Troll),
            Some("Burzum")
        ),
        "Claude Code Burzum [troll] - GLM 5.2 Fast Vercel"
    );
    assert_eq!(
        claude_pane_title(
            ClaudeProviderProfileKind::ZaiGlm52,
            Some(SpawnRole::Orc),
            None
        ),
        "Claude Code Orc - GLM 5.2 Z.AI"
    );
    assert_eq!(
        claude_pane_title(ClaudeProviderProfileKind::ClaudePlan, None, None),
        "Claude Code - Opus 4.8 Claude Plan"
    );
}

#[test]
fn create_pane_with_role_sets_spawn_role_and_title() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane_with_role(
            ClaudeProviderProfileKind::ClaudePlan,
            dir.path().to_path_buf(),
            dir.path(),
            Some(SpawnRole::Troll),
            Some("Burzum".to_string()),
        )
        .expect("create pane");
    let pane = registry
        .panes()
        .iter()
        .find(|pane| pane.id == pane_id)
        .expect("pane");

    assert_eq!(pane.spawn_role, Some(SpawnRole::Troll));
    assert_eq!(pane.spawn_nickname.as_deref(), Some("Burzum"));
    assert_eq!(
        pane.title,
        "Claude Code Burzum [troll] - Opus 4.8 Claude Plan"
    );
}

#[test]
fn command_plan_uses_session_id_then_resume_without_secret_in_args() {
    let (dir, mut pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let first =
        build_claude_command_plan(&pane, "hello".to_string(), dir.path()).expect("first plan");
    let first_session_id = first
        .args
        .windows(2)
        .find_map(|w| (w[0] == "--session-id").then(|| w[1].clone()))
        .expect("first plan should start a Claude session");
    assert!(
        Uuid::parse_str(&first_session_id).is_ok(),
        "Claude session id should be a fresh UUID"
    );
    assert_ne!(
        first_session_id,
        pane.id.trim_start_matches("claude-"),
        "fresh Claude session id must not reuse the pane id"
    );
    assert!(
        first
            .args
            .windows(2)
            .any(|w| w[0] == "--output-format" && w[1] == "stream-json")
    );
    assert!(first.args.iter().any(|arg| arg == "--verbose"));
    assert!(!first.args.iter().any(|arg| arg == "--max-turns"));
    assert_eq!(first.max_turns, None);
    assert_eq!(first.timeout_ms, None);
    assert!(
        first
            .args
            .iter()
            .any(|arg| arg == "--exclude-dynamic-system-prompt-sections")
    );
    assert!(
        first
            .args
            .windows(2)
            .any(|w| w[0] == "--effort" && w[1] == "high")
    );
    assert!(
        first
            .args
            .windows(2)
            .any(|w| w[0] == "--setting-sources" && w[1] == "project")
    );
    assert!(!first.args.iter().any(|arg| arg == "--tools"));
    assert!(!first.args.iter().any(|arg| arg.contains("secret")));

    pane.claude_session_id = Some("11111111-2222-4333-8444-555555555555".to_string());
    let second =
        build_claude_command_plan(&pane, "again".to_string(), dir.path()).expect("second plan");
    assert!(
        second
            .args
            .windows(2)
            .any(|w| { w[0] == "--resume" && w[1] == "11111111-2222-4333-8444-555555555555" })
    );
    assert!(!second.args.iter().any(|arg| arg.contains("secret")));
}

#[test]
fn registry_locks_turns_and_resumes_stored_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("create pane");

    let first = registry
        .prepare_turn(&pane_id, "first".to_string(), dir.path())
        .expect("first turn");
    assert!(
        registry
            .prepare_turn(&pane_id, "overlap".to_string(), dir.path())
            .is_err(),
        "a pane must not accept overlapping turns"
    );
    drop(first);

    let result = Ok(ClaudePaneTurnOutput {
        text: "done".to_string(),
        status: ClaudePaneTurnStatus::Success,
        session_id: Some("11111111-2222-4333-8444-555555555555".to_string()),
        usage_summary: None,
        usage_status: ClaudePaneUsageStatus::Missing,
        artifact_path: dir.path().join("turn-0001.jsonl"),
        audit_path: dir.path().join("turn-0001.audit.json"),
        duration_ms: 1,
        terminal_reason: None,
        error_summary: None,
        tool_names: Vec::new(),
        tool_events: Vec::new(),
        reasoning_events: Vec::new(),
        command_mode: ClaudeCommandMode::NewSession,
    });
    registry.finish_turn(&pane_id, &result);

    let second = registry
        .prepare_turn(&pane_id, "second".to_string(), dir.path())
        .expect("second turn");
    assert!(
        second
            .plan
            .args
            .windows(2)
            .any(|w| { w[0] == "--resume" && w[1] == "11111111-2222-4333-8444-555555555555" })
    );
}

#[test]
fn interrupted_turn_with_planned_session_resumes_next_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("create pane");

    let first = registry
        .prepare_turn(&pane_id, "first".to_string(), dir.path())
        .expect("first turn");
    let planned_session_id = first.plan.command_session_id.clone();
    let output = partial_failed_turn_output(
        &first.plan,
        10,
        ClaudePaneTurnStatus::Interrupted,
        Some("interrupted".to_string()),
        "interrupted by user".to_string(),
        "",
    );
    drop(first);

    registry.finish_turn(&pane_id, &Ok(output));

    let second = registry
        .prepare_turn(&pane_id, "second".to_string(), dir.path())
        .expect("second turn");
    assert!(
        second
            .plan
            .args
            .windows(2)
            .any(|w| { w[0] == "--resume" && w[1] == planned_session_id })
    );
    assert!(!second.plan.args.iter().any(|arg| arg == "--session-id"));
}

#[test]
fn provider_error_clears_resume_session_for_next_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = registry
        .create_pane(
            ClaudeProviderProfileKind::ClaudePlan,
            std::env::current_dir().expect("cwd"),
            dir.path(),
        )
        .expect("create pane");
    {
        let pane = registry
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            .expect("pane");
        pane.claude_session_id = Some("11111111-2222-4333-8444-555555555555".to_string());
    }

    let result = Ok(ClaudePaneTurnOutput {
        text: "API Error: The model request was rejected.".to_string(),
        status: ClaudePaneTurnStatus::ProviderError,
        session_id: Some("11111111-2222-4333-8444-555555555555".to_string()),
        usage_summary: None,
        usage_status: ClaudePaneUsageStatus::Untrusted,
        artifact_path: dir.path().join("turn-0001.jsonl"),
        audit_path: dir.path().join("turn-0001.audit.json"),
        duration_ms: 1,
        terminal_reason: Some("completed".to_string()),
        error_summary: Some("model request rejected".to_string()),
        tool_names: Vec::new(),
        tool_events: Vec::new(),
        reasoning_events: Vec::new(),
        command_mode: ClaudeCommandMode::Resume,
    });
    registry.finish_turn(&pane_id, &result);

    let next = registry
        .prepare_turn(&pane_id, "try again".to_string(), dir.path())
        .expect("next turn");
    let next_session_id = next
        .plan
        .args
        .windows(2)
        .find_map(|w| (w[0] == "--session-id").then(|| w[1].clone()))
        .expect("provider-error should force a fresh Claude session");
    assert!(
        Uuid::parse_str(&next_session_id).is_ok(),
        "fresh Claude session should be a UUID"
    );
    assert_ne!(
        next_session_id,
        pane_id.trim_start_matches("claude-"),
        "fresh Claude session must not reuse pane id"
    );
    assert_ne!(
        next_session_id, "11111111-2222-4333-8444-555555555555",
        "fresh Claude session must not reuse failed provider session"
    );
    assert!(!next.plan.args.iter().any(|arg| arg == "--resume"));
}

#[test]
fn max_turn_output_keeps_resume_guidance_and_audit_hint() {
    let (dir, _pane) = pane(ClaudeProviderProfileKind::AmbientGlm52);
    let output = ClaudePaneTurnOutput {
        text: String::new(),
        status: ClaudePaneTurnStatus::MaxTurnsPause,
        session_id: Some("44444444-4444-4444-8444-444444444444".to_string()),
        usage_summary: Some(r#"{"input_tokens":10}"#.to_string()),
        usage_status: ClaudePaneUsageStatus::Reported,
        artifact_path: dir.path().join("turn-0001.jsonl"),
        audit_path: dir.path().join("turn-0001.audit.json"),
        duration_ms: 10,
        terminal_reason: Some("max_turns".to_string()),
        error_summary: Some("Reached maximum number of turns (24)".to_string()),
        tool_names: vec!["Read".to_string()],
        tool_events: vec![ClaudePaneToolEvent {
            name: "Read".to_string(),
            preview: r#"{"file_path":"README.md"}"#.to_string(),
        }],
        reasoning_events: Vec::new(),
        command_mode: ClaudeCommandMode::NewSession,
    };

    assert!(output.failure_message().contains("Type `continue`"));
    let hint = output.audit_hint();
    assert!(hint.contains("status: max-turn-pause"));
    assert!(hint.contains("artifact:"));
    assert!(hint.contains("audit:"));
    assert!(hint.contains("tools: Read"));
}

#[test]
fn turn_audit_serializes_without_prompt_or_secret() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan = build_claude_command_plan(
        &pane,
        "this prompt must not be serialized into audit".to_string(),
        dir.path(),
    )
    .expect("plan");
    let output = failed_turn_output(
        &plan,
        5,
        ClaudePaneTurnStatus::ProviderError,
        Some("provider_error".to_string()),
        "simulated provider failure".to_string(),
    );

    write_turn_audit(&plan, &output, 1, 2, Some(1)).expect("write audit");
    let audit = std::fs::read_to_string(&plan.audit_path).expect("read audit");
    assert!(audit.contains("simulated provider failure"));
    assert!(!audit.contains("this prompt must not be serialized"));
    assert!(!audit.contains("ambient-secret"));
}

#[test]
fn turn_audit_counts_tool_events_not_unique_tool_names() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan = build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
    let output = ClaudePaneTurnOutput {
        text: "done".to_string(),
        status: ClaudePaneTurnStatus::Success,
        session_id: Some("11111111-2222-4333-8444-555555555555".to_string()),
        usage_summary: None,
        usage_status: ClaudePaneUsageStatus::Missing,
        artifact_path: plan.artifact_path.clone(),
        audit_path: plan.audit_path.clone(),
        duration_ms: 10,
        terminal_reason: None,
        error_summary: None,
        tool_names: vec!["Read".to_string(), "Bash".to_string()],
        tool_events: vec![
            ClaudePaneToolEvent {
                name: "Read".to_string(),
                preview: "{}".to_string(),
            },
            ClaudePaneToolEvent {
                name: "Read".to_string(),
                preview: "{}".to_string(),
            },
            ClaudePaneToolEvent {
                name: "Bash".to_string(),
                preview: "{}".to_string(),
            },
        ],
        reasoning_events: Vec::new(),
        command_mode: ClaudeCommandMode::NewSession,
    };

    write_turn_audit(&plan, &output, 1, 2, Some(1)).expect("write audit");
    let audit = std::fs::read_to_string(&plan.audit_path).expect("read audit");
    let audit: Value = serde_json::from_str(&audit).expect("audit json");
    assert_eq!(audit.get("tool_use_count").and_then(Value::as_u64), Some(3));
}

#[test]
fn turn_audit_serializes_reasoning_events() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan = build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
    let output = ClaudePaneTurnOutput {
        text: "done".to_string(),
        status: ClaudePaneTurnStatus::Success,
        session_id: Some("11111111-2222-4333-8444-555555555555".to_string()),
        usage_summary: None,
        usage_status: ClaudePaneUsageStatus::Missing,
        artifact_path: plan.artifact_path.clone(),
        audit_path: plan.audit_path.clone(),
        duration_ms: 10,
        terminal_reason: None,
        error_summary: None,
        tool_names: Vec::new(),
        tool_events: Vec::new(),
        reasoning_events: vec![ClaudePaneReasoningEvent {
            preview: "Inspect Orc output before reporting to the Nazgul.".to_string(),
        }],
        command_mode: ClaudeCommandMode::NewSession,
    };

    write_turn_audit(&plan, &output, 1, 2, Some(1)).expect("write audit");
    let audit = std::fs::read_to_string(&plan.audit_path).expect("read audit");
    let audit: Value = serde_json::from_str(&audit).expect("audit json");
    assert_eq!(
        audit.get("reasoning_event_count").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        audit.pointer("/reasoning_events/0/preview"),
        Some(&json!("Inspect Orc output before reporting to the Nazgul."))
    );
}

#[test]
fn allowed_auth_helper_labels_are_provider_scoped() {
    assert!(allowed_provider_vault_label("provider/zai_api_key"));
    assert!(allowed_provider_vault_label("provider/anthropic_api_key"));
    assert!(allowed_provider_vault_label("provider/ambient_api_key"));
    assert!(allowed_provider_vault_label("provider/baseten_api_key"));
    assert!(allowed_provider_vault_label("provider/openrouter_api_key"));
    assert!(allowed_provider_vault_label("provider/ai_gateway_api_key"));
    assert!(!allowed_provider_vault_label("random"));
}

#[test]
fn parsed_message_content_can_be_nested() {
    let value = json!({
        "type": "assistant",
        "message": {
            "content": [
                {"type": "text", "text": "one"},
                {"type": "tool_use", "name": "Read"}
            ]
        }
    });
    let parsed = parsed_from_value(&value).expect("parse");
    assert_eq!(parsed.text, "one");
}

#[test]
fn parse_stream_json_collects_thinking_blocks() {
    let stdout = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"The Troll should inspect the Orc output before reporting up."}]},"session_id":"11111111-2222-4333-8444-555555555555"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Reviewed."}]}}
{"type":"result","subtype":"success","result":"Reviewed.","session_id":"11111111-2222-4333-8444-555555555555","usage":{"input_tokens":10,"output_tokens":4}}"#;

    let parsed = parse_claude_output(stdout).expect("parse stream");
    assert_eq!(parsed.text, "Reviewed.");
    assert_eq!(parsed.reasoning_events.len(), 1);
    assert_eq!(
        parsed.reasoning_events[0].preview,
        "The Troll should inspect the Orc output before reporting up."
    );
}

#[test]
fn progress_can_emit_reasoning_and_tool_for_one_message() {
    let (dir, pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
    let plan = build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
    let started_at = Instant::now();
    let value = json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "thinking",
                    "thinking": "Read the file before editing."
                },
                {
                    "type": "tool_use",
                    "name": "Read",
                    "input": {"file_path": "README.md"}
                }
            ]
        }
    });

    let progresses = progresses_from_claude_value(&plan, &started_at, &value);
    assert_eq!(progresses.len(), 2);
    assert_eq!(progresses[0].phase, "reasoning");
    assert_eq!(progresses[1].phase, "tool-call");
    assert_eq!(
        progresses[1].summary,
        "Claude tool call: Read: reading README.md"
    );
}

#[test]
fn claude_tools_are_translated_to_ambient_chat_tools() {
    let request = json!({
        "tools": [{
            "name": "Read",
            "description": "Read a file",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }
        }]
    });
    let tools = ambient_chat_tools_from_claude_request(&request);

    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0].pointer("/type").and_then(Value::as_str),
        Some("function")
    );
    assert_eq!(
        tools[0].pointer("/function/name").and_then(Value::as_str),
        Some("Read")
    );
    assert_eq!(
        tools[0]
            .pointer("/function/parameters/required/0")
            .and_then(Value::as_str),
        Some("path")
    );
}

#[test]
fn claude_tool_history_is_translated_to_ambient_chat_messages() {
    let request = json!({
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "Read",
                    "input": { "path": "README.md" }
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": "hello"
                }]
            }
        ]
    });
    let messages = ambient_chat_messages_from_claude_request(&request).expect("messages");

    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0]
            .pointer("/tool_calls/0/id")
            .and_then(Value::as_str),
        Some("toolu_1")
    );
    assert_eq!(
        messages[1].pointer("/tool_call_id").and_then(Value::as_str),
        Some("toolu_1")
    );
    assert_eq!(
        messages[1].pointer("/content").and_then(Value::as_str),
        Some("hello")
    );
}

#[test]
fn ambient_tool_calls_are_translated_to_anthropic_tool_uses() {
    let upstream = json!({
        "choices": [{
            "message": {
                "tool_calls": [
                    {
                        "id": "chatcmpl-tool-1",
                        "type": "function",
                        "function": {
                            "name": "Read",
                            "arguments": "{\"path\":\"README.md\"}"
                        }
                    },
                    {
                        "id": "chatcmpl-tool-2",
                        "type": "function",
                        "function": {
                            "name": "Bash",
                            "arguments": "{\"command\":\"git status --short\"}"
                        }
                    }
                ]
            }
        }]
    });
    let calls = bridge_tool_calls_from_ambient_response(&upstream);
    let response = anthropic_tool_use_response(
        "z-ai/glm-5.2",
        &calls,
        &json!({"prompt_tokens": 5, "cached_tokens": 2, "completion_tokens": 3}),
    );

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "Read");
    assert_eq!(calls[1].name, "Bash");
    assert_eq!(
        response.pointer("/content/0/type").and_then(Value::as_str),
        Some("tool_use")
    );
    assert_eq!(
        response.pointer("/content/1/name").and_then(Value::as_str),
        Some("Bash")
    );
    assert_eq!(
        response.pointer("/stop_reason").and_then(Value::as_str),
        Some("tool_use")
    );
    assert_eq!(
        response
            .pointer("/usage/cache_read_input_tokens")
            .and_then(Value::as_u64),
        Some(2)
    );
}

#[test]
fn ambient_retry_after_delay_parses_seconds_and_caps_large_values() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, "42".parse().expect("header"));
    assert_eq!(
        ambient_retry_after_delay(&headers),
        Some(Duration::from_secs(42))
    );

    headers.insert(reqwest::header::RETRY_AFTER, "999".parse().expect("header"));
    assert_eq!(
        ambient_retry_after_delay(&headers),
        Some(Duration::from_secs(300))
    );
}

#[test]
fn anthropic_stream_events_preserve_upstream_usage_in_protocol_fields() {
    let start = anthropic_stream_start_event(
        "z-ai/glm-5.2",
        &serde_json::json!({
            "prompt_tokens": 120,
            "cached_tokens": 80,
            "completion_tokens": 34
        }),
    );
    let stop = anthropic_stream_stop_event(
        "end_turn",
        &serde_json::json!({
            "prompt_tokens": 120,
            "cached_tokens": 80,
            "completion_tokens": 34
        }),
    );

    assert_eq!(
        start
            .pointer("/message/usage/input_tokens")
            .and_then(Value::as_u64),
        Some(120)
    );
    assert_eq!(
        start
            .pointer("/message/usage/cache_read_input_tokens")
            .and_then(Value::as_u64),
        Some(80)
    );
    assert_eq!(
        start
            .pointer("/message/usage/output_tokens")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        stop.pointer("/usage/output_tokens").and_then(Value::as_u64),
        Some(34)
    );
    assert!(stop.pointer("/usage/input_tokens").is_none());
}

#[test]
fn anthropic_stream_error_event_is_protocol_error() {
    let event = anthropic_stream_error_event("upstream_transport_error", "boom");

    assert_eq!(event.get("type").and_then(Value::as_str), Some("error"));
    assert_eq!(
        event.pointer("/error/type").and_then(Value::as_str),
        Some("upstream_transport_error")
    );
    assert_eq!(
        event.pointer("/error/message").and_then(Value::as_str),
        Some("boom")
    );
    assert!(event.get("content").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires claude CLI and a live provider/ambient_api_key vault credential"]
async fn live_ambient_bridge_runs_claude_headless_for_two_turns() {
    let codex_home = std::env::var("PFTERMINAL_LIVE_CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/postfiat/.pfterminal"));
    let (_dir, mut pane) = pane(ClaudeProviderProfileKind::AmbientGlm52);

    let first_plan = build_claude_command_plan(
        &pane,
        "Reply with exactly: OK-PFTERMINAL-LIVE".to_string(),
        &codex_home,
    )
    .expect("first live plan");
    let first = run_claude_command_plan(first_plan, CancellationToken::new(), None)
        .await
        .expect("first live Claude turn");
    assert!(
        first.text.contains("OK-PFTERMINAL-LIVE"),
        "first turn did not return the requested marker: {}",
        first.text
    );
    pane.claude_session_id = first.session_id;
    pane.next_turn_index = 2;

    let second_plan = build_claude_command_plan(
        &pane,
        "What exact marker did you just return? Reply with only that marker.".to_string(),
        &codex_home,
    )
    .expect("second live plan");
    let second = run_claude_command_plan(second_plan, CancellationToken::new(), None)
        .await
        .expect("second live Claude turn");
    assert!(
        second.text.contains("OK-PFTERMINAL-LIVE"),
        "second turn did not retain session context: {}",
        second.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires claude CLI and a live provider/ambient_api_key vault credential"]
async fn live_ambient_bridge_runs_claude_tool_loop() {
    let codex_home = std::env::var("PFTERMINAL_LIVE_CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/postfiat/.pfterminal"));
    let (_dir, pane) = pane(ClaudeProviderProfileKind::AmbientGlm52);

    let plan = build_claude_command_plan(
            &pane,
            "Use your LS tool to inspect the current working directory. If Cargo.toml is present, reply exactly: FOUND-CARGO-TOML. Do not explain."
                .to_string(),
            &codex_home,
        )
        .expect("tool-loop live plan");
    let output = run_claude_command_plan(plan, CancellationToken::new(), None)
        .await
        .expect("tool-loop live Claude turn");
    assert!(
        output.text.contains("FOUND-CARGO-TOML"),
        "tool loop did not return expected marker: {}",
        output.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires claude CLI and a live provider/ambient_api_key vault credential"]
async fn live_ambient_bridge_runs_substantive_code_review() {
    let codex_home = std::env::var("PFTERMINAL_LIVE_CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/postfiat/.pfterminal"));
    let (_dir, pane) = pane(ClaudeProviderProfileKind::AmbientGlm52);

    let plan = build_claude_command_plan(
        &pane,
        concat!(
            "Perform a read-only code review of codex-rs/tui/src/claude_panes.rs. ",
            "Use filesystem tools to inspect the file. Reply with marker ",
            "PFT_REVIEW_OK and two concrete findings or risks. Do not edit files."
        )
        .to_string(),
        &codex_home,
    )
    .expect("review live plan");
    let output = run_claude_command_plan(plan, CancellationToken::new(), None)
        .await
        .expect("review live Claude turn");
    assert_eq!(output.status, ClaudePaneTurnStatus::Success);
    assert!(
        output.text.contains("PFT_REVIEW_OK"),
        "review did not return expected marker: {}",
        output.text
    );
    assert!(
        !output.tool_names.is_empty(),
        "review should use Claude Code tools; audit: {}",
        output.audit_path.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires claude CLI and a live provider/ambient_api_key vault credential"]
async fn live_ambient_bridge_runs_disposable_edit_task() {
    let codex_home = std::env::var("PFTERMINAL_LIVE_CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/postfiat/.pfterminal"));
    let (dir, mut pane) = pane(ClaudeProviderProfileKind::AmbientGlm52);
    pane.cwd = dir.path().to_path_buf();
    let target = dir.path().join("sample.txt");
    std::fs::write(&target, "before\n").expect("seed fixture");

    let plan = build_claude_command_plan(
            &pane,
            "Edit sample.txt so it contains exactly PFT_EDIT_OK followed by a newline. Then reply exactly: PFT_EDIT_DONE"
                .to_string(),
            &codex_home,
        )
        .expect("edit live plan");
    let output = run_claude_command_plan(plan, CancellationToken::new(), None)
        .await
        .expect("edit live Claude turn");
    assert_eq!(output.status, ClaudePaneTurnStatus::Success);
    assert!(
        output.text.contains("PFT_EDIT_DONE"),
        "edit did not return expected marker: {}",
        output.text
    );
    assert_eq!(
        std::fs::read_to_string(&target).expect("read edited fixture"),
        "PFT_EDIT_OK\n"
    );
}
