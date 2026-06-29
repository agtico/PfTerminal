//! Task Node menu, terminal auth, and task actions.

use super::*;
use codex_vault::AddCredential;
use codex_vault::CredentialType;
use codex_vault::Vault;
use codex_vault::VaultError;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;

const TASKNODE_MENU_VIEW_ID: &str = "tasknode-menu";
const TASKNODE_TASKS_VIEW_ID: &str = "tasknode-tasks";
const TASKNODE_TASK_ACTIONS_VIEW_ID: &str = "tasknode-task-actions";
const TASKNODE_REQUESTS_VIEW_ID: &str = "tasknode-requests";
const TASKNODE_CONTEXT_VIEW_ID: &str = "tasknode-context";
const TASKNODE_SESSION_LABEL: &str = "tasknode/session";
const TASKNODE_MENU_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub(crate) struct TaskNodeMenuCountsCache {
    outstanding: Option<usize>,
    verification: Option<usize>,
    refused: Option<usize>,
    rewarded: Option<usize>,
    request_count: Option<usize>,
}

impl TaskNodeMenuCountsCache {
    fn update_from_status(&mut self, status: &TaskNodeStatusResponse) {
        self.outstanding = Some(status.counts.outstanding);
        self.verification = Some(status.counts.verification);
        self.refused = Some(status.counts.refused);
        self.rewarded = Some(status.counts.rewarded);
    }
}

impl ChatWidget {
    pub(crate) fn open_tasknode_menu(&mut self) {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        let state = TaskNodeLocalState::load(&codex_home);
        let counts = self.tasknode_menu_counts.clone();
        let should_refresh_counts = state
            .as_ref()
            .ok()
            .and_then(|session| session.as_ref())
            .is_some_and(|session| session.terminal_token.is_some());
        self.show_or_replace_tasknode_selection(TASKNODE_MENU_VIEW_ID, || {
            tasknode_menu_params(state, counts.as_ref(), None)
        });
        if should_refresh_counts {
            self.tasknode_menu_poll_generation = self.tasknode_menu_poll_generation.wrapping_add(1);
            self.refresh_tasknode_menu_counts();
            self.schedule_tasknode_menu_poll();
        }
    }

    pub(crate) fn handle_tasknode_menu_status_result(&mut self, result: Result<Value, String>) {
        match parse_tasknode_value::<TaskNodeStatusResponse>(result, "menu status") {
            Ok(status) => {
                self.tasknode_menu_counts
                    .get_or_insert_with(TaskNodeMenuCountsCache::default)
                    .update_from_status(&status);
                self.refresh_active_tasknode_menu(None);
            }
            Err(err) => self.refresh_active_tasknode_menu(Some(err)),
        }
    }

    pub(crate) fn handle_tasknode_menu_poll(&mut self, generation: u64) {
        if generation != self.tasknode_menu_poll_generation
            || self.bottom_pane.active_view_id() != Some(TASKNODE_MENU_VIEW_ID)
            || !self.has_linked_tasknode_session()
        {
            return;
        }
        self.refresh_tasknode_menu_counts();
        self.schedule_tasknode_menu_poll();
    }

    pub(crate) fn handle_tasknode_menu_requests_result(&mut self, result: Result<Value, String>) {
        match parse_tasknode_value::<TaskNodeRequestsResponse>(result, "menu requests") {
            Ok(requests) => {
                self.tasknode_menu_counts
                    .get_or_insert_with(TaskNodeMenuCountsCache::default)
                    .request_count = Some(requests.items.len());
                self.refresh_active_tasknode_menu(None);
            }
            Err(err) => self.refresh_active_tasknode_menu(Some(err)),
        }
    }

    pub(crate) fn open_tasknode_link(&mut self) {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        match TaskNodeClient::new_without_token().start_github_link() {
            Ok(started) => {
                let session = TaskNodeLocalSession {
                    origin: tasknode_origin(),
                    account_id: None,
                    github_username: None,
                    terminal_token: None,
                    expires_at: None,
                    pending_request_id: Some(started.request_id.clone()),
                    pending_poll_token: Some(started.poll_token.clone()),
                    pending_verification_url: Some(started.verification_url.clone()),
                };
                if let Err(err) = session.save(&codex_home) {
                    self.add_error_message(format!(
                        "Failed to store Task Node link request: {err}"
                    ));
                    return;
                }
                let mut hint = tasknode_link_hint(&started.verification_url);
                if tasknode_should_open_browser()
                    && let Err(err) = webbrowser::open(&started.verification_url)
                {
                    hint = format!("{hint} Browser open failed: {err}");
                }
                self.add_info_message("Task Node GitHub link is ready.".to_string(), Some(hint));
            }
            Err(err) => self.add_error_message(format!("Task Node link failed: {err}")),
        }
    }

    pub(crate) fn open_tasknode_status(&mut self) {
        self.add_info_message("Loading Task Node status...".to_string(), None);
        self.spawn_tasknode_value_request(
            "status",
            |client| client.status(),
            |result| AppEvent::TaskNodeStatusResult { result },
        );
    }

    pub(crate) fn handle_tasknode_status_result(&mut self, result: Result<Value, String>) {
        match parse_tasknode_value::<TaskNodeStatusResponse>(result, "status") {
            Ok(status) => self.add_plain_history_lines(tasknode_status_lines(&status)),
            Err(err) => self.add_error_message(format!("Task Node status failed: {err}")),
        }
    }

    pub(crate) fn open_tasknode_task_list(&mut self, tab: String) {
        self.show_or_replace_tasknode_selection(TASKNODE_TASKS_VIEW_ID, || {
            tasknode_loading_selection_params(
                TASKNODE_TASKS_VIEW_ID,
                format!("Task Node {}", tab_title(&tab)),
                "Loading tasks from Task Node...".to_string(),
            )
        });
        self.spawn_tasknode_value_request(
            "tasks",
            {
                let tab = tab.clone();
                move |client| client.tasks(&tab)
            },
            move |result| AppEvent::TaskNodeTaskListResult {
                tab: tab.clone(),
                result,
            },
        );
    }

    pub(crate) fn handle_tasknode_task_list_result(
        &mut self,
        tab: String,
        result: Result<Value, String>,
    ) {
        match parse_tasknode_value::<TaskNodeTasksResponse>(result, "tasks") {
            Ok(response) => {
                let task_count = response.tasks.len();
                let mut header = ColumnRenderable::new();
                header.push(Line::from(
                    format!("Task Node {}", tab_title(&response.tab)).bold(),
                ));
                header.push(Line::from(format!("{task_count} task(s)").dim()));
                self.show_or_replace_tasknode_selection(TASKNODE_TASKS_VIEW_ID, || {
                    SelectionViewParams {
                        view_id: Some(TASKNODE_TASKS_VIEW_ID),
                        footer_hint: Some(standard_popup_hint_line()),
                        is_searchable: true,
                        search_placeholder: Some("Search tasks".to_string()),
                        header: Box::new(header),
                        items: tasknode_task_items(response.tasks),
                        ..Default::default()
                    }
                });
            }
            Err(err) => {
                self.show_or_replace_tasknode_selection(TASKNODE_TASKS_VIEW_ID, || {
                    tasknode_error_selection_params(
                        TASKNODE_TASKS_VIEW_ID,
                        format!("Task Node {}", tab_title(&tab)),
                        format!("Task Node tasks failed: {err}"),
                    )
                });
                self.add_error_message(format!("Task Node tasks failed: {err}"));
            }
        }
    }

    pub(crate) fn open_tasknode_task_actions(&mut self, task_id: String) {
        self.show_or_replace_tasknode_selection(TASKNODE_TASK_ACTIONS_VIEW_ID, || {
            tasknode_loading_selection_params(
                TASKNODE_TASK_ACTIONS_VIEW_ID,
                task_id.clone(),
                "Loading task detail from Task Node...".to_string(),
            )
        });
        self.spawn_tasknode_value_request(
            "task-detail",
            {
                let task_id = task_id.clone();
                move |client| client.task_detail(&task_id)
            },
            move |result| AppEvent::TaskNodeTaskActionsResult {
                task_id: task_id.clone(),
                result,
            },
        );
    }

    pub(crate) fn handle_tasknode_task_actions_result(
        &mut self,
        task_id: String,
        result: Result<Value, String>,
    ) {
        match parse_tasknode_value::<TaskNodeTaskDetailResponse>(result, "task detail") {
            Ok(detail) => {
                let header = tasknode_task_detail_header(&detail, &task_id);
                self.show_or_replace_tasknode_selection(TASKNODE_TASK_ACTIONS_VIEW_ID, || {
                    SelectionViewParams {
                        view_id: Some(TASKNODE_TASK_ACTIONS_VIEW_ID),
                        footer_hint: Some(standard_popup_hint_line()),
                        is_searchable: false,
                        header: Box::new(header),
                        items: tasknode_task_action_items(task_id, detail.actions),
                        ..Default::default()
                    }
                });
            }
            Err(err) => {
                self.show_or_replace_tasknode_selection(TASKNODE_TASK_ACTIONS_VIEW_ID, || {
                    tasknode_error_selection_params(
                        TASKNODE_TASK_ACTIONS_VIEW_ID,
                        task_id,
                        format!("Task Node task detail failed: {err}"),
                    )
                });
                self.add_error_message(format!("Task Node task detail failed: {err}"));
            }
        }
    }

    pub(crate) fn copy_tasknode_task_brief(&mut self, task_id: String) {
        self.add_info_message(
            "Loading Task Node task brief...".to_string(),
            Some(task_id.clone()),
        );
        self.spawn_tasknode_value_request(
            "copy-task-brief",
            {
                let task_id = task_id.clone();
                move |client| client.task_detail(&task_id)
            },
            move |result| AppEvent::CopyTaskNodeTaskBriefResult {
                task_id: task_id.clone(),
                result,
            },
        );
    }

    pub(crate) fn handle_copy_tasknode_task_brief_result(
        &mut self,
        task_id: String,
        result: Result<Value, String>,
    ) {
        match parse_tasknode_value::<TaskNodeTaskDetailResponse>(result, "task brief") {
            Ok(detail) => {
                let brief = tasknode_task_brief(&detail);
                if brief.trim().is_empty() {
                    self.add_error_message("Task Node did not return a task brief.".to_string());
                    return;
                }
                match crate::clipboard_copy::copy_to_clipboard(&brief) {
                    Ok(lease) => {
                        self.clipboard_lease = lease;
                        self.add_info_message(
                            "Copied Task Node task brief to clipboard.".to_string(),
                            Some(task_id),
                        );
                    }
                    Err(err) => {
                        self.add_error_message(format!("Failed to copy task brief: {err}"));
                    }
                }
            }
            Err(err) => self.add_error_message(format!("Task Node task detail failed: {err}")),
        }
    }

    pub(crate) fn submit_tasknode_task_action(&mut self, task_id: String, action: String) {
        self.add_info_message(
            format!("Submitting Task Node action: {action}..."),
            Some(task_id.clone()),
        );
        self.spawn_tasknode_value_request(
            "task-action",
            {
                let task_id = task_id.clone();
                let action = action.clone();
                move |client| client.task_action(&task_id, &action)
            },
            move |result| AppEvent::SubmitTaskNodeTaskActionResult {
                action: action.clone(),
                result,
            },
        );
    }

    pub(crate) fn handle_submit_tasknode_task_action_result(
        &mut self,
        action: String,
        result: Result<Value, String>,
    ) {
        match result {
            Ok(value) => {
                self.add_info_message(
                    format!("Task Node action recorded: {action}."),
                    tasknode_response_hint(&value),
                );
                self.refresh_tasknode_after_write();
            }
            Err(err) => self.add_error_message(format!("Task Node action failed: {err}")),
        }
    }

    pub(crate) fn open_tasknode_evidence_prompt(&mut self, task_id: String) {
        self.add_info_message(
            "Loading Task Node evidence context...".to_string(),
            Some(task_id.clone()),
        );
        self.spawn_tasknode_value_request(
            "evidence-context",
            {
                let task_id = task_id.clone();
                move |client| client.task_detail(&task_id)
            },
            move |result| AppEvent::OpenTaskNodeEvidencePromptResult {
                task_id: task_id.clone(),
                result,
            },
        );
    }

    pub(crate) fn handle_open_tasknode_evidence_prompt_result(
        &mut self,
        task_id: String,
        result: Result<Value, String>,
    ) {
        let (title, placeholder) =
            match parse_tasknode_value::<TaskNodeTaskDetailResponse>(result, "evidence context") {
                Ok(detail) => {
                    self.add_plain_history_lines(tasknode_evidence_context_lines(
                        &detail, &task_id,
                    ));
                    let prompt = detail
                        .terminal
                        .as_ref()
                        .and_then(|terminal| terminal.evidence_prompt.as_ref());
                    let title = prompt
                        .and_then(|prompt| prompt.title.clone())
                        .unwrap_or_else(|| "Submit Task Node evidence".to_string());
                    let placeholder = prompt
                    .map(tasknode_evidence_placeholder)
                    .unwrap_or_else(|| {
                        "Paste PR URL, commit URL, terminal output, test result, or concise proof"
                            .to_string()
                    });
                    (title, placeholder)
                }
                Err(err) => {
                    self.add_info_message(
                        "Task Node evidence context unavailable; submit evidence directly."
                            .to_string(),
                        Some(err.to_string()),
                    );
                    self.add_plain_history_lines(tasknode_evidence_context_fallback_lines(
                        &task_id,
                    ));
                    (
                        "Submit Task Node evidence".to_string(),
                        "Paste PR URL, commit URL, terminal output, test result, or concise proof"
                            .to_string(),
                    )
                }
            };
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            title,
            placeholder,
            String::new(),
            Some(format!("Evidence for {task_id}")),
            Box::new(move |summary: String| {
                tx.send(AppEvent::SubmitTaskNodeEvidence {
                    task_id: task_id.clone(),
                    summary,
                });
            }),
        );
        self.show_custom_prompt_view(view);
    }

    pub(crate) fn submit_tasknode_evidence(&mut self, task_id: String, summary: String) {
        self.add_info_message(
            "Submitting Task Node evidence...".to_string(),
            Some(task_id.clone()),
        );
        self.spawn_tasknode_value_request(
            "submit-evidence",
            move |client| client.submit_evidence(&task_id, &summary),
            |result| AppEvent::SubmitTaskNodeEvidenceResult { result },
        );
    }

    pub(crate) fn handle_submit_tasknode_evidence_result(&mut self, result: Result<Value, String>) {
        match result {
            Ok(value) => {
                self.add_info_message(
                    "Task Node evidence submitted.".to_string(),
                    tasknode_response_hint(&value),
                );
                self.refresh_tasknode_after_write();
            }
            Err(err) => self.add_error_message(format!("Task Node evidence failed: {err}")),
        }
    }

    pub(crate) fn open_tasknode_task_request_prompt(&mut self) {
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "Request personal task".to_string(),
            "Describe the work you want Task Node to generate".to_string(),
            String::new(),
            Some("Uses your Task Node context, memory, recent chats, and task queue.".to_string()),
            Box::new(move |detail: String| {
                tx.send(AppEvent::SubmitTaskNodeTaskRequest { detail });
            }),
        );
        self.show_custom_prompt_view(view);
    }

    pub(crate) fn submit_tasknode_task_request(&mut self, detail: String) {
        let detail = detail.trim().to_string();
        if detail.is_empty() {
            self.add_error_message("Task request text is required.".to_string());
            return;
        }
        self.add_info_message("Submitting Task Node task request...".to_string(), None);
        self.spawn_tasknode_value_request(
            "task-request",
            move |client| client.request_task(&detail),
            |result| AppEvent::SubmitTaskNodeTaskRequestResult { result },
        );
    }

    pub(crate) fn handle_submit_tasknode_task_request_result(
        &mut self,
        result: Result<Value, String>,
    ) {
        match result {
            Ok(value) => self.add_plain_history_lines(tasknode_task_request_result_lines(&value)),
            Err(err) => self.add_error_message(format!("Task Node task request failed: {err}")),
        }
    }

    pub(crate) fn open_tasknode_context(&mut self) {
        self.show_or_replace_tasknode_selection(TASKNODE_CONTEXT_VIEW_ID, || {
            tasknode_loading_selection_params(
                TASKNODE_CONTEXT_VIEW_ID,
                "Task Node context".to_string(),
                "Loading current context document from Task Node...".to_string(),
            )
        });
        self.spawn_tasknode_value_request(
            "context",
            |client| client.context(),
            |result| AppEvent::OpenTaskNodeContextResult { result },
        );
    }

    pub(crate) fn handle_open_tasknode_context_result(&mut self, result: Result<Value, String>) {
        match parse_tasknode_value::<TaskNodeContextResponse>(result, "context") {
            Ok(response) => {
                self.add_plain_history_lines(tasknode_context_lines(&response.context));
                let header = tasknode_context_header(&response.context);
                let items = tasknode_context_items(response.context);
                self.show_or_replace_tasknode_selection(TASKNODE_CONTEXT_VIEW_ID, || {
                    SelectionViewParams {
                        view_id: Some(TASKNODE_CONTEXT_VIEW_ID),
                        footer_hint: Some(standard_popup_hint_line()),
                        is_searchable: false,
                        header: Box::new(header),
                        items,
                        ..Default::default()
                    }
                });
            }
            Err(err) => {
                self.show_or_replace_tasknode_selection(TASKNODE_CONTEXT_VIEW_ID, || {
                    tasknode_error_selection_params(
                        TASKNODE_CONTEXT_VIEW_ID,
                        "Task Node context".to_string(),
                        format!("Task Node context failed: {err}"),
                    )
                });
                self.add_error_message(format!("Task Node context failed: {err}"));
            }
        }
    }

    pub(crate) fn open_tasknode_context_edit(
        &mut self,
        title: String,
        body: String,
        revision: u64,
        body_format: String,
    ) {
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "Edit Task Node context".to_string(),
            "Edit the current context document".to_string(),
            body,
            Some(format!("Revision {revision}; format: {body_format}")),
            Box::new(move |body: String| {
                tx.send(AppEvent::SubmitTaskNodeContextEdit {
                    title: title.clone(),
                    body,
                    revision,
                });
            }),
        )
        .with_submit_mode(CustomPromptSubmitMode::CtrlD);
        self.show_custom_prompt_view(view);
    }

    pub(crate) fn submit_tasknode_context_edit(
        &mut self,
        title: String,
        body: String,
        revision: u64,
    ) {
        if body.trim().is_empty() {
            self.add_error_message("Task Node context body is required.".to_string());
            return;
        }
        self.add_info_message("Saving Task Node context...".to_string(), None);
        self.spawn_tasknode_value_request(
            "save-context",
            move |client| client.save_context(&title, &body, revision),
            |result| AppEvent::SubmitTaskNodeContextEditResult { result },
        );
    }

    pub(crate) fn handle_submit_tasknode_context_edit_result(
        &mut self,
        result: Result<Value, String>,
    ) {
        match parse_tasknode_value::<TaskNodeContextSaveResponse>(result, "context save") {
            Ok(response) => {
                self.add_info_message(
                    response
                        .message
                        .unwrap_or_else(|| "Task Node context saved.".to_string()),
                    None,
                );
                self.add_plain_history_lines(tasknode_context_lines(&response.context));
                let header = tasknode_context_header(&response.context);
                let items = tasknode_context_items(response.context);
                self.show_or_replace_tasknode_selection(TASKNODE_CONTEXT_VIEW_ID, || {
                    SelectionViewParams {
                        view_id: Some(TASKNODE_CONTEXT_VIEW_ID),
                        footer_hint: Some(standard_popup_hint_line()),
                        is_searchable: false,
                        header: Box::new(header),
                        items,
                        ..Default::default()
                    }
                });
            }
            Err(err) => self.add_error_message(format!("Task Node context save failed: {err}")),
        }
    }

    pub(crate) fn open_tasknode_request_list(&mut self) {
        self.show_or_replace_tasknode_selection(TASKNODE_REQUESTS_VIEW_ID, || {
            tasknode_loading_selection_params(
                TASKNODE_REQUESTS_VIEW_ID,
                "Task Node requests".to_string(),
                "Loading task requests from Task Node...".to_string(),
            )
        });
        self.spawn_tasknode_value_request(
            "task-requests",
            |client| client.task_requests(),
            |result| AppEvent::OpenTaskNodeRequestListResult { result },
        );
    }

    pub(crate) fn handle_open_tasknode_request_list_result(
        &mut self,
        result: Result<Value, String>,
    ) {
        match parse_tasknode_value::<TaskNodeRequestsResponse>(result, "task requests") {
            Ok(response) => {
                let mut header = ColumnRenderable::new();
                header.push(Line::from("Task Node requests".bold()));
                header.push(Line::from(
                    format!("{} active or recent request(s)", response.items.len()).dim(),
                ));
                self.show_or_replace_tasknode_selection(TASKNODE_REQUESTS_VIEW_ID, || {
                    SelectionViewParams {
                        view_id: Some(TASKNODE_REQUESTS_VIEW_ID),
                        footer_hint: Some(standard_popup_hint_line()),
                        is_searchable: true,
                        search_placeholder: Some("Search task requests".to_string()),
                        header: Box::new(header),
                        items: tasknode_request_items(response.items),
                        ..Default::default()
                    }
                });
            }
            Err(err) => {
                self.show_or_replace_tasknode_selection(TASKNODE_REQUESTS_VIEW_ID, || {
                    tasknode_error_selection_params(
                        TASKNODE_REQUESTS_VIEW_ID,
                        "Task Node requests".to_string(),
                        format!("Task Node task requests failed: {err}"),
                    )
                });
                self.add_error_message(format!("Task Node task requests failed: {err}"));
            }
        }
    }

    pub(crate) fn open_tasknode_balance(&mut self) {
        self.add_info_message("Loading Task Node balance...".to_string(), None);
        self.spawn_tasknode_value_request(
            "balance",
            |client| client.balance(),
            |result| AppEvent::OpenTaskNodeBalanceResult { result },
        );
    }

    pub(crate) fn handle_open_tasknode_balance_result(&mut self, result: Result<Value, String>) {
        match result {
            Ok(value) => {
                self.add_plain_history_lines(tasknode_value_lines("Task Node balance", &value))
            }
            Err(err) => self.add_error_message(format!("Task Node balance failed: {err}")),
        }
    }

    pub(crate) fn open_tasknode_rewards(&mut self) {
        self.add_info_message("Loading Task Node rewards...".to_string(), None);
        self.spawn_tasknode_value_request(
            "rewards",
            |client| client.rewards(),
            |result| AppEvent::OpenTaskNodeRewardsResult { result },
        );
    }

    pub(crate) fn handle_open_tasknode_rewards_result(&mut self, result: Result<Value, String>) {
        match result {
            Ok(value) => self.add_plain_history_lines(tasknode_rewards_lines(&value)),
            Err(err) => self.add_error_message(format!("Task Node rewards failed: {err}")),
        }
    }

    pub(crate) fn logout_tasknode(&mut self) {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        let session = TaskNodeLocalState::load(&codex_home).ok().flatten();
        if let Some(token) = session.as_ref().and_then(|s| s.terminal_token.clone()) {
            let _ = TaskNodeClient::new(token).revoke();
        }
        self.tasknode_menu_counts = None;
        self.tasknode_menu_poll_generation = self.tasknode_menu_poll_generation.wrapping_add(1);
        match Vault::new(codex_home).delete(TASKNODE_SESSION_LABEL) {
            Ok(_) => self.add_info_message("Task Node session removed.".to_string(), None),
            Err(err) => {
                self.add_error_message(format!("Failed to remove Task Node session: {err}"))
            }
        }
    }

    fn spawn_tasknode_value_request(
        &mut self,
        label: &'static str,
        fetch: impl FnOnce(TaskNodeClient) -> Result<Value, String> + Send + 'static,
        event: impl FnOnce(Result<Value, String>) -> AppEvent + Send + 'static,
    ) {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        let tx = self.app_event_tx.clone();
        let spawn_result = std::thread::Builder::new()
            .name(format!("tasknode-{label}"))
            .spawn(move || {
                let result = tasknode_client_for_codex_home(&codex_home).and_then(fetch);
                tx.send(event(result));
            });
        if let Err(err) = spawn_result {
            self.add_error_message(format!("Task Node {label} worker failed: {err}"));
        }
    }

    fn refresh_tasknode_menu_counts(&mut self) {
        self.spawn_tasknode_value_request(
            "menu-status",
            |client| client.status(),
            |result| AppEvent::TaskNodeMenuStatusResult { result },
        );
        self.spawn_tasknode_value_request(
            "menu-requests",
            |client| client.task_requests(),
            |result| AppEvent::TaskNodeMenuRequestsResult { result },
        );
    }

    fn schedule_tasknode_menu_poll(&mut self) {
        let generation = self.tasknode_menu_poll_generation;
        let tx = self.app_event_tx.clone();
        let spawn_result = std::thread::Builder::new()
            .name("tasknode-menu-poll".to_string())
            .spawn(move || {
                std::thread::sleep(TASKNODE_MENU_POLL_INTERVAL);
                tx.send(AppEvent::TaskNodeMenuPoll { generation });
            });
        if let Err(err) = spawn_result {
            self.add_error_message(format!("Task Node menu poll failed: {err}"));
        }
    }

    fn has_linked_tasknode_session(&self) -> bool {
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        TaskNodeLocalState::load(&codex_home)
            .ok()
            .flatten()
            .is_some_and(|session| session.terminal_token.is_some())
    }

    fn refresh_active_tasknode_menu(&mut self, refresh_error: Option<String>) {
        if self.bottom_pane.active_view_id() != Some(TASKNODE_MENU_VIEW_ID) {
            return;
        }
        let codex_home = self.config.codex_home.as_path().to_path_buf();
        let state = TaskNodeLocalState::load(&codex_home);
        let counts = self.tasknode_menu_counts.clone();
        let params = tasknode_menu_params(state, counts.as_ref(), refresh_error.as_deref());
        let _ = self
            .bottom_pane
            .replace_selection_view_if_active(TASKNODE_MENU_VIEW_ID, params);
    }

    fn refresh_tasknode_after_write(&mut self) {
        self.refresh_tasknode_menu_counts();
        if self.bottom_pane.active_view_id() == Some(TASKNODE_TASKS_VIEW_ID) {
            self.open_tasknode_task_list("outstanding".to_string());
        }
    }

    fn show_or_replace_tasknode_selection(
        &mut self,
        view_id: &'static str,
        build: impl FnOnce() -> SelectionViewParams,
    ) {
        let replace_active = self.bottom_pane.active_view_id() == Some(view_id);
        let params = build();
        if replace_active {
            let _ = self
                .bottom_pane
                .replace_selection_view_if_active(view_id, params);
            return;
        }
        self.show_selection_view(params);
    }
}

fn tasknode_origin() -> String {
    std::env::var("PFT_TASKNODE_ORIGIN")
        .or_else(|_| std::env::var("TASKNODE_ORIGIN"))
        .unwrap_or_else(|_| "https://tasknode.postfiat.org".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn tasknode_link_hint(verification_url: &str) -> String {
    format!("Open this URL: {verification_url} Complete GitHub auth, then run /tasknode status.")
}

fn tasknode_should_open_browser() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn tasknode_client_for_codex_home(codex_home: &std::path::Path) -> Result<TaskNodeClient, String> {
    let session = ensure_tasknode_session(codex_home).map_err(|err| err.to_string())?;
    let token = session
        .terminal_token
        .ok_or_else(|| "Task Node session is missing a terminal token.".to_string())?;
    Ok(TaskNodeClient::new(token))
}

fn parse_tasknode_value<T: DeserializeOwned>(
    result: Result<Value, String>,
    label: &str,
) -> Result<T, String> {
    let value = result?;
    serde_json::from_value(value)
        .map_err(|err| format!("invalid Task Node {label} response: {err}"))
}

fn tasknode_loading_selection_params(
    view_id: &'static str,
    title: String,
    message: String,
) -> SelectionViewParams {
    let mut header = ColumnRenderable::new();
    header.push(Line::from(title.bold()));
    header.push(Line::from(message.dim()));
    SelectionViewParams {
        view_id: Some(view_id),
        footer_hint: Some(standard_popup_hint_line()),
        header: Box::new(header),
        items: vec![SelectionItem {
            name: "Loading...".to_string(),
            description: Some("Waiting for Task Node.".to_string()),
            is_disabled: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn tasknode_error_selection_params(
    view_id: &'static str,
    title: String,
    message: String,
) -> SelectionViewParams {
    let mut header = ColumnRenderable::new();
    header.push(Line::from(title.bold()));
    header.push(Line::from(message.clone().red()));
    SelectionViewParams {
        view_id: Some(view_id),
        footer_hint: Some(standard_popup_hint_line()),
        header: Box::new(header),
        items: vec![SelectionItem {
            name: "Task Node request failed".to_string(),
            description: Some(message),
            is_disabled: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn tasknode_menu_params(
    state: Result<Option<TaskNodeLocalSession>, TaskNodeLocalError>,
    counts: Option<&TaskNodeMenuCountsCache>,
    refresh_error: Option<&str>,
) -> SelectionViewParams {
    let mut header = ColumnRenderable::new();
    header.push(Line::from("Task Node".bold()));
    header.push(Line::from(
        "GitHub-linked tasks, rewards, and account status.".dim(),
    ));
    header.push(Line::from(format!("Origin: {}", tasknode_origin()).dim()));
    let linked = state
        .as_ref()
        .ok()
        .and_then(|session| session.as_ref())
        .is_some_and(|session| session.terminal_token.is_some());
    if let Ok(Some(session)) = &state {
        if session.terminal_token.is_some() {
            let username = session.github_username.as_deref().unwrap_or("");
            header.push(Line::from(
                format!(
                    "Linked{}",
                    if username.is_empty() {
                        String::new()
                    } else {
                        format!(" as {username}")
                    }
                )
                .cyan(),
            ));
            if counts.is_none() && refresh_error.is_none() {
                header.push(Line::from("Counts refreshing...".dim()));
            }
        } else if session.pending_request_id.is_some() {
            header.push(Line::from(
                "Link pending; run Status after browser auth.".cyan(),
            ));
        }
    }
    if let Some(err) = refresh_error {
        header.push(Line::from(format!("Counts unavailable: {err}").red()));
    }

    SelectionViewParams {
        view_id: Some(TASKNODE_MENU_VIEW_ID),
        footer_hint: Some(standard_popup_hint_line()),
        is_searchable: true,
        search_placeholder: Some("Search Task Node actions".to_string()),
        header: Box::new(header),
        items: tasknode_menu_items(state, linked.then_some(counts).flatten()),
        ..Default::default()
    }
}

fn tasknode_menu_count_badge(count: Option<usize>) -> String {
    count
        .map(|count| format!(" ({count})"))
        .unwrap_or_else(|| " (--)".to_string())
}

fn tasknode_menu_items(
    state: Result<Option<TaskNodeLocalSession>, TaskNodeLocalError>,
    counts: Option<&TaskNodeMenuCountsCache>,
) -> Vec<SelectionItem> {
    let session = state.as_ref().ok().and_then(|session| session.as_ref());
    let linked = session.is_some_and(|session| session.terminal_token.is_some());
    let pending = session.is_some_and(|session| session.pending_request_id.is_some());
    let mut items = Vec::new();
    if linked || pending {
        items.push(SelectionItem {
            name: "Status".to_string(),
            description: Some(
                counts
                    .map(|counts| {
                        format!(
                            "{} outstanding, {} verification, {} requests, {} rewards",
                            counts
                                .outstanding
                                .map(|count| count.to_string())
                                .unwrap_or_else(|| "--".to_string()),
                            counts
                                .verification
                                .map(|count| count.to_string())
                                .unwrap_or_else(|| "--".to_string()),
                            counts
                                .request_count
                                .map(|count| count.to_string())
                                .unwrap_or_else(|| "--".to_string()),
                            counts
                                .rewarded
                                .map(|count| count.to_string())
                                .unwrap_or_else(|| "--".to_string()),
                        )
                    })
                    .unwrap_or_else(|| "Poll pending link or show account/task counts".to_string()),
            ),
            actions: vec![Box::new(|tx| tx.send(AppEvent::OpenTaskNodeStatus))],
            dismiss_on_select: true,
            ..Default::default()
        });
    }
    if linked {
        items.extend([
            SelectionItem {
                name: format!(
                    "Outstanding tasks{}",
                    tasknode_menu_count_badge(counts.and_then(|counts| counts.outstanding))
                ),
                description: Some("Review and act on open Task Node work".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenTaskNodeTaskList {
                        tab: "outstanding".to_string(),
                    });
                })],
                dismiss_on_select: false,
                ..Default::default()
            },
            SelectionItem {
                name: format!(
                    "Verification requests{}",
                    tasknode_menu_count_badge(counts.and_then(|counts| counts.verification))
                ),
                description: Some("Tasks waiting on verification evidence".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenTaskNodeTaskList {
                        tab: "verification".to_string(),
                    });
                })],
                dismiss_on_select: false,
                ..Default::default()
            },
            SelectionItem {
                name: "Request personal task".to_string(),
                description: Some("Ask Task Node to generate work from text".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenTaskNodeTaskRequestPrompt)
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Context document".to_string(),
                description: Some("Review or edit the current Task Node context".to_string()),
                actions: vec![Box::new(|tx| tx.send(AppEvent::OpenTaskNodeContext))],
                dismiss_on_select: false,
                ..Default::default()
            },
            SelectionItem {
                name: format!(
                    "Active task requests{}",
                    tasknode_menu_count_badge(counts.and_then(|counts| counts.request_count))
                ),
                description: Some("Track queued, generating, or failed task requests".to_string()),
                actions: vec![Box::new(|tx| tx.send(AppEvent::OpenTaskNodeRequestList))],
                dismiss_on_select: false,
                ..Default::default()
            },
            SelectionItem {
                name: format!(
                    "Recent rewards{}",
                    tasknode_menu_count_badge(counts.and_then(|counts| counts.rewarded))
                ),
                description: Some("Show rewarded tasks".to_string()),
                actions: vec![Box::new(|tx| tx.send(AppEvent::OpenTaskNodeRewards))],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Balance".to_string(),
                description: Some("Read linked-wallet PFT balance".to_string()),
                actions: vec![Box::new(|tx| tx.send(AppEvent::OpenTaskNodeBalance))],
                dismiss_on_select: true,
                ..Default::default()
            },
        ]);
    }
    items.push(SelectionItem {
        name: if linked || pending {
            "Relink GitHub / Task Node".to_string()
        } else {
            "Link GitHub / Task Node".to_string()
        },
        description: Some("Open GitHub OAuth through Task Node".to_string()),
        actions: vec![Box::new(|tx| tx.send(AppEvent::OpenTaskNodeLink))],
        dismiss_on_select: true,
        ..Default::default()
    });
    if linked || pending {
        items.push(SelectionItem {
            name: "Logout Task Node".to_string(),
            description: Some("Remove local terminal session".to_string()),
            actions: vec![Box::new(|tx| tx.send(AppEvent::LogoutTaskNode))],
            dismiss_on_select: true,
            ..Default::default()
        });
    }
    if let Err(err) = state {
        items.push(SelectionItem {
            name: "Local session unavailable".to_string(),
            description: Some(err.to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }
    items
}

fn tasknode_task_items(tasks: Vec<TaskNodeTask>) -> Vec<SelectionItem> {
    if tasks.is_empty() {
        return vec![SelectionItem {
            name: "No tasks".to_string(),
            description: Some("Task Node did not return tasks for this tab.".to_string()),
            is_disabled: true,
            dismiss_on_select: false,
            ..Default::default()
        }];
    }
    tasks
        .into_iter()
        .map(|task| {
            let task_id = tasknode_task_id(&task);
            let status = tasknode_task_status(&task);
            let pft = tasknode_pft_text(task.pft.as_ref());
            let due = task
                .full_due
                .clone()
                .or(task.due.clone())
                .unwrap_or_default();
            SelectionItem {
                name: task.title.unwrap_or_else(|| task_id.clone()),
                description: Some(format!(
                    "{}   {} PFT   {}",
                    status,
                    pft,
                    if due.is_empty() {
                        "No deadline"
                    } else {
                        due.as_str()
                    }
                )),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenTaskNodeTaskActions {
                        task_id: task_id.clone(),
                    });
                })],
                dismiss_on_select: false,
                ..Default::default()
            }
        })
        .collect()
}

fn tasknode_task_id(task: &TaskNodeTask) -> String {
    task.task_id
        .clone()
        .or(task.full_id.clone())
        .or(task.id.clone())
        .unwrap_or_default()
}

fn tasknode_task_status(task: &TaskNodeTask) -> String {
    task.status
        .clone()
        .or(task.status_key.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

fn tasknode_pft_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => "0".to_string(),
        Some(other) => other.to_string().trim_matches('"').to_string(),
    }
}

fn tasknode_wrapped_paragraph(text: impl Into<String>) -> Paragraph<'static> {
    Paragraph::new(text.into()).wrap(Wrap { trim: false })
}

fn push_tasknode_wrapped_line(header: &mut ColumnRenderable<'static>, text: impl Into<String>) {
    header.push(tasknode_wrapped_paragraph(text));
}

fn push_tasknode_section(header: &mut ColumnRenderable<'static>, title: &str, body: &str) {
    let body = body.trim();
    if body.is_empty() {
        return;
    }
    header.push(Line::from(""));
    header.push(Line::from(Span::from(title.to_string()).bold().cyan()));
    header.push(tasknode_wrapped_paragraph(body.to_string()));
}

fn tasknode_task_detail_header(
    detail: &TaskNodeTaskDetailResponse,
    fallback_task_id: &str,
) -> ColumnRenderable<'static> {
    let task = detail.task.as_ref();
    let task_id = task
        .map(tasknode_task_id)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| fallback_task_id.to_string());
    let title = task
        .and_then(|task| task.title.clone())
        .unwrap_or_else(|| task_id.clone());
    let status = task.map(tasknode_task_status).unwrap_or_default();
    let kind = task.and_then(|task| task.kind.clone()).unwrap_or_default();
    let pft = tasknode_pft_text(task.and_then(|task| task.pft.as_ref()));
    let due_label = task
        .and_then(|task| task.due_label.clone())
        .unwrap_or_else(|| "Deadline".to_string());
    let due = task
        .and_then(|task| task.full_due.clone().or(task.due.clone()))
        .unwrap_or_else(|| "No deadline".to_string());

    let mut header: ColumnRenderable<'static> = ColumnRenderable::new();
    header.push(Line::from(title.bold()));
    header.push(Line::from(task_id.clone().dim()));
    header.push(Line::from(
        [
            status,
            kind,
            format!("{pft} PFT"),
            format!("{due_label}: {due}"),
        ]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
        .dim(),
    ));

    if let Some(task) = task {
        let task_metadata = [
            task.metadata
                .as_ref()
                .and_then(|metadata| {
                    metadata
                        .get("requestId")
                        .or_else(|| metadata.get("request_id"))
                        .and_then(Value::as_str)
                })
                .filter(|request_id| !request_id.is_empty())
                .map(|request_id| format!("Request: {request_id}")),
            task.updated_at
                .as_ref()
                .filter(|updated_at| !updated_at.is_empty())
                .map(|updated_at| format!("Updated: {updated_at}")),
            task.last_event_at
                .as_ref()
                .filter(|last_event_at| !last_event_at.is_empty())
                .map(|last_event_at| format!("Last event: {last_event_at}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        push_tasknode_section(&mut header, "Task Metadata", &task_metadata);
        push_tasknode_section(
            &mut header,
            "Objective",
            task.description
                .as_deref()
                .unwrap_or("No description provided."),
        );
        if !task.steps.is_empty() {
            header.push(Line::from(""));
            header.push(Line::from("Steps".bold().cyan()));
            for (index, step) in task.steps.iter().enumerate() {
                push_tasknode_wrapped_line(&mut header, format!("{}. {step}", index + 1));
            }
        }
        let verification = task
            .verification
            .as_ref()
            .and_then(|verification| verification.body.clone().or(verification.title.clone()))
            .or_else(|| {
                task.submission_requirement
                    .as_ref()
                    .and_then(|requirement| requirement.criteria.clone())
            })
            .unwrap_or_else(|| "Submit evidence that satisfies the task requirement.".to_string());
        push_tasknode_section(&mut header, "Verification", &verification);
    }

    if let Some(request) = &detail.current_verification_request {
        let mut body = request
            .body
            .clone()
            .or(request.ask.clone())
            .unwrap_or_default();
        if let Some(reason) = &request.reason {
            if !reason.is_empty() {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(&format!("Reason: {reason}"));
            }
        }
        push_tasknode_section(&mut header, "Current Verification Request", &body);
    }

    if let Some(outcome) = &detail.reward_outcome {
        let reward = outcome
            .get("rewardPft")
            .or_else(|| outcome.get("reward_pft"))
            .map(|value| tasknode_pft_text(Some(value)))
            .unwrap_or_default();
        let decision = outcome
            .get("decision")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let reason = outcome
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let feedback = outcome
            .get("userFeedback")
            .or_else(|| outcome.get("user_feedback"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = [
            (!decision.is_empty()).then(|| format!("Decision: {decision}")),
            (!reward.is_empty()).then(|| format!("Reward: {reward} PFT")),
            (!reason.is_empty()).then(|| format!("Reason: {reason}")),
            (!feedback.is_empty()).then(|| format!("Feedback: {feedback}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        push_tasknode_section(&mut header, "Reward Outcome", &body);
    }

    if let Some(forensics) = &detail.forensics {
        let mut parts = Vec::new();
        if let Some(count) = forensics.event_count {
            parts.push(format!("{count} indexed event(s)"));
        }
        if let Some(tx) = &forensics.last_event_tx_hash {
            if !tx.is_empty() {
                parts.push(format!("last tx {tx}"));
            }
        }
        if !parts.is_empty() {
            push_tasknode_section(&mut header, "Forensics", &parts.join("\n"));
        }
    }

    header
}

fn tasknode_task_action_items(
    task_id: String,
    actions: Option<TaskNodeActions>,
) -> Vec<SelectionItem> {
    let actions = actions.unwrap_or_default();
    let mut items = Vec::new();
    if actions.can_accept {
        items.push(tasknode_action_item(
            "Accept task",
            "accept",
            task_id.clone(),
        ));
    }
    if actions.can_refuse {
        items.push(tasknode_action_item(
            "Refuse task",
            "refuse",
            task_id.clone(),
        ));
    }
    if actions.can_cancel {
        items.push(tasknode_action_item(
            "Cancel task",
            "cancel",
            task_id.clone(),
        ));
    }
    if actions.can_submit_initial_evidence || actions.can_submit_verification_evidence {
        let evidence_task_id = task_id.clone();
        items.push(SelectionItem {
            name: if actions.can_submit_verification_evidence {
                "Submit verification evidence".to_string()
            } else {
                "Submit evidence".to_string()
            },
            description: Some("Open task-aware evidence prompt".to_string()),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenTaskNodeEvidencePrompt {
                    task_id: evidence_task_id.clone(),
                });
            })],
            dismiss_on_select: true,
            ..Default::default()
        });
    }
    items.push(SelectionItem {
        name: "Copy task brief".to_string(),
        description: Some("Copy objective, steps, verification, and requested output".to_string()),
        actions: vec![Box::new({
            let task_id = task_id.clone();
            move |tx| {
                tx.send(AppEvent::CopyTaskNodeTaskBrief {
                    task_id: task_id.clone(),
                });
            }
        })],
        dismiss_on_select: true,
        ..Default::default()
    });
    if items.is_empty() {
        items.push(SelectionItem {
            name: "No task actions available".to_string(),
            description: Some("This task is terminal or waiting on another actor.".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }
    items
}

fn tasknode_action_item(name: &str, action: &str, task_id: String) -> SelectionItem {
    let action = action.to_string();
    SelectionItem {
        name: name.to_string(),
        description: Some("Server-backed Task Node action".to_string()),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::SubmitTaskNodeTaskAction {
                task_id: task_id.clone(),
                action: action.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn tab_title(tab: &str) -> &'static str {
    match tab {
        "verification" => "Verification",
        "refused" => "Refused",
        "rewarded" => "Rewarded",
        _ => "Outstanding",
    }
}

fn tasknode_status_lines(status: &TaskNodeStatusResponse) -> Vec<Line<'static>> {
    vec![
        Line::from(vec!["Task Node status".bold().cyan()]),
        Line::from(vec![
            "  account: ".dim(),
            status.account_id.clone().unwrap_or_default().into(),
        ]),
        Line::from(vec![
            "  github:  ".dim(),
            status.github.username.clone().unwrap_or_default().into(),
        ]),
        Line::from(vec![
            "  wallet:  ".dim(),
            if status.wallet.linked {
                status.wallet.address.clone().unwrap_or_default()
            } else {
                "not linked".to_string()
            }
            .into(),
        ]),
        Line::from(vec![
            "  tasks:   ".dim(),
            format!(
                "{} outstanding, {} verification, {} refused, {} rewarded",
                status.counts.outstanding,
                status.counts.verification,
                status.counts.refused,
                status.counts.rewarded
            )
            .into(),
        ]),
        Line::from(vec![
            "  actions: ".dim(),
            if status.server.terminal_task_actions {
                "terminal direct-write enabled".to_string()
            } else {
                "wallet handoff required".to_string()
            }
            .into(),
        ]),
    ]
}

fn tasknode_value_lines(title: &str, value: &Value) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![title.to_string().bold().cyan()])];
    match serde_json::to_string_pretty(value) {
        Ok(text) => lines.extend(text.lines().map(|line| Line::from(format!("  {line}")))),
        Err(_) => lines.push(Line::from("  unavailable")),
    }
    lines
}

fn tasknode_rewards_lines(value: &Value) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec!["Task Node recent rewards".bold().cyan()])];
    let rewards = value
        .get("rewards")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rewards.is_empty() {
        lines.push(Line::from("  No recent rewards returned."));
        return lines;
    }
    for reward in rewards.into_iter().take(10) {
        let title = reward
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled task");
        let pft = reward
            .get("pft")
            .map(Value::to_string)
            .unwrap_or_else(|| "0".to_string());
        lines.push(Line::from(vec![
            "  • ".into(),
            Span::from(title.to_string()).cyan(),
            "  ".into(),
            Span::from(format!("{pft} PFT")).dim(),
        ]));
    }
    lines
}

fn tasknode_task_brief(detail: &TaskNodeTaskDetailResponse) -> String {
    detail
        .terminal
        .as_ref()
        .and_then(|terminal| terminal.brief_text.clone())
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| tasknode_local_task_brief(detail))
}

fn tasknode_local_task_brief(detail: &TaskNodeTaskDetailResponse) -> String {
    let task = detail.task.as_ref();
    let title = task
        .and_then(|task| task.title.clone())
        .unwrap_or_else(|| "Untitled task".to_string());
    let task_id = task.map(tasknode_task_id).unwrap_or_default();
    let status = task.map(tasknode_task_status).unwrap_or_default();
    let kind = task.and_then(|task| task.kind.clone()).unwrap_or_default();
    let pft = tasknode_pft_text(task.and_then(|task| task.pft.as_ref()));
    let due = task
        .and_then(|task| task.full_due.clone().or(task.due.clone()))
        .unwrap_or_default();
    let description = task
        .and_then(|task| task.description.clone())
        .unwrap_or_else(|| "No description provided.".to_string());
    let mut lines = vec![
        "Task for Codex".to_string(),
        String::new(),
        format!("Title: {title}"),
    ];
    if !task_id.is_empty() {
        lines.push(format!("Task ID: {task_id}"));
    }
    if !kind.is_empty() {
        lines.push(format!("Kind: {kind}"));
    }
    if !status.is_empty() {
        lines.push(format!("Status: {status}"));
    }
    lines.push(format!("Reward: {pft} PFT"));
    if !due.is_empty() {
        lines.push(format!("Deadline: {due}"));
    }
    lines.extend([String::new(), "Objective".to_string(), description]);
    if let Some(task) = task {
        if !task.steps.is_empty() {
            lines.extend([String::new(), "Steps".to_string()]);
            lines.extend(
                task.steps
                    .iter()
                    .enumerate()
                    .map(|(index, step)| format!("{}. {step}", index + 1)),
            );
        }
        let verification = task
            .verification
            .as_ref()
            .and_then(|verification| verification.body.clone().or(verification.title.clone()))
            .or_else(|| {
                task.submission_requirement
                    .as_ref()
                    .and_then(|requirement| requirement.criteria.clone())
            })
            .unwrap_or_else(|| "Submit evidence that satisfies the task requirement.".to_string());
        lines.extend([
            String::new(),
            "Verification Requirements".to_string(),
            verification,
        ]);
    }
    if let Some(request) = &detail.current_verification_request {
        let body = request
            .body
            .clone()
            .or(request.ask.clone())
            .unwrap_or_default();
        if !body.is_empty() {
            lines.extend([
                String::new(),
                "Current Verification Request".to_string(),
                body,
            ]);
        }
    }
    lines.extend([
        String::new(),
        "Requested Output".to_string(),
        "Complete the task and return the evidence needed for the verification requirement. Include changed files, commands run, test results, links, screenshots, or concise proof artifacts when relevant.".to_string(),
    ]);
    lines.join("\n")
}

fn tasknode_evidence_context_lines(
    detail: &TaskNodeTaskDetailResponse,
    fallback_task_id: &str,
) -> Vec<Line<'static>> {
    let task = detail.task.as_ref();
    let title = task
        .and_then(|task| task.title.clone())
        .unwrap_or_else(|| fallback_task_id.to_string());
    let task_id = task
        .map(tasknode_task_id)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| fallback_task_id.to_string());
    let mut lines = vec![
        Line::from(vec!["Task Node evidence context".bold().cyan()]),
        Line::from(title.bold()),
        Line::from(task_id.dim()),
    ];
    if let Some(task) = task {
        if let Some(description) = &task.description {
            if !description.trim().is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from("Objective".bold()));
                lines.push(Line::from(description.clone()));
            }
        }
        if !task.steps.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("Steps".bold()));
            for (index, step) in task.steps.iter().take(5).enumerate() {
                lines.push(Line::from(format!("{}. {step}", index + 1)));
            }
        }
        let verification = task
            .verification
            .as_ref()
            .and_then(|verification| verification.body.clone().or(verification.title.clone()))
            .or_else(|| {
                task.submission_requirement
                    .as_ref()
                    .and_then(|requirement| requirement.criteria.clone())
            })
            .unwrap_or_default();
        if !verification.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("Verification requirement".bold()));
            lines.push(Line::from(verification));
        }
    }
    if let Some(request) = &detail.current_verification_request {
        let body = request
            .body
            .clone()
            .or(request.ask.clone())
            .unwrap_or_default();
        if !body.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("Current verification request".bold()));
            lines.push(Line::from(body));
        }
        if let Some(reason) = &request.reason {
            if !reason.is_empty() {
                lines.push(Line::from(format!("Reason: {reason}").dim()));
            }
        }
    }
    lines
}

fn tasknode_evidence_context_fallback_lines(fallback_task_id: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(vec!["Task Node evidence context unavailable".bold().yellow()]),
        Line::from(fallback_task_id.to_string().dim()),
        Line::from(
            "Submit the evidence directly. Include the proof text, command result, PR URL, commit, or concise verification response."
                .to_string(),
        ),
    ]
}

fn tasknode_evidence_placeholder(prompt: &TaskNodeEvidencePrompt) -> String {
    let examples = if prompt.examples.is_empty() {
        "PR URL, commit URL, terminal output, test result, or concise proof".to_string()
    } else {
        prompt.examples.join(", ")
    };
    format!(
        "{}: {examples}",
        prompt.mode.as_deref().unwrap_or("Evidence")
    )
}

fn tasknode_task_request_result_lines(value: &Value) -> Vec<Line<'static>> {
    let request_id = value
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Task request recorded.");
    vec![
        Line::from(vec!["Task request recorded".bold().cyan()]),
        Line::from(format!("  {message}")),
        Line::from(format!("  request: {request_id}")),
        Line::from("  Run /tasknode requests to track generation."),
    ]
}

fn tasknode_context_header(context: &TaskNodeContextDocument) -> ColumnRenderable<'static> {
    let mut header = ColumnRenderable::new();
    header.push(Line::from("Task Node context".bold()));
    header.push(Line::from(tasknode_context_title(context).cyan()));
    header.push(Line::from(tasknode_context_summary(context).dim()));
    header
}

fn tasknode_context_items(context: TaskNodeContextDocument) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    let title = tasknode_context_title(&context);
    let body = context.body.clone();
    let revision = context.revision;
    let body_format = context
        .terminal
        .as_ref()
        .and_then(|terminal| terminal.editable_body_format.clone())
        .unwrap_or_else(|| "text".to_string());
    items.push(SelectionItem {
        name: "Edit context document".to_string(),
        description: Some("Open a multiline editor seeded with the current context".to_string()),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenTaskNodeContextEdit {
                title: title.clone(),
                body: body.clone(),
                revision,
                body_format: body_format.clone(),
            });
        })],
        is_disabled: !context.can_edit,
        disabled_reason: (!context.can_edit).then(|| "Context is read-only.".to_string()),
        dismiss_on_select: true,
        ..Default::default()
    });
    items.push(SelectionItem {
        name: "Refresh context document".to_string(),
        description: Some("Reload context from Task Node".to_string()),
        actions: vec![Box::new(|tx| tx.send(AppEvent::OpenTaskNodeContext))],
        dismiss_on_select: false,
        ..Default::default()
    });
    items
}

fn tasknode_context_lines(context: &TaskNodeContextDocument) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec!["Task Node context".bold().cyan()]),
        Line::from(vec![
            "  title:    ".dim(),
            tasknode_context_title(context).into(),
        ]),
        Line::from(vec![
            "  revision: ".dim(),
            context.revision.to_string().into(),
        ]),
        Line::from(vec![
            "  updated:  ".dim(),
            context.updated_at.clone().unwrap_or_default().into(),
        ]),
        Line::from(vec![
            "  digest:   ".dim(),
            context
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.digest.clone())
                .unwrap_or_default()
                .into(),
        ]),
        Line::from(vec![
            "  size:     ".dim(),
            tasknode_context_size(context).into(),
        ]),
        Line::from(""),
    ];

    let body = context
        .body_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(context.body.as_str());
    if body.trim().is_empty() {
        lines.push(Line::from("  Context document is empty."));
        return lines;
    }

    for source_line in body.lines() {
        if source_line.trim().is_empty() {
            lines.push(Line::from(""));
            continue;
        }
        for wrapped in tasknode_soft_wrap_line(source_line, 100) {
            lines.push(Line::from(wrapped));
        }
    }
    lines
}

fn tasknode_context_title(context: &TaskNodeContextDocument) -> String {
    if context.title.trim().is_empty() {
        "Task Node Context".to_string()
    } else {
        context.title.clone()
    }
}

fn tasknode_context_summary(context: &TaskNodeContextDocument) -> String {
    [
        format!("revision {}", context.revision),
        context
            .updated_at
            .as_ref()
            .filter(|updated_at| !updated_at.is_empty())
            .map(|updated_at| format!("updated {updated_at}"))
            .unwrap_or_default(),
        context
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.digest.clone())
            .unwrap_or_default(),
        tasknode_context_size(context),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" | ")
}

fn tasknode_context_size(context: &TaskNodeContextDocument) -> String {
    if let Some(terminal) = &context.terminal {
        return format!(
            "{} words, {} lines, {} chars",
            terminal.word_count.unwrap_or(0),
            terminal.line_count.unwrap_or(0),
            terminal.char_count.unwrap_or(0)
        );
    }
    let body = context
        .body_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(context.body.as_str());
    let words = body.split_whitespace().count();
    let lines = body.lines().count().max(1);
    format!("{words} words, {lines} lines, {} chars", body.len())
}

fn tasknode_soft_wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.chars().count() <= width {
        return vec![line.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in line.split_whitespace() {
        let word_len = word.chars().count();
        let current_len = current.chars().count();
        if current_len > 0 && current_len + 1 + word_len > width {
            lines.push(current);
            current = String::new();
        }
        if word_len > width {
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
            }
            let mut chunk = String::new();
            for ch in word.chars() {
                if chunk.chars().count() >= width {
                    lines.push(chunk);
                    chunk = String::new();
                }
                chunk.push(ch);
            }
            if !chunk.is_empty() {
                current = chunk;
            }
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn tasknode_request_items(requests: Vec<TaskNodeRequestRow>) -> Vec<SelectionItem> {
    if requests.is_empty() {
        return vec![SelectionItem {
            name: "No active task requests".to_string(),
            description: Some("Task Node did not return queued or recent requests.".to_string()),
            is_disabled: true,
            ..Default::default()
        }];
    }
    requests
        .into_iter()
        .map(|request| {
            let generated_task_id = request.generated_task_id.clone().unwrap_or_default();
            let is_pending = generated_task_id.is_empty();
            let request_id = request.request_id.clone();
            let text = request.user_detail_text.clone().unwrap_or_default();
            let description = [
                request.status_label.or(request.status).unwrap_or_default(),
                request.ago.unwrap_or_default(),
                text.chars().take(80).collect::<String>(),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("   ");
            SelectionItem {
                name: if request_id.is_empty() {
                    "Task request".to_string()
                } else {
                    request_id
                },
                description: Some(description),
                actions: if is_pending {
                    Vec::new()
                } else {
                    let generated_task_id = generated_task_id.clone();
                    vec![Box::new(move |tx| {
                        tx.send(AppEvent::OpenTaskNodeTaskActions {
                            task_id: generated_task_id.clone(),
                        });
                    })]
                },
                is_disabled: is_pending,
                disabled_reason: is_pending
                    .then(|| "Request has not generated a visible task yet.".to_string()),
                dismiss_on_select: false,
                ..Default::default()
            }
        })
        .collect()
}

fn tasknode_response_hint(value: &Value) -> Option<String> {
    value
        .get("message")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            value
                .get("txHash")
                .and_then(Value::as_str)
                .map(|tx| format!("tx: {tx}"))
        })
}

fn ensure_tasknode_session(codex_home: &Path) -> Result<TaskNodeLocalSession, TaskNodeLocalError> {
    let Some(mut session) = TaskNodeLocalState::load(codex_home)? else {
        return Err(TaskNodeLocalError::NoSession);
    };
    if session.terminal_token.is_some() {
        return Ok(session);
    }
    let request_id = session
        .pending_request_id
        .clone()
        .ok_or(TaskNodeLocalError::NoSession)?;
    let poll_token = session
        .pending_poll_token
        .clone()
        .ok_or(TaskNodeLocalError::NoSession)?;
    match TaskNodeClient::new_without_token().poll_session(&request_id, &poll_token) {
        Ok(poll) => {
            session.account_id = Some(poll.account_id);
            session.github_username = poll.github_username;
            session.terminal_token = Some(poll.terminal_token);
            session.expires_at = poll.expires_at;
            session.pending_request_id = None;
            session.pending_poll_token = None;
            session.pending_verification_url = None;
            session.save(codex_home)?;
            Ok(session)
        }
        Err(TaskNodeClientError::Pending) => Err(TaskNodeLocalError::Pending {
            verification_url: session.pending_verification_url.unwrap_or_default(),
        }),
        Err(err) => Err(TaskNodeLocalError::Client(err.to_string())),
    }
}

#[derive(Debug)]
enum TaskNodeLocalState {}

impl TaskNodeLocalState {
    fn load(codex_home: &Path) -> Result<Option<TaskNodeLocalSession>, TaskNodeLocalError> {
        let vault = Vault::new(codex_home.to_path_buf());
        match vault.reveal(TASKNODE_SESSION_LABEL) {
            Ok(secret) => serde_json::from_str(&secret).map(Some).map_err(|err| {
                TaskNodeLocalError::Client(format!("invalid local Task Node session: {err}"))
            }),
            Err(VaultError::NotFound { .. }) => Ok(None),
            Err(err) => Err(TaskNodeLocalError::Vault(err.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskNodeLocalSession {
    origin: String,
    account_id: Option<String>,
    github_username: Option<String>,
    terminal_token: Option<String>,
    expires_at: Option<String>,
    pending_request_id: Option<String>,
    pending_poll_token: Option<String>,
    pending_verification_url: Option<String>,
}

impl TaskNodeLocalSession {
    fn save(&self, codex_home: &Path) -> Result<(), TaskNodeLocalError> {
        let vault = Vault::new(codex_home.to_path_buf());
        let secret = serde_json::to_string(self).map_err(|err| {
            TaskNodeLocalError::Client(format!("serialize session failed: {err}"))
        })?;
        match vault.add(AddCredential {
            label: TASKNODE_SESSION_LABEL.to_string(),
            credential_type: CredentialType::BearerToken,
            provider: Some("tasknode".to_string()),
            notes: Some("Task Node terminal session; token is not printed to chat.".to_string()),
            revocation_notes: Some(format!("{}/settings/accounts", self.origin)),
            secret: secret.clone(),
        }) {
            Ok(()) => Ok(()),
            Err(VaultError::CredentialExists { .. }) => vault
                .update(
                    TASKNODE_SESSION_LABEL,
                    Some(secret),
                    Some(Some("tasknode".to_string())),
                    None,
                    None,
                )
                .map(|_| ())
                .map_err(|err| TaskNodeLocalError::Vault(err.to_string())),
            Err(err) => Err(TaskNodeLocalError::Vault(err.to_string())),
        }
    }
}

#[derive(Debug)]
enum TaskNodeLocalError {
    NoSession,
    Pending { verification_url: String },
    Vault(String),
    Client(String),
}

impl std::fmt::Display for TaskNodeLocalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => write!(f, "Task Node is not linked. Run /tasknode link."),
            Self::Pending { verification_url } => {
                write!(
                    f,
                    "Task Node link is pending. Finish GitHub auth: {verification_url}"
                )
            }
            Self::Vault(err) | Self::Client(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TaskNodeLocalError {}

struct TaskNodeClient {
    origin: String,
    token: Option<String>,
}

impl TaskNodeClient {
    fn new(token: String) -> Self {
        Self {
            origin: tasknode_origin(),
            token: Some(token),
        }
    }

    fn new_without_token() -> Self {
        Self {
            origin: tasknode_origin(),
            token: None,
        }
    }

    fn start_github_link(&self) -> Result<TerminalAuthStartResponse, String> {
        self.post_json("/api/auth/terminal/start/github", &serde_json::json!({}))
            .map_err(|err| err.to_string())
    }

    fn poll_session(
        &self,
        request_id: &str,
        poll_token: &str,
    ) -> Result<TerminalSessionResponse, TaskNodeClientError> {
        let path = format!(
            "/api/auth/terminal/session?requestId={}&pollToken={}",
            urlencoding::encode(request_id),
            urlencoding::encode(poll_token)
        );
        self.get_json(&path)
    }

    fn status(&self) -> Result<Value, String> {
        self.get_json("/api/terminal/tasknode/status")
            .map_err(|err| err.to_string())
    }

    fn tasks(&self, tab: &str) -> Result<Value, String> {
        self.get_json(&format!(
            "/api/terminal/tasknode/tasks?tab={}",
            urlencoding::encode(tab)
        ))
        .map_err(|err| err.to_string())
    }

    fn task_detail(&self, task_id: &str) -> Result<Value, String> {
        self.get_json(&format!(
            "/api/terminal/tasknode/tasks/{}",
            urlencoding::encode(task_id)
        ))
        .map_err(|err| err.to_string())
    }

    fn task_action(&self, task_id: &str, action: &str) -> Result<Value, String> {
        self.post_json(
            &format!(
                "/api/terminal/tasknode/tasks/{}/action",
                urlencoding::encode(task_id)
            ),
            &serde_json::json!({
                "action": action,
                "source": "pfterminal",
                "idempotencyKey": format!("pfterminal:{}:{action}", Uuid::new_v4()),
            }),
        )
        .map_err(|err| err.to_string())
    }

    fn submit_evidence(&self, task_id: &str, summary: &str) -> Result<Value, String> {
        let evidence = evidence_items_from_summary(summary);
        self.post_json(
            &format!(
                "/api/terminal/tasknode/tasks/{}/evidence",
                urlencoding::encode(task_id)
            ),
            &serde_json::json!({
                "summary": summary,
                "evidence": evidence,
                "source": "pfterminal",
                "idempotencyKey": format!("pfterminal-evidence:{}:{}", task_id, Uuid::new_v4()),
            }),
        )
        .map_err(|err| err.to_string())
    }

    fn balance(&self) -> Result<Value, String> {
        self.get_json("/api/terminal/tasknode/balance")
            .map_err(|err| err.to_string())
    }

    fn rewards(&self) -> Result<Value, String> {
        self.get_json("/api/terminal/tasknode/rewards?limit=10")
            .map_err(|err| err.to_string())
    }

    fn task_requests(&self) -> Result<Value, String> {
        self.get_json("/api/terminal/tasknode/requests?limit=20")
            .map_err(|err| err.to_string())
    }

    fn request_task(&self, detail: &str) -> Result<Value, String> {
        self.post_json(
            "/api/terminal/tasknode/requests",
            &serde_json::json!({
                "userDetailText": detail,
                "requestedTaskKind": "personal",
                "source": "pfterminal",
                "sourceConversationTitle": "PFTerminal",
                "idempotencyKey": format!("pfterminal-request:{}", Uuid::new_v4()),
            }),
        )
        .map_err(|err| err.to_string())
    }

    fn context(&self) -> Result<Value, String> {
        self.get_json("/api/terminal/tasknode/context")
            .map_err(|err| err.to_string())
    }

    fn save_context(&self, title: &str, body: &str, revision: u64) -> Result<Value, String> {
        self.post_json(
            "/api/terminal/tasknode/context",
            &serde_json::json!({
                "title": title,
                "body": body,
                "revision": revision,
                "source": "pfterminal",
            }),
        )
        .map_err(|err| err.to_string())
    }

    fn revoke(&self) -> Result<Value, String> {
        self.post_json("/api/auth/terminal/revoke", &serde_json::json!({}))
            .map_err(|err| err.to_string())
    }

    fn get_json<T: DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
    ) -> Result<T, TaskNodeClientError> {
        let url = format!("{}{}", self.origin, path);
        let token = self.token.clone();
        tasknode_blocking_http(move || {
            let http = tasknode_http_client()?;
            let mut request = http.get(url);
            if let Some(token) = &token {
                request = request.bearer_auth(token);
            }
            parse_tasknode_response(request.send())
        })
    }

    fn post_json<T: DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
        body: &Value,
    ) -> Result<T, TaskNodeClientError> {
        let url = format!("{}{}", self.origin, path);
        let token = self.token.clone();
        let body = body.clone();
        tasknode_blocking_http(move || {
            let http = tasknode_http_client()?;
            let mut request = http.post(url).json(&body);
            if let Some(token) = &token {
                request = request.bearer_auth(token);
            }
            parse_tasknode_response(request.send())
        })
    }
}

fn tasknode_http_client() -> Result<reqwest::blocking::Client, TaskNodeClientError> {
    reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|err| TaskNodeClientError::Http(tasknode_reqwest_error(err)))
}

fn tasknode_blocking_http<T: Send + 'static>(
    request: impl FnOnce() -> Result<T, TaskNodeClientError> + Send + 'static,
) -> Result<T, TaskNodeClientError> {
    let handle = std::thread::Builder::new()
        .name("tasknode-http".to_string())
        .spawn(request)
        .map_err(|err| TaskNodeClientError::Http(err.to_string()))?;
    handle
        .join()
        .map_err(|_| TaskNodeClientError::Http("Task Node HTTP worker panicked".to_string()))?
}

fn parse_tasknode_response<T: DeserializeOwned>(
    response: Result<reqwest::blocking::Response, reqwest::Error>,
) -> Result<T, TaskNodeClientError> {
    let response =
        response.map_err(|err| TaskNodeClientError::Http(tasknode_reqwest_error(err)))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .map_err(|err| TaskNodeClientError::Http(tasknode_reqwest_error(err)))?;
    if status == 202 {
        return Err(TaskNodeClientError::Pending);
    }
    if !(200..300).contains(&status) {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .or_else(|| value.get("error"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or(text);
        return Err(TaskNodeClientError::Http(message));
    }
    serde_json::from_str(&text).map_err(|err| TaskNodeClientError::Http(err.to_string()))
}

fn tasknode_reqwest_error(err: reqwest::Error) -> String {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(err) = source {
        let part = err.to_string();
        if !part.is_empty() && !message.contains(&part) {
            message.push_str(": ");
            message.push_str(&part);
        }
        source = std::error::Error::source(err);
    }
    message
}

fn evidence_items_from_summary(summary: &str) -> Vec<Value> {
    summary
        .split_whitespace()
        .filter(|part| part.starts_with("http://") || part.starts_with("https://"))
        .take(5)
        .map(|url| {
            let item_type = if url.contains("github.com/") && url.contains("/pull/") {
                "github_pr"
            } else if url.contains("github.com/") && url.contains("/commit/") {
                "git_commit"
            } else {
                "url"
            };
            serde_json::json!({ "type": item_type, "url": url })
        })
        .collect()
}

#[derive(Debug)]
enum TaskNodeClientError {
    Pending,
    Http(String),
}

impl std::fmt::Display for TaskNodeClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Http(err) => write!(f, "{err}"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TerminalAuthStartResponse {
    #[serde(rename = "requestId")]
    request_id: String,
    #[serde(rename = "pollToken")]
    poll_token: String,
    #[serde(rename = "verificationUrl")]
    verification_url: String,
}

#[derive(Debug, Deserialize)]
struct TerminalSessionResponse {
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "githubUsername")]
    github_username: Option<String>,
    #[serde(rename = "terminalToken")]
    terminal_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TaskNodeStatusResponse {
    #[serde(rename = "accountId")]
    account_id: Option<String>,
    #[serde(default)]
    github: TaskNodeGithubStatus,
    #[serde(default)]
    wallet: TaskNodeWalletStatus,
    #[serde(default)]
    counts: TaskNodeCounts,
    #[serde(default)]
    server: TaskNodeServerStatus,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeGithubStatus {
    username: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeWalletStatus {
    linked: bool,
    address: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeCounts {
    outstanding: usize,
    verification: usize,
    refused: usize,
    rewarded: usize,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeServerStatus {
    #[serde(rename = "terminalTaskActions")]
    terminal_task_actions: bool,
}

#[derive(Debug, Deserialize)]
struct TaskNodeTasksResponse {
    tab: String,
    #[serde(default)]
    tasks: Vec<TaskNodeTask>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeTask {
    id: Option<String>,
    #[serde(rename = "taskId")]
    task_id: Option<String>,
    #[serde(rename = "fullId")]
    full_id: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    status: Option<String>,
    #[serde(rename = "statusKey")]
    status_key: Option<String>,
    pft: Option<Value>,
    due: Option<String>,
    #[serde(rename = "fullDue")]
    full_due: Option<String>,
    #[serde(rename = "dueLabel")]
    due_label: Option<String>,
    description: Option<String>,
    #[serde(default)]
    steps: Vec<String>,
    verification: Option<TaskNodeVerification>,
    #[serde(rename = "submissionRequirement")]
    submission_requirement: Option<TaskNodeSubmissionRequirement>,
    metadata: Option<Value>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(rename = "lastEventAt")]
    last_event_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeTaskDetailResponse {
    task: Option<TaskNodeTask>,
    actions: Option<TaskNodeActions>,
    #[serde(rename = "currentVerificationRequest")]
    current_verification_request: Option<TaskNodeVerificationRequest>,
    #[serde(rename = "rewardOutcome")]
    reward_outcome: Option<Value>,
    forensics: Option<TaskNodeForensics>,
    terminal: Option<TaskNodeTerminalRendering>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeActions {
    #[serde(rename = "canAccept")]
    can_accept: bool,
    #[serde(rename = "canRefuse")]
    can_refuse: bool,
    #[serde(rename = "canCancel")]
    can_cancel: bool,
    #[serde(rename = "canSubmitInitialEvidence")]
    can_submit_initial_evidence: bool,
    #[serde(rename = "canSubmitVerificationEvidence")]
    can_submit_verification_evidence: bool,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeVerification {
    title: Option<String>,
    body: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeSubmissionRequirement {
    criteria: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeVerificationRequest {
    body: Option<String>,
    ask: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeForensics {
    #[serde(rename = "eventCount")]
    event_count: Option<usize>,
    #[serde(rename = "lastEventTxHash")]
    last_event_tx_hash: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeTerminalRendering {
    #[serde(rename = "briefText")]
    brief_text: Option<String>,
    #[serde(rename = "evidencePrompt")]
    evidence_prompt: Option<TaskNodeEvidencePrompt>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeEvidencePrompt {
    mode: Option<String>,
    title: Option<String>,
    #[serde(default)]
    examples: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeRequestsResponse {
    #[serde(default)]
    items: Vec<TaskNodeRequestRow>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeContextResponse {
    #[serde(default)]
    context: TaskNodeContextDocument,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeContextSaveResponse {
    message: Option<String>,
    #[serde(default)]
    context: TaskNodeContextDocument,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct TaskNodeContextDocument {
    title: String,
    body: String,
    #[serde(rename = "bodyText")]
    body_text: Option<String>,
    revision: u64,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(rename = "canEdit")]
    can_edit: bool,
    terminal: Option<TaskNodeContextTerminal>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct TaskNodeContextTerminal {
    digest: Option<String>,
    #[serde(rename = "lineCount")]
    line_count: Option<usize>,
    #[serde(rename = "wordCount")]
    word_count: Option<usize>,
    #[serde(rename = "charCount")]
    char_count: Option<usize>,
    #[serde(rename = "editableBodyFormat")]
    editable_body_format: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TaskNodeRequestRow {
    #[serde(rename = "requestId")]
    request_id: String,
    status: Option<String>,
    #[serde(rename = "statusLabel")]
    status_label: Option<String>,
    #[serde(rename = "userDetailText")]
    user_detail_text: Option<String>,
    #[serde(rename = "generatedTaskId")]
    generated_task_id: Option<String>,
    ago: Option<String>,
}
