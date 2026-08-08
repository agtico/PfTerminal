//! Shared App fixtures for app submodule unit tests.
//!
//! This module keeps heavyweight `App` construction and config-inspection helpers available to
//! focused sibling test modules without making `app/tests.rs` the only practical place to test
//! app-owned behavior.

use super::*;
use crate::chatwidget::tests::make_chatwidget_manual_with_sender;
use codex_models_manager::test_support::construct_model_info_offline_for_tests;
use codex_models_manager::test_support::get_model_offline_for_tests;

pub(super) async fn make_test_app() -> App {
    let (chat_widget, app_event_tx, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
    let config = chat_widget.config_ref().clone();
    let file_search = FileSearchManager::new(config.cwd.to_path_buf(), app_event_tx.clone());
    let model = get_model_offline_for_tests(config.model.as_deref());
    let session_telemetry = test_session_telemetry(&config, model.as_str());

    App {
        model_catalog: chat_widget.model_catalog(),
        session_telemetry,
        app_event_tx,
        chat_widget,
        workspace_command_runner: None,
        launch_cwd: config.cwd.to_path_buf(),
        config,
        state_db: None,
        cli_kv_overrides: Vec::new(),
        harness_overrides: ConfigOverrides::default(),
        loader_overrides: LoaderOverrides::without_managed_config_for_tests(),
        cloud_config_bundle: CloudConfigBundleLoader::default(),
        runtime_approval_policy_override: None,
        runtime_permission_profile_override: None,
        file_search,
        transcript_cells: Vec::new(),
        claude_pane_transcript_cells: HashMap::new(),
        overlay: None,
        deferred_history_lines: Vec::new(),
        has_emitted_history_lines: false,
        transcript_reflow: TranscriptReflowState::default(),
        initial_history_replay_buffer: None,
        scrollback_has_older_history: false,
        enhanced_keys_supported: false,
        keymap: crate::keymap::RuntimeKeymap::defaults(),
        key_chord_matcher: crate::keymap::KeyChordMatcher::default(),
        commit_anim_running: Arc::new(AtomicBool::new(false)),
        status_line_invalid_items_warned: Arc::new(AtomicBool::new(false)),
        terminal_title_invalid_items_warned: Arc::new(AtomicBool::new(false)),
        skill_load_warnings: SkillLoadWarningState::default(),
        backtrack: BacktrackState::default(),
        backtrack_render_pending: false,
        feedback: codex_feedback::CodexFeedback::new(),
        feedback_audience: FeedbackAudience::External,
        environment_manager: Arc::new(EnvironmentManager::default_for_tests()),
        app_server_target: crate::AppServerTarget::Embedded,
        pending_update_action: None,
        pending_shutdown_exit_thread_id: None,
        windows_sandbox: WindowsSandboxState::default(),
        thread_event_channels: HashMap::new(),
        thread_event_listener_tasks: HashMap::new(),
        agent_navigation: AgentNavigationState::default(),
        spawn_parent_by_thread: HashMap::new(),
        spawn_parent_by_node: HashMap::new(),
        spawn_native_runtime_by_node: HashMap::new(),
        spawn_native_endpoint_by_node: HashMap::new(),
        spawn_crew: None,
        spawn_legacy_read_only: false,
        spawn_status_by_thread: HashMap::new(),
        spawn_waiting_for_agents_by_thread: HashMap::new(),
        spawn_parent_reports_by_node: HashMap::new(),
        spawn_dispatch_acks_by_target_task: HashMap::new(),
        spawn_next_dispatch_seq: 1,
        spawn_processed_dispatch_seq_ids: HashSet::new(),
        spawn_processed_dispatch_origins: HashSet::new(),
        spawn_processed_terminal_turns: HashSet::new(),
        spawn_auto_loop_state_by_node: HashMap::new(),
        spawn_operator_input_seen: false,
        spawn_quarantine_notified_by_node: HashSet::new(),
        spawn_context_left_by_thread: HashMap::new(),
        spawn_last_report_seq_by_node: HashMap::new(),
        spawn_last_dispatch_seq_by_node: HashMap::new(),
        spawn_last_event_at_by_node: HashMap::new(),
        spawn_nazgul_pane_id: None,
        spawn_nazgul_rebind_required: false,
        orchestrate_whips: Box::new(HashMap::new()),
        orchestrate_next_whip_seq: 0,
        orchestrate_now_override: None,
        orchestrate_idle_generation_by_target: Box::new(HashMap::new()),
        side_threads: HashMap::new(),
        claude_panes: Default::default(),
        abandoned_side_threads: HashSet::new(),
        active_thread_id: None,
        active_thread_rx: None,
        primary_thread_id: None,
        last_subagent_backfill_attempt: None,
        primary_session_configured: None,
        pending_primary_events: VecDeque::new(),
        pending_app_server_requests: PendingAppServerRequests::default(),
        pending_startup_thread_start: false,
        rate_limit_hard_stop_generation: 0,
        pending_plugin_enabled_writes: HashMap::new(),
        pending_hook_enabled_writes: HashMap::new(),
    }
}

fn test_session_telemetry(config: &Config, model: &str) -> SessionTelemetry {
    let model_info =
        construct_model_info_offline_for_tests(model, &config.to_models_manager_config());
    SessionTelemetry::new(
        ThreadId::new(),
        model,
        model_info.slug.as_str(),
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test_originator".to_string(),
        /*log_user_prompts*/ false,
        "test".to_string(),
        serde_json::from_value(serde_json::json!("cli"))
            .expect("cli session source should deserialize"),
    )
}

pub(super) fn app_enabled_in_effective_config(config: &Config, app_id: &str) -> Option<bool> {
    config
        .config_layer_stack
        .effective_config()
        .as_table()
        .and_then(|table| table.get("apps"))
        .and_then(TomlValue::as_table)
        .and_then(|apps| apps.get(app_id))
        .and_then(TomlValue::as_table)
        .and_then(|app| app.get("enabled"))
        .and_then(TomlValue::as_bool)
}
