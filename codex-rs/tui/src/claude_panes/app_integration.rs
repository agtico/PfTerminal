//! App integration: pane pickers, turn submission, and display synchronization.

use std::sync::Arc;

use anyhow::Result;
use anyhow::anyhow;

use crate::app::App;
use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_server_session::AppServerSession;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::chatwidget::ChatWidget;
use crate::spawn_orchestration::SpawnRole;
use crate::tui;
use codex_protocol::ThreadId;

use super::command_plan::claude_pane_title;
use super::command_plan::compose_claude_pane_prompt;
use super::command_plan::prompt_from_user_turn;
use super::execution::run_prepared_claude_turn;
use super::pane::ClaudePaneStatus;
use super::pane::ClaudePaneUsageStatus;
use super::pane::PaneLayoutState;
use super::progress::truncate_for_display;
use super::provider::ClaudeProviderProfileKind;
use super::registry::CODEX_MAIN_PANE_ID;
use super::registry::ClaudePaneRegistry;
use super::registry::PANE_LAYOUT_VERSION;
use super::registry::load_pane_layout;
use super::registry::persist_pane_layout;
use super::turn_types::ClaudePaneTurnOutput;
use super::turn_types::ClaudePaneTurnProgress;
impl App {
    pub(crate) async fn open_pane_picker(&mut self, app_server: &mut AppServerSession) {
        self.backfill_loaded_subagent_threads(app_server).await;
        self.restore_native_spawn_panes_from_saved_state(app_server)
            .await;

        let mut items = Vec::new();
        items.push(section_item("User Panes"));
        items.extend(self.user_pane_items());
        items.push(section_item("Agent Panes"));
        items.extend(self.spawn_tree_items(/*show_task_actions*/ false));
        items.push(section_item("New Pane"));
        items.extend(new_pane_items());

        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some("Panes".to_string()),
            subtitle: Some("Switch user panes or create Codex/Claude panes.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search panes".to_string()),
            ..Default::default()
        });
    }

    pub(crate) async fn restore_pane_layout_for_thread(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) {
        let thread_id_string = thread_id.to_string();
        let restored_pane_layout =
            load_pane_layout(self.config.codex_home.as_ref(), Some(&thread_id_string));

        self.spawn_parent_by_node = restored_pane_layout
            .as_ref()
            .map(|layout| {
                layout
                    .spawn_parent_by_node
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        self.spawn_parent_by_thread.clear();
        self.claude_panes = ClaudePaneRegistry::restore_from_disk(
            self.config.codex_home.as_ref(),
            restored_pane_layout.as_ref(),
        );
        self.spawn_nazgul_pane_id = restored_pane_layout
            .as_ref()
            .and_then(|layout| layout.spawn_nazgul_pane_id.clone())
            .filter(|pane_id| self.valid_restored_nazgul_binding(pane_id));
        self.claude_pane_transcript_cells.clear();
        self.seed_restored_claude_pane_transcripts();
        self.restore_native_spawn_panes_from_saved_state(app_server)
            .await;
        self.show_restored_active_claude_pane();
    }

    fn valid_restored_nazgul_binding(&self, pane_id: &str) -> bool {
        pane_id == CODEX_MAIN_PANE_ID
            || crate::spawn_orchestration::node_id_thread(pane_id).is_some()
            || self
                .claude_panes
                .panes()
                .iter()
                .any(|pane| pane.id == pane_id)
    }

    pub(crate) fn open_claude_pane_profile_picker(&mut self) {
        let mut items = Vec::new();
        for profile in ClaudeProviderProfileKind::creation_options() {
            let profile_config = profile.profile();
            let kind = *profile;
            items.push(SelectionItem {
                name: format!("+ {}", profile.status_model_label()),
                description: Some(profile_config.description.to_string()),
                search_value: Some(format!(
                    "{} {}",
                    profile_config.title, profile_config.description
                )),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::CreateClaudePane { profile: kind });
                })],
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some("New Claude Pane".to_string()),
            subtitle: Some("Choose the provider route for Claude Code headless.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search Claude providers".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn open_codex_pane_model_picker(&mut self) {
        let current_model = self.chat_widget.current_model().to_string();
        let current_effort = self.chat_widget.current_reasoning_effort();
        let mut items = Vec::new();
        items.push(section_item("Current Model"));
        items.push(codex_pane_model_item(
            current_model.clone(),
            ChatWidget::model_provider_for_selection(&current_model),
            current_effort,
            Some("Create a native Codex pane using the current model and reasoning.".to_string()),
        ));

        let presets = self
            .chat_widget
            .model_catalog()
            .try_list_models()
            .unwrap_or_default();
        let mut added_other_section = false;
        for preset in presets
            .into_iter()
            .filter(ChatWidget::show_in_pfterminal_model_picker)
            .filter(|preset| preset.model != current_model)
        {
            if !added_other_section {
                items.push(section_item("Other Models"));
                added_other_section = true;
            }
            let description = (!preset.description.is_empty()).then_some(preset.description);
            items.push(codex_pane_model_item(
                preset.model.clone(),
                ChatWidget::model_provider_for_selection(&preset.model),
                Some(preset.default_reasoning_effort),
                description,
            ));
        }

        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some("New Codex Pane".to_string()),
            subtitle: Some("Choose the model for the native Codex pane.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search models".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn save_active_claude_pane_transcript(&mut self) {
        let Some(active_pane_id) = self
            .claude_panes
            .active_claude_pane_id()
            .map(ToString::to_string)
        else {
            return;
        };
        self.claude_pane_transcript_cells
            .insert(active_pane_id, self.transcript_cells.clone());
    }

    fn restore_claude_pane_transcript(&mut self, tui: &mut tui::Tui, pane_id: &str) -> Result<()> {
        self.reset_for_thread_switch(tui)
            .map_err(|err| anyhow!(err.to_string()))?;
        self.transcript_cells = self
            .claude_pane_transcript_cells
            .get(pane_id)
            .cloned()
            .unwrap_or_default();
        let width = self
            .chat_widget
            .history_wrap_width(tui.terminal.last_known_screen_size.width);
        for cell in self.transcript_cells.clone() {
            self.insert_history_cell_lines(tui, cell.as_ref(), width);
        }
        Ok(())
    }

    pub(crate) fn append_inactive_claude_pane_transcript_cell(
        &mut self,
        pane_id: &str,
        cell: Arc<dyn crate::history_cell::HistoryCell>,
    ) {
        self.claude_pane_transcript_cells
            .entry(pane_id.to_string())
            .or_default()
            .push(cell);
    }

    pub(crate) fn persist_pane_state(&self) {
        let codex_thread_id = self
            .primary_thread_id
            .or_else(|| self.chat_widget.thread_id())
            .map(|thread_id| thread_id.to_string());
        let Some(codex_thread_id) = codex_thread_id else {
            return;
        };
        let layout = PaneLayoutState {
            version: PANE_LAYOUT_VERSION,
            codex_thread_id: Some(codex_thread_id),
            active_user_pane_id: Some(self.claude_panes.active_user_pane_id().to_string()),
            spawn_nazgul_pane_id: self.spawn_nazgul_pane_id.clone(),
            claude_pane_ids: self
                .claude_panes
                .panes()
                .iter()
                .map(|pane| pane.id.clone())
                .collect(),
            spawn_parent_by_node: self
                .spawn_parent_by_node
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        };
        if let Err(err) = persist_pane_layout(self.config.codex_home.as_ref(), &layout) {
            tracing::warn!(error = %err, "failed to persist pane layout");
        }
    }

    pub(crate) fn seed_restored_claude_pane_transcripts(&mut self) {
        let cwd = self.config.cwd.clone();
        let restored: Vec<_> = self
            .claude_panes
            .panes()
            .iter()
            .map(|pane| {
                (
                    pane.id.clone(),
                    pane.latest_result_message.clone(),
                    pane.latest_audit_path.clone(),
                    pane.latest_turn_status,
                )
            })
            .collect();
        for (pane_id, result, audit_path, status) in restored {
            let entry = self
                .claude_pane_transcript_cells
                .entry(pane_id)
                .or_default();
            if !entry.is_empty() {
                continue;
            }
            if let Some(result) = result {
                entry.push(Arc::new(crate::history_cell::AgentMarkdownCell::new(
                    result,
                    cwd.as_path(),
                )));
            }
            if let Some(audit_path) = audit_path {
                let status = status
                    .map(super::pane::ClaudePaneTurnStatus::label)
                    .unwrap_or("unknown");
                entry.push(Arc::new(crate::history_cell::new_info_event(
                    "Restored Claude pane state.".to_string(),
                    Some(format!(
                        "latest status: {status}; audit: {}",
                        audit_path.display()
                    )),
                )));
            }
        }
    }

    pub(crate) fn show_restored_active_claude_pane(&mut self) {
        let Some(active_pane_id) = self
            .claude_panes
            .active_claude_pane_id()
            .map(ToString::to_string)
        else {
            self.sync_external_pane_turn_display(CODEX_MAIN_PANE_ID);
            self.sync_active_agent_label();
            return;
        };
        self.transcript_cells = self
            .claude_pane_transcript_cells
            .get(&active_pane_id)
            .cloned()
            .unwrap_or_default();
        self.sync_external_pane_turn_display(&active_pane_id);
        self.sync_active_agent_label();
    }

    pub(crate) async fn select_user_pane(&mut self, tui: &mut tui::Tui, pane_id: String) {
        self.save_active_claude_pane_transcript();
        match self.claude_panes.set_active_user_pane(&pane_id) {
            Ok(()) if pane_id == CODEX_MAIN_PANE_ID => {
                self.sync_external_pane_turn_display(&pane_id);
                self.sync_active_agent_label();
                self.persist_pane_state();
            }
            Ok(()) => {
                self.detach_active_thread_for_external_pane().await;
                if let Err(err) = self.restore_claude_pane_transcript(tui, &pane_id) {
                    self.chat_widget
                        .add_error_message(format!("Failed to switch Claude pane display: {err}"));
                }
                self.sync_external_pane_turn_display(&pane_id);
                self.sync_active_agent_label();
                self.persist_pane_state();
            }
            Err(err) => self.chat_widget.add_error_message(err.to_string()),
        }
    }

    pub(crate) fn sync_external_pane_turn_display(&mut self, pane_id: &str) {
        if pane_id == CODEX_MAIN_PANE_ID || !self.claude_panes.claude_pane_is_running(pane_id) {
            self.chat_widget.suspend_external_pane_turn_display();
            return;
        }
        self.chat_widget.begin_external_pane_turn();
        if let Some(status) = self.claude_panes.live_status_for_pane(pane_id) {
            self.chat_widget
                .update_external_pane_live_status(status.header, status.details);
        }
    }

    pub(crate) async fn create_claude_pane(
        &mut self,
        tui: &mut tui::Tui,
        profile: ClaudeProviderProfileKind,
    ) {
        self.save_active_claude_pane_transcript();
        match self.claude_panes.create_pane(
            profile,
            self.config.cwd.to_path_buf(),
            self.config.codex_home.as_ref(),
        ) {
            Ok(id) => {
                self.detach_active_thread_for_external_pane().await;
                self.claude_pane_transcript_cells
                    .entry(id.clone())
                    .or_default();
                if let Err(err) = self.restore_claude_pane_transcript(tui, &id) {
                    self.chat_widget.add_error_message(format!(
                        "Failed to initialize Claude pane display: {err}"
                    ));
                }
                let title = profile.profile().title;
                self.sync_active_agent_label();
                self.persist_pane_state();
                self.chat_widget.add_info_message(
                    format!("Created and switched to {title}."),
                    Some("Type normally; turns will run through Claude Code headless.".to_string()),
                );
                tracing::info!(pane_id = %id, profile = ?profile, "created Claude headless pane");
            }
            Err(err) => self.chat_widget.add_error_message(err.to_string()),
        }
    }

    pub(crate) async fn create_spawn_claude_pane(
        &mut self,
        _tui: &mut tui::Tui,
        role: SpawnRole,
        parent_node_id: Option<String>,
        profile: ClaudeProviderProfileKind,
    ) {
        if role == SpawnRole::Nazgul {
            self.chat_widget.add_error_message(
                "Nazgul is a pane binding, not a spawned Claude worker.".to_string(),
            );
            return;
        }
        let spawn_nickname = self.next_spawn_agent_nickname(role);
        match self.claude_panes.create_pane_with_role(
            profile,
            self.config.cwd.to_path_buf(),
            self.config.codex_home.as_ref(),
            Some(role),
            spawn_nickname.clone(),
        ) {
            Ok(id) => {
                // Spawned workers are background children: the operator's
                // control surface stays where it is. Switching focus into the
                // new pane caused follow-up control-plane input to be routed
                // into the worker (round-3 B5 failure mode).
                self.claude_pane_transcript_cells
                    .entry(id.clone())
                    .or_default();
                let title = claude_pane_title(profile, Some(role), spawn_nickname.as_deref());
                let logical_parent_node_id =
                    self.logical_parent_node_for_spawn(role, parent_node_id.as_deref());
                self.spawn_parent_by_node.insert(
                    crate::spawn_orchestration::pane_node_id(&id),
                    logical_parent_node_id.clone(),
                );
                self.sync_active_agent_label();
                self.persist_pane_state();
                self.persist_claude_spawn_pane_state(&id, &logical_parent_node_id)
                    .await;
                self.chat_widget.add_info_message(
                    format!("Created {title} as a background worker."),
                    Some(format!(
                        "Harness: Claude Code; role: {}; control stays on the current pane (use /panes to open it); no task was started.",
                        role.label()
                    )),
                );
                tracing::info!(
                    pane_id = %id,
                    profile = ?profile,
                    role = ?role,
                    "created Claude spawn pane"
                );
            }
            Err(err) => self.chat_widget.add_error_message(err.to_string()),
        }
    }

    pub(crate) fn try_submit_active_claude_pane_op(&mut self, op: &AppCommand) -> bool {
        let Some(pane_id) = self
            .claude_panes
            .active_claude_pane_id()
            .map(ToString::to_string)
        else {
            return false;
        };
        if matches!(op, AppCommand::Interrupt { .. }) {
            if !self.claude_panes.claude_pane_is_running(&pane_id) {
                self.chat_widget.complete_external_pane_turn(
                    /*last_agent_message*/ None, /*duration_ms*/ None,
                );
                return true;
            }
            match self.claude_panes.interrupt_turn(&pane_id) {
                Ok(()) => {
                    self.chat_widget.update_external_pane_live_status(
                        "Claude interrupting".to_string(),
                        Some("Waiting for the Claude process to stop.".to_string()),
                    );
                }
                Err(err) => self.chat_widget.add_error_message(err.to_string()),
            }
            return true;
        }
        let prompt = match prompt_from_user_turn(op) {
            Ok(Some(prompt)) => prompt,
            Ok(None) => return false,
            Err(err) => {
                self.chat_widget.fail_external_pane_turn(err.to_string());
                return true;
            }
        };
        // Control-plane guard: slash commands act on PFTerminal itself, never
        // on the active worker pane. Without this, a recognized command that
        // reaches the op path while a Claude pane is active is forwarded to
        // the worker as task text (the round-3 B5 failure mode).
        if self.chat_widget.try_dispatch_slash_input(&prompt) {
            return true;
        }
        let prompt_context = self.claude_pane_prompt_context(&pane_id);
        let prompt = compose_claude_pane_prompt(prompt, prompt_context.as_deref());
        let prepared =
            match self
                .claude_panes
                .prepare_turn(&pane_id, prompt, self.config.codex_home.as_ref())
            {
                Ok(prepared) => prepared,
                Err(err) => {
                    self.chat_widget.fail_external_pane_turn(err.to_string());
                    return true;
                }
            };

        self.chat_widget.begin_external_pane_turn();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let pane_id = prepared.pane_id.clone();
            let result = run_prepared_claude_turn(prepared, Some(tx.clone())).await;
            tx.send(AppEvent::ClaudePaneTurnFinished { pane_id, result });
        });
        true
    }

    pub(crate) fn submit_claude_pane_task(&mut self, pane_id: String, task: String) {
        let task = task.trim().to_string();
        if task.is_empty() {
            self.chat_widget
                .add_error_message("Claude pane task cannot be empty.".to_string());
            return;
        }
        let is_active = self.claude_panes.active_user_pane_id() == pane_id;
        let user_cell =
            crate::history_cell::new_user_prompt(task.clone(), Vec::new(), Vec::new(), Vec::new());
        if is_active {
            self.app_event_tx
                .send(AppEvent::InsertHistoryCell(Box::new(user_cell)));
        } else {
            self.append_inactive_claude_pane_transcript_cell(&pane_id, Arc::new(user_cell));
        }
        self.claude_panes
            .set_latest_task_message(&pane_id, Some(task.clone()));
        let prompt_context = self.claude_pane_prompt_context(&pane_id);
        let prompt = compose_claude_pane_prompt(task.clone(), prompt_context.as_deref());
        let prepared =
            match self
                .claude_panes
                .prepare_turn(&pane_id, prompt, self.config.codex_home.as_ref())
            {
                Ok(prepared) => prepared,
                Err(err) => {
                    self.chat_widget.add_error_message(err.to_string());
                    return;
                }
            };
        self.record_claude_spawn_rollout_task_started(&pane_id, &task, prepared.plan.turn_index);

        if self.claude_panes.active_user_pane_id() == pane_id {
            self.chat_widget.begin_external_pane_turn();
        }
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let pane_id = prepared.pane_id.clone();
            let result = run_prepared_claude_turn(prepared, Some(tx.clone())).await;
            tx.send(AppEvent::ClaudePaneTurnFinished { pane_id, result });
        });
    }

    fn claude_pane_prompt_context(&self, pane_id: &str) -> Option<String> {
        let mut contexts = Vec::new();
        if let Some(role_context) = self
            .claude_panes
            .claude_pane_spawn_role(pane_id)
            .and_then(SpawnRole::claude_pane_context)
        {
            contexts.push(role_context.to_string());
        }
        if let Some(spawn_context) = self.spawn_context_for_user_pane(pane_id) {
            contexts.push(spawn_context);
        }
        (!contexts.is_empty()).then(|| contexts.join("\n\n"))
    }

    pub(crate) fn on_claude_pane_turn_progress(&mut self, progress: ClaudePaneTurnProgress) {
        let is_active = self.claude_panes.active_user_pane_id() == progress.pane_id;
        if let Some(delta) = progress.assistant_text_delta.as_deref() {
            let dispatches = self
                .claude_panes
                .collect_spawn_dispatches_from_assistant_delta(&progress.pane_id, delta);
            if !dispatches.is_empty() {
                self.dispatch_spawn_task_blocks(&progress.pane_id, dispatches);
            }
        }
        if let Some(status) = self.claude_panes.update_live_progress(&progress)
            && is_active
        {
            if progress.phase == "assistant-text"
                && let Some(delta) = self
                    .claude_panes
                    .take_visible_assistant_transcript_delta(&progress.pane_id)
            {
                self.chat_widget.stream_external_pane_response_delta(delta);
            }
            self.chat_widget
                .update_external_pane_live_status(status.header, status.details);
        }
    }

    pub(crate) fn on_claude_pane_turn_finished(
        &mut self,
        pane_id: String,
        result: Result<ClaudePaneTurnOutput, String>,
    ) {
        match result {
            Ok(mut output) => {
                let is_active = self.claude_panes.active_user_pane_id() == pane_id;
                let (visible_text, dispatches) =
                    crate::spawn_orchestration::extract_spawn_task_dispatches(&output.text);
                output.text = visible_text;
                if is_active
                    && let Some(delta) = self
                        .claude_panes
                        .take_final_visible_assistant_transcript_delta(&pane_id, &output.text)
                {
                    self.chat_widget.stream_external_pane_response_delta(delta);
                }
                let active_text_streamed = is_active
                    && self
                        .claude_panes
                        .has_emitted_visible_assistant_transcript(&pane_id);
                let dispatches = self
                    .claude_panes
                    .filter_new_spawn_dispatches(&pane_id, dispatches);
                self.claude_panes.finish_turn(&pane_id, &Ok(output.clone()));
                let report_status = output.status.label().to_string();
                let report_text = if output.text.trim().is_empty() {
                    output.failure_message()
                } else {
                    output.text.clone()
                };
                self.record_claude_spawn_rollout_task_completed(&pane_id, &output);
                self.record_spawn_child_report_for_claude_pane(
                    &pane_id,
                    &report_status,
                    Some(&report_text),
                );
                if !dispatches.is_empty() {
                    self.dispatch_spawn_task_blocks(&pane_id, dispatches);
                }
                if !output.text.trim().is_empty() {
                    if is_active && !active_text_streamed {
                        self.chat_widget
                            .append_external_pane_response(output.text.clone());
                    } else if !is_active {
                        self.append_inactive_claude_pane_transcript_cell(
                            &pane_id,
                            Arc::new(crate::history_cell::AgentMarkdownCell::new(
                                output.text.clone(),
                                self.config.cwd.as_path(),
                            )),
                        );
                    }
                }
                let hint = output.audit_hint();
                if output.status.is_success() {
                    if is_active {
                        self.chat_widget.complete_external_pane_turn(
                            Some(output.text),
                            Some(output.duration_ms),
                        );
                        self.chat_widget
                            .add_info_message("Claude pane turn complete.".to_string(), Some(hint));
                    } else {
                        self.append_inactive_claude_pane_transcript_cell(
                            &pane_id,
                            Arc::new(crate::history_cell::new_info_event(
                                "Claude pane turn complete.".to_string(),
                                Some(hint),
                            )),
                        );
                    }
                } else if is_active {
                    self.chat_widget
                        .fail_external_pane_turn(output.failure_message());
                    self.chat_widget.add_info_message(
                        "Claude pane turn audit recorded.".to_string(),
                        Some(hint),
                    );
                } else {
                    self.append_inactive_claude_pane_transcript_cell(
                        &pane_id,
                        Arc::new(crate::history_cell::new_error_event(
                            output.failure_message(),
                        )),
                    );
                    self.append_inactive_claude_pane_transcript_cell(
                        &pane_id,
                        Arc::new(crate::history_cell::new_info_event(
                            "Claude pane turn audit recorded.".to_string(),
                            Some(hint),
                        )),
                    );
                }
            }
            Err(error) => {
                self.claude_panes.finish_turn(&pane_id, &Err(error.clone()));
                self.record_spawn_child_report_for_claude_pane(&pane_id, "error", Some(&error));
                if self.claude_panes.active_user_pane_id() == pane_id {
                    self.chat_widget.fail_external_pane_turn(error);
                } else {
                    self.append_inactive_claude_pane_transcript_cell(
                        &pane_id,
                        Arc::new(crate::history_cell::new_error_event(error)),
                    );
                }
            }
        }
    }

    fn user_pane_items(&self) -> Vec<SelectionItem> {
        let mut items = Vec::new();
        let is_current = self.claude_panes.active_user_pane_id() == CODEX_MAIN_PANE_ID;
        items.push(SelectionItem {
            name: "Codex - Main".to_string(),
            description: Some("Current PFTerminal/Codex session".to_string()),
            is_current,
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::SelectUserPane {
                    pane_id: CODEX_MAIN_PANE_ID.to_string(),
                });
            })],
            dismiss_on_select: true,
            ..Default::default()
        });
        items.extend(self.codex_user_pane_items());
        for pane in self.claude_panes.panes() {
            let pane_id = pane.id.clone();
            let mut description = match pane.status {
                ClaudePaneStatus::Idle => "idle".to_string(),
                ClaudePaneStatus::Running => "running".to_string(),
            };
            if let Some(status) = pane.latest_turn_status {
                description.push_str(&format!("; latest status: {}", status.label()));
            }
            if let Some(status) = pane.latest_usage_status {
                match (status, pane.latest_usage_summary.as_deref()) {
                    (ClaudePaneUsageStatus::Reported, Some(usage)) => {
                        description.push_str(&format!("; latest usage: {usage}"));
                    }
                    _ => {
                        description.push_str(&format!("; latest usage: {}", status.label()));
                    }
                }
            }
            if let Some(path) = pane.latest_audit_path.as_ref() {
                description.push_str(&format!("; audit: {}", path.display()));
            }
            if let Some(task) = pane.latest_task_message.as_deref() {
                description.push_str(&format!("; task: {task}"));
            }
            if let Some(result) = pane.latest_result_message.as_deref() {
                description.push_str(&format!("; result: {result}"));
            }
            items.push(SelectionItem {
                name: pane.title.clone(),
                description: Some(description),
                is_current: self.claude_panes.active_user_pane_id() == pane.id,
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::SelectUserPane {
                        pane_id: pane_id.clone(),
                    });
                })],
                dismiss_on_select: true,
                search_value: Some(format!("{} {}", pane.title, pane.id)),
                ..Default::default()
            });
        }
        items
    }

    fn codex_user_pane_items(&self) -> Vec<SelectionItem> {
        self.agent_navigation
            .ordered_threads()
            .into_iter()
            .filter(|(thread_id, _)| Some(*thread_id) != self.primary_thread_id)
            .filter(|(thread_id, _)| !self.is_spawn_orchestration_thread(*thread_id))
            .filter(|(_, entry)| {
                entry
                    .agent_role
                    .as_deref()
                    .map(|role| role == "default")
                    .unwrap_or(true)
            })
            .map(|(thread_id, entry)| {
                let name = entry
                    .agent_nickname
                    .as_deref()
                    .filter(|nickname| !nickname.trim().is_empty())
                    .map(|nickname| format!("Codex - {nickname}"))
                    .unwrap_or_else(|| format!("Codex - {}", short_thread_id(thread_id)));
                let mut description = if entry.is_closed {
                    "done".to_string()
                } else if entry.is_running {
                    "running".to_string()
                } else {
                    "idle".to_string()
                };
                if let Some(task) = entry.last_task_message.as_deref() {
                    description.push_str(&format!(
                        "; latest task: {}",
                        truncate_for_display(task, 80)
                    ));
                }
                if let Some(result) = entry.last_result_message.as_deref() {
                    description.push_str(&format!(
                        "; latest result: {}",
                        truncate_for_display(result, 80)
                    ));
                }
                SelectionItem {
                    name: name.clone(),
                    name_prefix_spans: crate::multi_agents::agent_picker_status_dot_spans(
                        entry.is_closed,
                    ),
                    description: Some(description),
                    is_current: self.claude_panes.active_user_pane_id() == CODEX_MAIN_PANE_ID
                        && self.active_thread_id == Some(thread_id),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::SelectAgentThread(thread_id));
                    })],
                    dismiss_on_select: true,
                    search_value: Some(format!("{name} {thread_id}")),
                    ..Default::default()
                }
            })
            .collect()
    }

    pub(crate) fn next_codex_pane_nickname(&self) -> String {
        let count = self
            .agent_navigation
            .ordered_threads()
            .into_iter()
            .filter(|(thread_id, _)| Some(*thread_id) != self.primary_thread_id)
            .filter(|(thread_id, _)| !self.is_spawn_orchestration_thread(*thread_id))
            .filter(|(_, entry)| {
                entry
                    .agent_role
                    .as_deref()
                    .map(|role| role == "default")
                    .unwrap_or(true)
            })
            .count();
        format!("Codex {}", count + 1)
    }
}

fn codex_pane_model_item(
    model: String,
    provider: Option<String>,
    effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    description: Option<String>,
) -> SelectionItem {
    SelectionItem {
        name: model.clone(),
        description,
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::CreateCodexPane {
                model: model.clone(),
                provider: provider.clone(),
                effort: effort.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

pub(crate) fn new_pane_items() -> Vec<SelectionItem> {
    vec![
        SelectionItem {
            name: "+ Codex Pane".to_string(),
            description: Some(
                "Create a persistent native Codex pane; choose model next.".to_string(),
            ),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::OpenCodexPaneModelPicker);
            })],
            dismiss_on_select: true,
            ..Default::default()
        },
        SelectionItem {
            name: "+ Claude Pane".to_string(),
            description: Some(
                "Create a Claude Code headless pane; choose provider next.".to_string(),
            ),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::OpenClaudePaneProfilePicker);
            })],
            dismiss_on_select: true,
            ..Default::default()
        },
    ]
}

fn short_thread_id(thread_id: codex_protocol::ThreadId) -> String {
    thread_id.to_string().chars().take(8).collect()
}

fn section_item(name: &str) -> SelectionItem {
    SelectionItem {
        name: name.to_string(),
        is_disabled: true,
        ..Default::default()
    }
}
