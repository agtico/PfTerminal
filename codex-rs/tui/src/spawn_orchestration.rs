use crate::app::App;
use crate::app_event::AppEvent;
use crate::app_server_session::AppServerSession;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::custom_prompt_view::CustomPromptView;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::chatwidget::ChatWidget;
use crate::claude_panes::CODEX_MAIN_PANE_ID;
use crate::claude_panes::ClaudeProviderProfileKind;
use crate::multi_agents::agent_picker_status_dot_spans;
use crate::multi_agents::format_agent_picker_item_name;
use crate::session_state::ThreadSessionState;
use chrono::Utc;
use codex_app_server_protocol::AdditionalContextEntry;
use codex_app_server_protocol::AdditionalContextKind;
use codex_app_server_protocol::SessionSource as AppServerSessionSource;
use codex_app_server_protocol::ThreadStatus;
use codex_features::Feature;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::VERCEL_ANTHROPIC_FAST_PROVIDER_ID;
use codex_model_provider_info::VERCEL_GLM_5_2_FAST_MODEL;
use codex_model_provider_info::ZAI_DEFAULT_MODEL;
use codex_model_provider_info::ZAI_PROVIDER_ID;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::SessionSource as CoreSessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::ThreadSource as CoreThreadSource;
use codex_state::DirectionalThreadSpawnEdgeStatus;
use color_eyre::eyre::Result;
use color_eyre::eyre::eyre;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

const NAZGUL_ROLE_NAME: &str = "nazgul";
const TROLL_ROLE: &str = "troll";
const ORC_ROLE: &str = "orc";
const SEND_TASK_FENCE_OPEN: &str = "```pfterminal-send-task";
const SEND_TASK_FENCE_CLOSE: &str = "```";
const SEND_TASK_OPEN: &str = "<pfterminal_send_task";
const SEND_TASK_CLOSE: &str = "</pfterminal_send_task>";
const SPAWN_PARENT_REPORT_LIMIT: usize = 12;
const SPAWN_PROCESSED_DISPATCH_TURN_LIMIT: usize = 1024;
const SPAWN_PROCESSED_DISPATCH_TURN_RETAIN: usize = SPAWN_PROCESSED_DISPATCH_TURN_LIMIT / 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnTaskDispatch {
    pub(crate) target: String,
    pub(crate) task: String,
}

enum SpawnTaskTarget {
    Native(ThreadId),
    ClaudePane(String),
    UnavailableNative(ThreadId),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SavedSpawnThreadMetadata {
    nickname: Option<String>,
    role: Option<String>,
}

struct SpawnThreadStateMetadata<'a> {
    thread_id: ThreadId,
    parent_thread_id: Option<ThreadId>,
    agent_role: &'a str,
    agent_nickname: Option<String>,
    model: String,
    model_provider: String,
    rollout_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SavedSpawnChildIdentity {
    parent_node_id: String,
    role: String,
    identity: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpawnRole {
    Nazgul,
    Troll,
    Orc,
}

impl SpawnRole {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Nazgul => "Nazgul",
            Self::Troll => "Troll",
            Self::Orc => "Orc",
        }
    }

    pub(crate) fn agent_type(self) -> Option<&'static str> {
        match self {
            Self::Nazgul => Some(NAZGUL_ROLE_NAME),
            Self::Troll => Some(TROLL_ROLE),
            Self::Orc => Some(ORC_ROLE),
        }
    }

    pub(crate) fn claude_pane_context(self) -> Option<&'static str> {
        match self {
            Self::Nazgul => None,
            Self::Troll => Some(
                "<pfterminal_spawn_role>\nBehavior:\nYou are the PFTerminal Troll: an engineering manager / VP-of-engineering style supervisor. You report to the Nazgul, the effective CTO. Orcs are IC executors who report to you. You are not an IC.\n\nMandate:\nPrefer delegation, review, coordination, and enforcement over implementation yourself. Work against spec docs, and after work is done make sure the docs reflect what shipped. You may do code reviews yourself or have one Orc review another Orc's work. If a review finds a bug, send the fix back to the responsible Orc. Do not claim completion without concrete evidence.\n\nPersonality:\nHold a very high bar for correctness, business objective fit, tests, evidence, and documentation. Be blunt, adversarial, and demanding about weak work; pick apart Orc output, reject shortcuts, and force rework when the evidence is not good enough. Critique the work product directly.\n\nFinal Report Standards:\nYour final report to the Nazgul must include: Orcs used, what each did, evidence, issues forced back for rework, remaining risk.\n</pfterminal_spawn_role>",
            ),
            Self::Orc => Some(
                "<pfterminal_spawn_role>\nYou are the PFTerminal Orc: an IC executor at the bottom of the chain of command. You report to your supervising Troll engineering manager, who reports to the Nazgul CTO, who reports to Sauron/the human CEO. Do exactly what the Troll tells you. Do not expand scope, reinterpret the assignment, or wander into unrelated work. Execute directly, produce concrete evidence, and report changed files, tests, benchmark output, or findings. Do not spawn child agents. Do not declare done without evidence. If your work is rejected, fix it precisely.\n</pfterminal_spawn_role>",
            ),
        }
    }
}

fn spawn_role_from_agent_type(agent_type: &str) -> Option<SpawnRole> {
    match agent_type {
        NAZGUL_ROLE_NAME => Some(SpawnRole::Nazgul),
        TROLL_ROLE => Some(SpawnRole::Troll),
        ORC_ROLE => Some(SpawnRole::Orc),
        _ => None,
    }
}

impl App {
    pub(crate) fn open_spawn_role_picker(&mut self) {
        let items = vec![
            section_item("Quick start"),
            SelectionItem {
                name: "Create standard crew: Nazgul + Troll + 2 Orcs".to_string(),
                description: Some(
                    "Create persistent named panes (Nazgul GLM 5.2 Z.AI xhigh, Troll Z.AI Vercel Fast, 2x Codex gpt-5.5 xhigh). No task is started."
                        .to_string(),
                ),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::CreateSpawnStandardCrew);
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            section_item("Roles"),
            self.spawn_role_item(SpawnRole::Nazgul),
            self.spawn_role_item(SpawnRole::Troll),
            self.spawn_role_item(SpawnRole::Orc),
            section_item("Status"),
            SelectionItem {
                name: "Spawn status".to_string(),
                description: Some("Show Nazgul -> Troll -> Orc hierarchy.".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenSpawnStatus);
                })],
                dismiss_on_select: true,
                search_value: Some("spawn status status hierarchy".to_string()),
                ..Default::default()
            },
        ];

        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some("Spawn".to_string()),
            subtitle: Some("Create supervised native agents or bind a root pane.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search spawn roles".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn open_spawn_nazgul_pane_picker(&mut self) {
        let mut items = Vec::new();
        items.push(section_item("Existing User Panes"));
        items.push(self.nazgul_pane_item(
            CODEX_MAIN_PANE_ID.to_string(),
            "Codex - Main".to_string(),
            "Current PFTerminal/Codex session".to_string(),
        ));
        for pane in self.claude_panes.panes() {
            items.push(self.nazgul_pane_item(
                pane.id.clone(),
                pane.title.clone(),
                "Claude Code headless pane".to_string(),
            ));
        }

        // Native Codex agent panes (threads) are also eligible Nazgul roots. A Codex pane is a
        // user-controllable thread that is not itself a spawn supervisor/executor role, i.e. the
        // primary Codex Main thread plus any additional Codex threads without a Troll/Orc role.
        let codex_panes = self.nazgul_codex_pane_picker_items();
        if !codex_panes.is_empty() {
            items.push(section_item("Codex Agent Panes"));
            items.extend(codex_panes);
        }

        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some("Bind Nazgul Pane".to_string()),
            subtitle: Some("Select an existing user pane to act as the visible root.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search user panes".to_string()),
            ..Default::default()
        });
    }

    /// Build Nazgul-binding picker rows for native Codex agent panes (threads).
    ///
    /// Only threads that can plausibly act as the root orchestration pane are listed: the primary
    /// Codex Main thread (when known) and any additional Codex threads that are not Troll/Orc
    /// workers. Troll and Orc threads are spawn children and cannot be the root, so they are
    /// excluded. `codex-main` is already offered as a user pane above and is not duplicated here.
    pub(crate) fn nazgul_codex_pane_picker_items(&self) -> Vec<SelectionItem> {
        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (thread_id, entry) in self.agent_navigation.ordered_threads() {
            // Skip Troll/Orc workers; they are spawn children, not roots.
            if entry
                .agent_role
                .as_deref()
                .is_some_and(|role| role == TROLL_ROLE || role == ORC_ROLE)
            {
                continue;
            }
            let node_id = thread_node_id(thread_id);
            if !seen.insert(node_id.clone()) {
                continue;
            }
            let is_primary = self.primary_thread_id == Some(thread_id);
            let name = format_agent_picker_item_name(
                entry.agent_nickname.as_deref(),
                entry.agent_role.as_deref(),
                is_primary,
            );
            let description = if is_primary {
                "Primary Codex Main thread".to_string()
            } else {
                "Codex agent pane".to_string()
            };
            items.push(self.nazgul_pane_item(node_id, name, description));
        }
        items
    }

    pub(crate) fn set_spawn_nazgul_pane_binding(&mut self, pane_id: String) {
        self.spawn_nazgul_pane_id = Some(pane_id);
        self.persist_pane_state();
    }

    pub(crate) fn bind_spawn_nazgul_pane(&mut self, pane_id: String) {
        let title = self.nazgul_bound_display_title(&pane_id);
        self.set_spawn_nazgul_pane_binding(pane_id);
        self.chat_widget.add_info_message(
            format!("Bound {title} as Nazgul root."),
            Some("No worker was spawned.".to_string()),
        );
    }

    pub(crate) fn spawn_context_for_user_pane(&self, pane_id: &str) -> Option<String> {
        let bound_pane_id = self
            .spawn_nazgul_pane_id
            .as_deref()
            .unwrap_or(CODEX_MAIN_PANE_ID);
        if bound_pane_id != pane_id {
            let pane = self
                .claude_panes
                .panes()
                .iter()
                .find(|pane| pane.id == pane_id)?;
            return match pane.spawn_role {
                Some(SpawnRole::Troll) => Some(self.render_troll_spawn_context(pane)),
                Some(SpawnRole::Orc) => Some(self.render_orc_spawn_context(pane)),
                _ => None,
            };
        }
        Some(self.render_nazgul_spawn_context(bound_pane_id))
    }

    /// Render the live spawn-orchestration context for a native Codex thread (a spawned
    /// Nazgul/Troll/Orc pane). Unlike the role config applied at spawn time, this is computed fresh
    /// on every turn so the pane sees the CURRENT Troll/Orc hierarchy — e.g. a Nazgul learns that a
    /// Troll and two Orcs now exist even though none were present when it was spawned.
    pub(crate) fn spawn_context_for_thread(&self, thread_id: ThreadId) -> Option<String> {
        let role = self
            .agent_navigation
            .get(&thread_id)
            .and_then(|entry| entry.agent_role.as_deref())?;
        let label = format_agent_picker_item_name(
            self.agent_navigation
                .get(&thread_id)
                .and_then(|entry| entry.agent_nickname.as_deref()),
            Some(role),
            self.primary_thread_id == Some(thread_id),
        );
        match role {
            NAZGUL_ROLE_NAME => {
                Some(self.render_nazgul_spawn_context_with_title(
                    label, /*include_role_prompt*/ false,
                ))
            }
            TROLL_ROLE => Some(self.render_troll_spawn_context_for_thread(thread_id, label)),
            ORC_ROLE => Some(self.render_orc_spawn_context_for_thread(thread_id, label)),
            _ => None,
        }
    }

    pub(crate) fn native_agent_thread_has_loaded_session(&self, thread_id: ThreadId) -> bool {
        self.thread_has_loaded_session(thread_id)
    }

    pub(crate) fn unloaded_agent_thread_reason(&self, thread_id: ThreadId) -> Option<String> {
        let entry = self.agent_navigation.get(&thread_id)?;
        if self.native_agent_thread_has_loaded_session(thread_id) || !entry.is_closed {
            return None;
        }
        Some(
            "Saved in pane layout, but no replay transcript or live session is loaded for this thread."
                .to_string(),
        )
    }

    fn native_spawn_task_disabled_reason(
        &self,
        thread_id: ThreadId,
        entry: &crate::multi_agents::AgentPickerThreadEntry,
    ) -> Option<String> {
        if self.native_agent_thread_has_loaded_session(thread_id) || !entry.is_closed {
            return None;
        }
        if self.saved_native_spawn_thread_is_task_routable(thread_id) {
            return None;
        }
        Some(
            "Saved in pane layout, but this pane has no loaded session to receive tasks."
                .to_string(),
        )
    }

    fn saved_native_spawn_thread_is_task_routable(&self, thread_id: ThreadId) -> bool {
        let thread_node_id = thread_node_id(thread_id);
        self.spawn_nazgul_pane_id.as_deref() == Some(thread_node_id.as_str())
            || self.spawn_parent_by_node.contains_key(&thread_node_id)
            || self
                .spawn_parent_by_node
                .values()
                .any(|parent_node_id| parent_node_id == &thread_node_id)
            || self.spawn_parent_by_thread.contains_key(&thread_id)
            || self
                .spawn_parent_by_thread
                .values()
                .any(|parent_thread_id| *parent_thread_id == thread_id)
    }

    fn replacement_for_superseded_saved_native_spawn_thread(
        &self,
        thread_id: ThreadId,
        entry: &crate::multi_agents::AgentPickerThreadEntry,
    ) -> Option<ThreadId> {
        self.unloaded_agent_thread_reason(thread_id)?;
        let nickname = entry
            .agent_nickname
            .as_deref()
            .map(str::trim)
            .filter(|nickname| !nickname.is_empty())?;
        let role = entry.agent_role.as_deref()?;

        self.agent_navigation
            .ordered_threads()
            .into_iter()
            .find_map(|(candidate_thread_id, candidate_entry)| {
                if candidate_thread_id == thread_id {
                    return None;
                }
                if self
                    .unloaded_agent_thread_reason(candidate_thread_id)
                    .is_some()
                {
                    return None;
                }
                let same_role = candidate_entry.agent_role.as_deref() == Some(role);
                let same_nickname = candidate_entry
                    .agent_nickname
                    .as_deref()
                    .is_some_and(|candidate| candidate.trim() == nickname);
                (same_role && same_nickname).then_some(candidate_thread_id)
            })
    }

    fn is_superseded_saved_native_spawn_thread(
        &self,
        thread_id: ThreadId,
        entry: &crate::multi_agents::AgentPickerThreadEntry,
    ) -> bool {
        self.replacement_for_superseded_saved_native_spawn_thread(thread_id, entry)
            .is_some()
    }

    pub(crate) fn apply_native_spawn_task_session_fallbacks(
        &self,
        thread_id: ThreadId,
        session: &mut ThreadSessionState,
    ) {
        if session.model.trim().is_empty() {
            session.model = self.native_spawn_fallback_model_for_thread(thread_id);
        }
        if session.model_provider_id.trim().is_empty() {
            session.model_provider_id = self.config.model_provider_id.clone();
        }
        if session.runtime_workspace_roots.is_empty() {
            session.runtime_workspace_roots = self.config.workspace_roots.clone();
        }
    }

    fn native_spawn_fallback_model_for_thread(&self, thread_id: ThreadId) -> String {
        match self
            .agent_navigation
            .get(&thread_id)
            .and_then(|entry| entry.agent_role.as_deref())
        {
            Some(NAZGUL_ROLE_NAME) => Self::STANDARD_NAZGUL_MODEL.to_string(),
            Some(TROLL_ROLE) => Self::STANDARD_TROLL_MODEL.to_string(),
            Some(ORC_ROLE) => Self::STANDARD_ORC_MODEL.to_string(),
            _ => self.chat_widget.current_model().to_string(),
        }
    }

    pub(crate) async fn materialize_saved_native_spawn_thread_for_task(
        &mut self,
        app_server: &mut AppServerSession,
        requested_thread_id: ThreadId,
    ) -> Result<ThreadId> {
        if self.thread_has_loaded_session(requested_thread_id) {
            return Ok(requested_thread_id);
        }

        let mut chain = Vec::new();
        let mut current_thread_id = requested_thread_id;
        let mut seen = HashSet::new();
        loop {
            if self.thread_has_loaded_session(current_thread_id) {
                break;
            }
            if !seen.insert(current_thread_id) {
                return Err(eyre!(
                    "Cannot materialize saved spawn pane {requested_thread_id}: cycle in saved hierarchy."
                ));
            }
            if !self.saved_native_spawn_thread_is_task_routable(current_thread_id) {
                return Err(eyre!(
                    "Cannot materialize saved spawn pane {current_thread_id}: no saved hierarchy edge."
                ));
            }
            chain.push(current_thread_id);

            let current_node_id = thread_node_id(current_thread_id);
            let Some(parent_node_id) = self.spawn_parent_by_node.get(&current_node_id).cloned()
            else {
                break;
            };
            let Some(parent_thread_id) = node_id_thread(&parent_node_id) else {
                break;
            };
            current_thread_id = parent_thread_id;
        }

        let mut materialized_thread_id = requested_thread_id;
        for old_thread_id in chain.into_iter().rev() {
            if self.thread_has_loaded_session(old_thread_id) {
                continue;
            }
            let (role, nickname) =
                self.saved_native_spawn_materialization_metadata(old_thread_id)?;
            let old_node_id = thread_node_id(old_thread_id);
            let parent_node_id = self
                .spawn_parent_by_node
                .get(&old_node_id)
                .cloned()
                .unwrap_or_else(|| self.spawn_root_node_id());
            let Some(parent_thread_id) =
                self.backend_parent_thread_for_spawn(role, Some(parent_node_id.as_str()))
            else {
                return Err(eyre!(
                    "Cannot materialize {}: saved parent {parent_node_id} is not backed by a runnable native thread.",
                    self.thread_label(old_thread_id)
                ));
            };
            let Some(agent_type) = role.agent_type() else {
                return Err(eyre!(
                    "Cannot materialize {}: unsupported saved role.",
                    self.thread_label(old_thread_id)
                ));
            };
            let (model, provider, effort) = Self::standard_native_spawn_runtime_for_role(role);
            self.ensure_native_spawn_provider_ready(Some(provider))?;
            let spawn_config = self.native_spawn_agent_config()?;
            let started = app_server
                .spawn_agent_thread(
                    &spawn_config,
                    parent_thread_id,
                    agent_type.to_string(),
                    nickname.clone(),
                    model.to_string(),
                    Some(provider.to_string()),
                    effort,
                    /*base_instructions*/ None,
                )
                .await?;
            let new_thread_id = started.session.thread_id;
            self.register_spawn_agent_pane(
                new_thread_id,
                parent_thread_id,
                parent_node_id.clone(),
                nickname.clone(),
                agent_type,
                started,
            )
            .await;
            self.replace_saved_native_spawn_thread(old_thread_id, new_thread_id);
            if old_thread_id == requested_thread_id {
                materialized_thread_id = new_thread_id;
            }
        }

        Ok(materialized_thread_id)
    }

    fn saved_native_spawn_materialization_metadata(
        &self,
        thread_id: ThreadId,
    ) -> Result<(SpawnRole, Option<String>)> {
        let entry = self.agent_navigation.get(&thread_id);
        let role_name = entry
            .and_then(|entry| entry.agent_role.as_deref())
            .or_else(|| {
                saved_spawn_role_for_thread(
                    thread_id,
                    self.spawn_nazgul_pane_id.as_deref(),
                    &self.spawn_parent_by_node,
                )
            })
            .ok_or_else(|| eyre!("Saved spawn pane {thread_id} is missing role metadata."))?;
        let role = spawn_role_from_agent_type(role_name).ok_or_else(|| {
            eyre!("Saved spawn pane {thread_id} has unsupported role {role_name}.")
        })?;
        let nickname = entry.and_then(|entry| entry.agent_nickname.clone());
        Ok((role, nickname))
    }

    fn standard_native_spawn_runtime_for_role(
        role: SpawnRole,
    ) -> (&'static str, &'static str, Option<ReasoningEffort>) {
        match role {
            SpawnRole::Nazgul => (
                Self::STANDARD_NAZGUL_MODEL,
                ZAI_PROVIDER_ID,
                Some(ReasoningEffort::XHigh),
            ),
            SpawnRole::Troll => (
                Self::STANDARD_TROLL_MODEL,
                VERCEL_ANTHROPIC_FAST_PROVIDER_ID,
                None,
            ),
            SpawnRole::Orc => (
                Self::STANDARD_ORC_MODEL,
                OPENAI_PROVIDER_ID,
                Some(ReasoningEffort::XHigh),
            ),
        }
    }

    fn replace_saved_native_spawn_thread(
        &mut self,
        old_thread_id: ThreadId,
        new_thread_id: ThreadId,
    ) {
        let old_node_id = thread_node_id(old_thread_id);
        let new_node_id = thread_node_id(new_thread_id);

        if self.spawn_nazgul_pane_id.as_deref() == Some(old_node_id.as_str()) {
            self.spawn_nazgul_pane_id = Some(new_node_id.clone());
        }
        self.spawn_parent_by_node.remove(&old_node_id);
        for parent_node_id in self.spawn_parent_by_node.values_mut() {
            if parent_node_id == &old_node_id {
                *parent_node_id = new_node_id.clone();
            }
        }
        self.spawn_parent_by_thread.remove(&old_thread_id);
        for parent_thread_id in self.spawn_parent_by_thread.values_mut() {
            if *parent_thread_id == old_thread_id {
                *parent_thread_id = new_thread_id;
            }
        }
        self.agent_navigation.remove(old_thread_id);
        self.persist_pane_state();
    }

    fn prune_superseded_saved_native_spawn_threads(&mut self) {
        let replacements = self
            .agent_navigation
            .ordered_threads()
            .into_iter()
            .filter_map(|(thread_id, entry)| {
                self.replacement_for_superseded_saved_native_spawn_thread(thread_id, entry)
                    .map(|replacement_thread_id| (thread_id, replacement_thread_id))
            })
            .collect::<Vec<_>>();
        if replacements.is_empty() {
            return;
        }

        for (old_thread_id, new_thread_id) in replacements {
            let old_node_id = thread_node_id(old_thread_id);
            let new_node_id = thread_node_id(new_thread_id);
            if self.spawn_nazgul_pane_id.as_deref() == Some(old_node_id.as_str()) {
                self.spawn_nazgul_pane_id = Some(new_node_id.clone());
            }
            self.spawn_parent_by_node.remove(&old_node_id);
            for parent_node_id in self.spawn_parent_by_node.values_mut() {
                if parent_node_id == &old_node_id {
                    *parent_node_id = new_node_id.clone();
                }
            }
            self.spawn_parent_by_thread.remove(&old_thread_id);
            for parent_thread_id in self.spawn_parent_by_thread.values_mut() {
                if *parent_thread_id == old_thread_id {
                    *parent_thread_id = new_thread_id;
                }
            }
            self.agent_navigation.remove(old_thread_id);
        }

        self.persist_pane_state();
    }

    pub(crate) fn prune_duplicate_live_native_spawn_threads(&mut self) {
        let mut retained_by_identity: HashMap<(String, String, String), ThreadId> = HashMap::new();
        let mut replacements = Vec::new();
        for (thread_id, entry) in self.agent_navigation.ordered_threads() {
            if self.unloaded_agent_thread_reason(thread_id).is_some() {
                continue;
            }
            let node_id = thread_node_id(thread_id);
            let Some(parent_node_id) = self.spawn_parent_by_node.get(&node_id).cloned() else {
                continue;
            };
            let Some(role) = entry
                .agent_role
                .as_deref()
                .map(str::trim)
                .filter(|role| !role.is_empty())
                .map(ToString::to_string)
            else {
                continue;
            };
            let Some(identity) = entry
                .agent_nickname
                .as_deref()
                .or(entry.agent_path.as_deref())
                .map(str::trim)
                .filter(|identity| !identity.is_empty())
                .map(ToString::to_string)
            else {
                continue;
            };
            let key = (parent_node_id, role, identity);
            if let Some(previous_thread_id) = retained_by_identity.insert(key, thread_id) {
                replacements.push((previous_thread_id, thread_id));
            }
        }
        if replacements.is_empty() {
            return;
        }
        for (old_thread_id, new_thread_id) in replacements {
            self.replace_saved_native_spawn_thread(old_thread_id, new_thread_id);
        }
        self.persist_pane_state();
    }

    pub(crate) fn spawn_additional_context_for_thread(
        &self,
        thread_id: ThreadId,
    ) -> Option<HashMap<String, AdditionalContextEntry>> {
        if !self.is_spawn_orchestration_thread(thread_id) {
            return None;
        }
        self.spawn_context_for_thread(thread_id).map(|context| {
            let mut map = HashMap::new();
            map.insert(
                "pfterminal_spawn_context".to_string(),
                AdditionalContextEntry {
                    value: context,
                    kind: AdditionalContextKind::Application,
                },
            );
            map
        })
    }

    fn render_troll_spawn_context_for_thread(&self, thread_id: ThreadId, label: String) -> String {
        let troll_node_id = thread_node_id(thread_id);
        let mut context = String::new();
        let _ = writeln!(context, "<pfterminal_spawn_context>");
        let _ = writeln!(context, "You are the PFTerminal Troll pane: {label}.");
        let _ = writeln!(context, "Behavior:");
        let _ = writeln!(
            context,
            "You are an engineering manager / VP-of-engineering style supervisor. You report to the Nazgul, the effective CTO. Orcs are IC executors who report to you."
        );
        let _ = writeln!(context, "Mandate:");
        let _ = writeln!(
            context,
            "Prefer delegation, review, coordination, and enforcement over implementation. Work against spec docs, ensure shipped work is documented, and send bugs found in review back to the responsible Orc."
        );
        let _ = writeln!(context, "Personality:");
        let _ = writeln!(
            context,
            "Be blunt, adversarial, and demanding about weak work; reject shortcuts and force rework when evidence is not good enough."
        );
        let _ = writeln!(context, "Final Report Standards:");
        let _ = writeln!(
            context,
            "Report Orcs used, what each did, evidence, issues forced back for rework, and remaining risk."
        );
        write_spawn_product_contract(&mut context);
        write_spawn_dispatch_contract(&mut context);
        let _ = writeln!(context, "Orcs assigned to you:");
        let (orcs, claude_orcs) = self.spawn_orc_children_for_node(&troll_node_id);
        if orcs.is_empty() && claude_orcs.is_empty() {
            let _ = writeln!(context, "- none assigned yet.");
        } else {
            for (orc_thread_id, orc_entry) in orcs {
                self.write_spawn_context_agent(
                    &mut context,
                    "- ",
                    orc_thread_id,
                    orc_entry,
                    Some(ORC_ROLE),
                );
            }
            for pane in claude_orcs {
                self.write_spawn_context_claude_pane(&mut context, "- ", pane, SpawnRole::Orc);
            }
        }
        self.write_spawn_parent_reports(&mut context, &troll_node_id);
        let _ = writeln!(context, "</pfterminal_spawn_context>");
        context
    }

    fn render_orc_spawn_context_for_thread(&self, thread_id: ThreadId, label: String) -> String {
        let mut context = String::new();
        let _ = writeln!(context, "<pfterminal_spawn_context>");
        let _ = writeln!(context, "You are the PFTerminal Orc pane: {label}.");
        let _ = writeln!(
            context,
            "You are an IC executor. Chain of command: Orc -> Troll engineering manager -> Nazgul CTO -> Sauron/the human CEO."
        );
        let _ = writeln!(
            context,
            "Do exactly what your Troll tells you. Do not expand scope. Execute directly and provide evidence."
        );
        write_spawn_product_contract(&mut context);
        if let Some(parent_node_id) = self.logical_parent_node_for_thread(thread_id)
            && let Some(parent_title) = self.spawn_node_title(&parent_node_id)
        {
            let _ = writeln!(context, "You report to: {parent_title}.");
        } else {
            let _ = writeln!(
                context,
                "You do not currently have an assigned Troll supervisor."
            );
        }
        let _ = writeln!(context, "</pfterminal_spawn_context>");
        context
    }

    pub(crate) fn next_spawn_agent_nickname(&self, role: SpawnRole) -> Option<String> {
        let role_name = role.agent_type()?;
        let candidates = crate::legacy_core::config::agent_nickname_candidates_for_role(
            &self.config,
            Some(role_name),
        );
        let used_nicknames = self
            .agent_navigation
            .ordered_threads()
            .into_iter()
            .filter_map(|(_, entry)| entry.agent_nickname.as_deref())
            .chain(
                self.claude_panes
                    .panes()
                    .iter()
                    .filter_map(|pane| pane.spawn_nickname.as_deref()),
            );
        next_spawn_agent_nickname_from_used(candidates.iter().map(String::as_str), used_nicknames)
    }

    pub(crate) fn open_spawn_parent_picker(&mut self, role: SpawnRole) {
        match role {
            SpawnRole::Nazgul => self.open_spawn_nazgul_pane_picker(),
            SpawnRole::Troll => {
                self.open_spawn_harness_picker(role, Some(self.spawn_root_node_id()));
            }
            SpawnRole::Orc => {
                let trolls = self.spawn_troll_node_items();
                if trolls.is_empty() {
                    self.chat_widget.add_error_message(
                        "Spawn a Troll before creating Orc panes, then choose that Troll as supervisor."
                            .to_string(),
                    );
                    return;
                }
                self.chat_widget.show_selection_view(SelectionViewParams {
                    title: Some("Assign Orc Supervisor".to_string()),
                    subtitle: Some("Choose the Troll that will supervise this Orc.".to_string()),
                    footer_hint: Some(standard_popup_hint_line()),
                    items: trolls,
                    is_searchable: true,
                    search_placeholder: Some("Search Trolls".to_string()),
                    ..Default::default()
                });
            }
        }
    }

    pub(crate) fn open_spawn_harness_picker(
        &mut self,
        role: SpawnRole,
        parent_node_id: Option<String>,
    ) {
        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some(format!("Spawn {}", role.label())),
            subtitle: Some(format!(
                "Choose the harness for this {} pane.",
                role.label()
            )),
            footer_hint: Some(standard_popup_hint_line()),
            items: vec![
                SelectionItem {
                    name: "Harness: Codex".to_string(),
                    description: Some(
                        "Native PFTerminal/Codex agent pane; choose model and reasoning next."
                            .to_string(),
                    ),
                    actions: vec![Box::new({
                        let parent_node_id = parent_node_id.clone();
                        move |tx| {
                            tx.send(AppEvent::OpenSpawnModelPicker {
                                role,
                                parent_node_id: parent_node_id.clone(),
                            });
                        }
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Harness: Claude Code".to_string(),
                    description: Some(
                        "Claude Code headless pane; choose provider route next.".to_string(),
                    ),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::OpenSpawnClaudeProfilePicker {
                            role,
                            parent_node_id: parent_node_id.clone(),
                        });
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
    }

    pub(crate) fn open_spawn_model_picker(
        &mut self,
        role: SpawnRole,
        parent_node_id: Option<String>,
    ) {
        let current_model = self.native_spawn_default_model();
        let presets = self
            .chat_widget
            .model_catalog()
            .try_list_models()
            .unwrap_or_default();
        let current_effort = self.native_spawn_effort_for_model(role, &current_model);
        let mut items = Vec::new();
        items.push(section_item("Codex Native Agent"));
        items.push(spawn_model_item(
            role,
            parent_node_id.clone(),
            current_model.clone(),
            ChatWidget::model_provider_for_selection(&current_model),
            current_effort,
            Some(format!(
                "Create a Codex-native {} pane with the current model and role default reasoning.",
                role.label()
            )),
            true,
        ));

        for preset in presets
            .into_iter()
            .filter(ChatWidget::show_in_pfterminal_model_picker)
            .filter(|preset| preset.model != current_model)
        {
            if items.len() == 2 {
                items.push(section_item("Other Codex Models"));
            }
            let description =
                (!preset.description.is_empty()).then_some(preset.description.clone());
            items.push(spawn_model_item(
                role,
                parent_node_id.clone(),
                preset.model.clone(),
                ChatWidget::model_provider_for_selection(&preset.model),
                Some(spawn_reasoning_effort_for_role(role, &preset)),
                description,
                false,
            ));
        }

        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some(format!("Spawn Codex {}", role.label())),
            subtitle: Some(format!(
                "Choose the model for the Codex-native {} pane.",
                role.label()
            )),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search models".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn open_spawn_claude_profile_picker(
        &mut self,
        role: SpawnRole,
        parent_node_id: Option<String>,
    ) {
        let mut items = Vec::new();
        for profile in ClaudeProviderProfileKind::creation_options() {
            let profile_config = profile.profile();
            let kind = *profile;
            items.push(SelectionItem {
                name: format!("Claude {}: {}", role.label(), profile.status_model_label()),
                description: Some(profile_config.description.to_string()),
                search_value: Some(format!(
                    "claude {} {} {}",
                    role.label(),
                    profile_config.title,
                    profile_config.description
                )),
                actions: vec![Box::new({
                    let parent_node_id = parent_node_id.clone();
                    move |tx| {
                        tx.send(AppEvent::CreateSpawnClaudePane {
                            role,
                            parent_node_id: parent_node_id.clone(),
                            profile: kind,
                        });
                    }
                })],
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some(format!("Spawn Claude {}", role.label())),
            subtitle: Some(format!(
                "Choose the Claude Code provider route for this {} pane.",
                role.label()
            )),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search Claude providers".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn open_spawn_status(&mut self) {
        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some("Spawn Status".to_string()),
            subtitle: Some("Nazgul -> Troll -> Orc hierarchy.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items: self.spawn_tree_items(/*show_task_actions*/ true),
            is_searchable: true,
            search_placeholder: Some("Search spawned work".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn open_spawn_agent_task_prompt(&mut self, thread_id: ThreadId) {
        let title = self.thread_label(thread_id);
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            format!("Send Task to {title}"),
            "Describe the work to run in this pane".to_string(),
            String::new(),
            Some("Task".to_string()),
            Box::new(move |task| {
                tx.send(AppEvent::SubmitSpawnAgentTask { thread_id, task });
            }),
        );
        self.chat_widget.show_custom_prompt_view(view);
    }

    pub(crate) fn spawn_agent_task_for_submission(
        &self,
        thread_id: ThreadId,
        task: &str,
    ) -> String {
        let Some(entry) = self.agent_navigation.get(&thread_id) else {
            return task.to_string();
        };
        if entry.agent_role.as_deref() != Some(TROLL_ROLE) {
            return task.to_string();
        }

        let mut context = String::new();
        let troll_name = format_agent_picker_item_name(
            entry.agent_nickname.as_deref(),
            entry.agent_role.as_deref().or(Some(TROLL_ROLE)),
            false,
        );
        let _ = writeln!(context, "<pfterminal_spawn_troll_task_context>");
        let _ = writeln!(
            context,
            "You are receiving this task through /spawn as {troll_name}."
        );
        let troll_node_id = thread_node_id(thread_id);
        let (orcs, claude_orcs) = self.spawn_orc_children_for_node(&troll_node_id);
        let has_orcs = !orcs.is_empty() || !claude_orcs.is_empty();
        if !has_orcs {
            let _ = writeln!(context, "No existing Orc panes are assigned to you yet.");
            let _ = writeln!(
                context,
                "If this task requires execution, create or request Orc panes before claiming completion."
            );
        } else {
            let _ = writeln!(context, "Existing Orc panes assigned to you:");
        }
        for (orc_thread_id, orc_entry) in orcs {
            self.write_spawn_context_agent(
                &mut context,
                "- ",
                orc_thread_id,
                orc_entry,
                Some(ORC_ROLE),
            );
            if let Some(path) = orc_entry.agent_path.as_deref() {
                let _ = writeln!(context, "  canonical_task_name={path}");
            }
        }
        for pane in claude_orcs {
            self.write_spawn_context_claude_pane(&mut context, "- ", pane, SpawnRole::Orc);
        }
        self.write_spawn_parent_reports(&mut context, &troll_node_id);
        if has_orcs {
            let _ = writeln!(
                context,
                "Use these existing Orc panes first. Do not call spawn_agent for work that can be assigned to the listed Orc panes."
            );
            let _ = writeln!(
                context,
                "Assign work by emitting one pfterminal_send_task host dispatch block per target. Target the exact listed name, thread id, pane id, or canonical_task_name."
            );
            let _ = writeln!(
                context,
                "Observe completion from child report messages and /spawn status before reviewing or claiming completion."
            );
            let _ = writeln!(
                context,
                "Only call spawn_agent if the listed Orc panes are insufficient, and state the reason before doing so."
            );
        }
        let _ = writeln!(context, "</pfterminal_spawn_troll_task_context>");
        let _ = writeln!(context);
        let _ = writeln!(context, "Task from Sauron/Nazgul:");
        let _ = write!(context, "{task}");
        context
    }

    pub(crate) fn open_spawn_claude_pane_task_prompt(&mut self, pane_id: String) {
        let title = self.user_pane_title(&pane_id);
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            format!("Send Task to {title}"),
            "Describe the work to run in this Claude pane".to_string(),
            String::new(),
            Some("Task".to_string()),
            Box::new(move |task| {
                tx.send(AppEvent::SubmitSpawnClaudePaneTask {
                    pane_id: pane_id.clone(),
                    task,
                });
            }),
        );
        self.chat_widget.show_custom_prompt_view(view);
    }

    pub(crate) fn dispatch_spawn_task_blocks(
        &mut self,
        source_pane_id: &str,
        dispatches: Vec<SpawnTaskDispatch>,
    ) {
        let source_thread_id = node_id_thread(source_pane_id);
        let source_is_active = self.claude_panes.active_user_pane_id() == source_pane_id
            || source_thread_id.is_some_and(|thread_id| {
                self.active_thread_id == Some(thread_id)
                    && self.claude_panes.active_user_pane_id() == CODEX_MAIN_PANE_ID
            });
        let source_title = self
            .spawn_node_title(source_pane_id)
            .unwrap_or_else(|| self.user_pane_title(source_pane_id));
        for dispatch in dispatches {
            if dispatch.task.trim().is_empty() {
                self.record_spawn_dispatch_error(
                    source_pane_id,
                    source_is_active,
                    format!(
                        "Ignored empty task dispatch for target `{}`.",
                        dispatch.target
                    ),
                );
                continue;
            }
            match self.resolve_spawn_task_target(&dispatch.target) {
                Ok(SpawnTaskTarget::Native(thread_id)) => {
                    let label = self.thread_label(thread_id);
                    let task = task_with_dispatch_provenance(&dispatch.task, &source_title, &label);
                    self.app_event_tx
                        .send(AppEvent::SubmitSpawnAgentTask { thread_id, task });
                    self.record_spawn_dispatch_queued(
                        source_pane_id,
                        source_is_active,
                        &format!("Queued task for {label}."),
                        &dispatch.task,
                    );
                }
                Ok(SpawnTaskTarget::UnavailableNative(thread_id)) => {
                    self.record_spawn_dispatch_error(
                        source_pane_id,
                        source_is_active,
                        self.unavailable_native_spawn_target_error(thread_id),
                    );
                }
                Ok(SpawnTaskTarget::ClaudePane(pane_id)) => {
                    if pane_id == source_pane_id {
                        self.record_spawn_dispatch_error(
                            source_pane_id,
                            source_is_active,
                            "Claude pane cannot dispatch a task to itself.".to_string(),
                        );
                        continue;
                    }
                    let title = self.user_pane_title(&pane_id);
                    let task = task_with_dispatch_provenance(&dispatch.task, &source_title, &title);
                    self.app_event_tx
                        .send(AppEvent::SubmitSpawnClaudePaneTask { pane_id, task });
                    self.record_spawn_dispatch_queued(
                        source_pane_id,
                        source_is_active,
                        &format!("Queued task for {title}."),
                        &dispatch.task,
                    );
                }
                Err(err) => {
                    self.record_spawn_dispatch_error(source_pane_id, source_is_active, err);
                }
            }
        }
    }

    pub(crate) fn dispatch_native_spawn_task_blocks_from_turn(
        &mut self,
        source_thread_id: ThreadId,
        turn: &codex_app_server_protocol::Turn,
    ) {
        if !self.is_spawn_orchestration_thread(source_thread_id) {
            return;
        }
        if turn.status == codex_app_server_protocol::TurnStatus::InProgress {
            return;
        }
        let dispatch_turn = (source_thread_id, turn.id.clone());
        if !self
            .spawn_processed_dispatch_turns
            .insert(dispatch_turn.clone())
        {
            return;
        }
        self.evict_spawn_processed_dispatch_turns_if_needed(&dispatch_turn);
        let mut assistant_text = String::new();
        for item in &turn.items {
            if let codex_app_server_protocol::ThreadItem::AgentMessage { text, .. } = item {
                if !assistant_text.is_empty() {
                    assistant_text.push('\n');
                }
                assistant_text.push_str(text);
            }
        }
        if assistant_text.trim().is_empty() {
            return;
        }
        let (_visible, dispatches) = extract_spawn_task_dispatches(&assistant_text);
        if dispatches.is_empty() {
            return;
        }
        self.dispatch_spawn_task_blocks(&thread_node_id(source_thread_id), dispatches);
    }

    fn evict_spawn_processed_dispatch_turns_if_needed(
        &mut self,
        protected_turn: &(ThreadId, String),
    ) {
        let live_threads: HashSet<ThreadId> = self
            .agent_navigation
            .ordered_threads()
            .into_iter()
            .map(|(thread_id, _)| thread_id)
            .collect();
        let live_thread_count = live_threads.len();
        if let Some((before_len, after_len)) = evict_spawn_processed_dispatch_turns(
            &mut self.spawn_processed_dispatch_turns,
            &live_threads,
            protected_turn,
        ) {
            tracing::debug!(
                before_len,
                after_len,
                limit = SPAWN_PROCESSED_DISPATCH_TURN_LIMIT,
                live_thread_count,
                protected_thread_id = %protected_turn.0,
                protected_turn_id = protected_turn.1.as_str(),
                "evicted processed native spawn dispatch turns"
            );
        }
    }

    pub(crate) fn record_spawn_child_report_for_thread(
        &mut self,
        thread_id: ThreadId,
        status: codex_app_server_protocol::CollabAgentStatus,
        result: Option<String>,
    ) {
        if !self.is_spawn_orchestration_thread(thread_id) {
            return;
        }
        let Some(parent_node_id) = self.logical_parent_node_for_thread(thread_id) else {
            return;
        };
        let Some(entry) = self.agent_navigation.get(&thread_id) else {
            return;
        };
        let child_title = format_agent_picker_item_name(
            entry.agent_nickname.as_deref(),
            entry.agent_role.as_deref(),
            self.primary_thread_id == Some(thread_id),
        );
        let report = spawn_child_report(
            &child_title,
            collab_status_label(&status),
            result.as_deref(),
        );
        self.record_spawn_parent_report(parent_node_id, report);
    }

    pub(crate) fn record_spawn_child_report_for_claude_pane(
        &mut self,
        pane_id: &str,
        status: &str,
        result: Option<&str>,
    ) {
        if !self
            .claude_panes
            .panes()
            .iter()
            .any(|pane| pane.id == pane_id && pane.spawn_role.is_some())
        {
            return;
        }
        let Some(parent_node_id) = self.logical_parent_node_for_pane(pane_id) else {
            return;
        };
        let child_title = self.user_pane_title(pane_id);
        let report = spawn_child_report(&child_title, status, result);
        self.record_spawn_parent_report(parent_node_id, report);
    }

    fn record_spawn_parent_report(&mut self, parent_node_id: String, report: String) {
        let inserted = {
            let reports = self
                .spawn_parent_reports_by_node
                .entry(parent_node_id.clone())
                .or_default();
            if reports.back() == Some(&report) {
                false
            } else {
                reports.push_back(report.clone());
                while reports.len() > SPAWN_PARENT_REPORT_LIMIT {
                    reports.pop_front();
                }
                true
            }
        };
        if inserted {
            self.notify_spawn_parent_report(&parent_node_id, &report);
        }
    }

    fn notify_spawn_parent_report(&mut self, parent_node_id: &str, report: &str) {
        let summary = "Child report delivered.".to_string();
        let hint = Some(report.to_string());
        // `codex-main` is the native primary Codex thread, not a Claude pane. Route it through the
        // native-thread path below instead of the Claude-pane path, otherwise dispatch errors with
        // "Claude pane `codex-main` does not exist".
        if let Some(parent_pane_id) = node_id_pane(parent_node_id)
            && parent_pane_id != CODEX_MAIN_PANE_ID
        {
            if self.claude_panes.active_user_pane_id() == parent_pane_id {
                self.chat_widget.add_info_message(summary, hint);
            } else {
                self.append_inactive_claude_pane_transcript_cell(
                    parent_pane_id,
                    Arc::new(crate::history_cell::new_info_event(summary, hint)),
                );
            }
            if !self.claude_panes.claude_pane_is_running(parent_pane_id) {
                let trigger_prompt = child_report_processing_prompt(report);
                self.app_event_tx.send(AppEvent::SubmitSpawnClaudePaneTask {
                    pane_id: parent_pane_id.to_string(),
                    task: trigger_prompt,
                });
            }
            return;
        }
        // Native Codex parent thread (a spawned Nazgul/Troll) or `codex-main` (the primary thread).
        let parent_thread_id = if parent_node_id == pane_node_id(CODEX_MAIN_PANE_ID) {
            self.primary_thread_id
        } else {
            node_id_thread(parent_node_id)
        };
        if let Some(parent_thread_id) = parent_thread_id {
            if self.active_thread_id == Some(parent_thread_id)
                && self.claude_panes.active_user_pane_id() == CODEX_MAIN_PANE_ID
            {
                self.chat_widget.add_info_message(summary, hint);
            }
            // Deliver the report as a real parent processing turn when the parent is idle. When the
            // parent is mid-turn, enqueue the report instead of dropping it; it is flushed into a
            // processing turn when the parent next goes idle (see flush_pending_reports_for_thread).
            let is_running = self
                .agent_navigation
                .get(&parent_thread_id)
                .map(|e| e.is_running)
                .unwrap_or(false);
            if is_running {
                self.enqueue_pending_report(parent_thread_id, report);
            } else {
                let trigger_prompt = child_report_processing_prompt(report);
                self.app_event_tx.send(AppEvent::SubmitSpawnAgentTask {
                    thread_id: parent_thread_id,
                    task: trigger_prompt,
                });
            }
        }
    }

    /// Queue a child report for a native Codex parent thread that is currently mid-turn. The queued
    /// report is turned into a parent processing turn when the parent goes idle.
    fn enqueue_pending_report(&mut self, parent_thread_id: ThreadId, report: &str) {
        let queue = self
            .spawn_pending_reports_by_thread
            .entry(parent_thread_id)
            .or_default();
        // Deduplicate against the most recent queued report so a repeated identical result does not
        // spawn redundant turns.
        if queue.back() == Some(&report.to_string()) {
            return;
        }
        queue.push_back(report.to_string());
        tracing::info!(
            thread_id = %parent_thread_id,
            queue_len = queue.len(),
            "queued child report for busy parent; will flush on parent idle"
        );
    }

    /// Drain pending child reports for a native Codex parent thread that has just gone idle, turning
    /// each into a parent processing turn. Returns true if any report was flushed.
    ///
    /// This is the fix for the multi-turn race: a report that arrived while the parent was mid-turn
    /// is no longer dropped; it is processed as soon as the parent becomes idle, like a user query.
    pub(crate) fn flush_pending_reports_for_thread(&mut self, parent_thread_id: ThreadId) -> bool {
        let Some(queue) = self
            .spawn_pending_reports_by_thread
            .get_mut(&parent_thread_id)
        else {
            return false;
        };
        if queue.is_empty() {
            return false;
        }
        // Drain the queue into a single combined processing turn so the parent reviews every pending
        // report at once rather than starting one turn per report. This keeps the parent's attention
        // on the full set of outstanding child results and avoids interleaving many short turns.
        let mut reports: Vec<String> = queue.drain(..).collect();
        let body = if reports.len() == 1 {
            reports.remove(0)
        } else {
            let mut combined = String::from(
                "Multiple child panes have reported back while you were busy. Review every report \
                 below, triage each, dispatch follow-up work or acknowledge, and do not skip any \
                 of them.\n\n",
            );
            for (index, report) in reports.into_iter().enumerate() {
                let _ = writeln!(combined, "## Child report {index}\n{report}\n");
            }
            combined
        };
        let trigger_prompt = child_report_processing_prompt(&body);
        self.app_event_tx.send(AppEvent::SubmitSpawnAgentTask {
            thread_id: parent_thread_id,
            task: trigger_prompt,
        });
        true
    }

    fn record_spawn_dispatch_queued(
        &mut self,
        source_pane_id: &str,
        source_is_active: bool,
        summary: &str,
        task: &str,
    ) {
        let hint = Some(format!(
            "Dispatched from Claude pane output. Task: {}",
            compact_spawn_context_value(task)
        ));
        if source_is_active {
            self.chat_widget.add_info_message(summary.to_string(), hint);
        } else {
            self.append_inactive_claude_pane_transcript_cell(
                source_pane_id,
                Arc::new(crate::history_cell::new_info_event(
                    summary.to_string(),
                    hint,
                )),
            );
        }
    }

    fn record_spawn_dispatch_error(
        &mut self,
        source_pane_id: &str,
        source_is_active: bool,
        message: String,
    ) {
        if source_is_active {
            self.chat_widget.add_error_message(message);
        } else {
            self.append_inactive_claude_pane_transcript_cell(
                source_pane_id,
                Arc::new(crate::history_cell::new_error_event(message)),
            );
        }
    }

    pub(crate) fn spawn_parent_thread_for_new_agent(&self, role: SpawnRole) -> Option<ThreadId> {
        let active_thread_role = self
            .active_thread_id
            .and_then(|thread_id| self.agent_navigation.get(&thread_id))
            .and_then(|entry| entry.agent_role.as_deref());
        let troll_thread_ids = self
            .spawn_troll_threads()
            .into_iter()
            .map(|(thread_id, _)| thread_id)
            .collect::<Vec<_>>();
        spawn_parent_thread_for_new_agent(
            role,
            self.claude_panes.active_claude_pane_id().is_some(),
            self.primary_thread_id,
            self.active_thread_id,
            active_thread_role,
            &troll_thread_ids,
        )
    }

    pub(crate) fn backend_parent_thread_for_spawn(
        &self,
        role: SpawnRole,
        parent_node_id: Option<&str>,
    ) -> Option<ThreadId> {
        if role == SpawnRole::Orc
            && let Some(parent_node_id) = parent_node_id
        {
            if let Some(parent_thread_id) = node_id_thread(parent_node_id) {
                return Some(parent_thread_id);
            }
            if let Some(parent_pane_id) = node_id_pane(parent_node_id)
                && self.claude_panes.panes().iter().any(|pane| {
                    pane.id == parent_pane_id && pane.spawn_role == Some(SpawnRole::Troll)
                })
            {
                return self.primary_thread_id;
            }
        }
        self.spawn_parent_thread_for_new_agent(role)
    }

    pub(crate) fn logical_parent_node_for_spawn(
        &self,
        role: SpawnRole,
        parent_node_id: Option<&str>,
    ) -> String {
        if let Some(parent_node_id) = parent_node_id {
            return parent_node_id.to_string();
        }
        match role {
            SpawnRole::Nazgul => self.spawn_root_node_id(),
            SpawnRole::Troll => self.spawn_root_node_id(),
            SpawnRole::Orc => self
                .single_troll_node_id()
                .unwrap_or_else(|| self.spawn_root_node_id()),
        }
    }

    fn single_troll_node_id(&self) -> Option<String> {
        let mut troll_nodes = self
            .spawn_troll_threads()
            .into_iter()
            .map(|(thread_id, _)| thread_node_id(thread_id))
            .chain(
                self.claude_spawn_panes(SpawnRole::Troll)
                    .into_iter()
                    .map(|pane| pane_node_id(&pane.id)),
            )
            .collect::<Vec<_>>();
        if troll_nodes.len() == 1 {
            troll_nodes.pop()
        } else {
            None
        }
    }

    pub(crate) fn native_spawn_default_model(&self) -> String {
        if let Some(pane_id) = self.claude_panes.active_claude_pane_id()
            && let Some(pane) = self
                .claude_panes
                .panes()
                .iter()
                .find(|pane| pane.id == pane_id)
            && let Some(model) = pane.profile.native_codex_model()
        {
            return model.to_string();
        }
        self.chat_widget.current_model().to_string()
    }

    pub(crate) fn native_spawn_effort_for_model(
        &self,
        role: SpawnRole,
        model: &str,
    ) -> Option<ReasoningEffort> {
        self.chat_widget
            .model_catalog()
            .try_list_models()
            .ok()
            .and_then(|presets| {
                presets
                    .into_iter()
                    .find(|preset| preset.model == model)
                    .map(|preset| spawn_reasoning_effort_for_role(role, &preset))
            })
            .or_else(|| self.chat_widget.current_reasoning_effort())
    }

    /// Standard crew model/effort mapping (all Codex-native):
    ///   Nazgul: glm-5.2 (Z.AI) @ xhigh
    ///   Troll:  zai/glm-5.2-fast (Vercel) @ default
    ///   Orc 1/2: gpt-5.5 (OpenAI) @ xhigh
    pub(crate) const STANDARD_NAZGUL_MODEL: &'static str = ZAI_DEFAULT_MODEL;
    pub(crate) const STANDARD_TROLL_MODEL: &'static str = VERCEL_GLM_5_2_FAST_MODEL;
    pub(crate) const STANDARD_ORC_MODEL: &'static str = "gpt-5.5";

    pub(crate) fn ensure_standard_crew_providers_ready(&self) -> Result<()> {
        // Preflight every provider before creating the root Nazgul. Without this, a missing Troll
        // or Orc credential leaves a half-created crew with only the already-bound Nazgul pane.
        for provider_id in [
            ZAI_PROVIDER_ID,
            VERCEL_ANTHROPIC_FAST_PROVIDER_ID,
            OPENAI_PROVIDER_ID,
        ] {
            self.ensure_native_spawn_provider_ready(Some(provider_id))?;
        }
        Ok(())
    }

    pub(crate) async fn create_spawn_standard_crew(
        &mut self,
        app_server: &mut AppServerSession,
    ) -> Result<(ThreadId, ThreadId)> {
        let root_thread_id = self
            .primary_thread_id
            .or(self.active_thread_id)
            .ok_or_else(|| eyre!("Cannot create standard crew before Codex Main has started."))?;
        let spawn_config = self.native_spawn_agent_config()?;
        self.ensure_standard_crew_providers_ready()?;

        // Nazgul — glm-5.2 (Z.AI) @ xhigh, root, loaded through the built-in Nazgul role config.
        let nazgul_nickname = self.next_spawn_agent_nickname(SpawnRole::Nazgul);
        let nazgul = app_server
            .spawn_agent_thread(
                &spawn_config,
                root_thread_id,
                NAZGUL_ROLE_NAME.to_string(),
                nazgul_nickname.clone(),
                Self::STANDARD_NAZGUL_MODEL.to_string(),
                Some(ZAI_PROVIDER_ID.to_string()),
                Some(ReasoningEffort::XHigh),
                /*base_instructions*/ None,
            )
            .await?;
        let nazgul_thread_id = nazgul.session.thread_id;
        self.register_spawn_agent_pane(
            nazgul_thread_id,
            root_thread_id,
            // The Nazgul is the root; its logical parent is the current root node (codex-main or a
            // prior binding). It will be auto-bound below.
            self.spawn_root_node_id(),
            nazgul_nickname,
            NAZGUL_ROLE_NAME,
            nazgul,
        )
        .await;
        // Bind the freshly spawned Nazgul as the visible root so Troll spawns and "Nazgul"
        // dispatches route to it.
        self.set_spawn_nazgul_pane_binding(thread_node_id(nazgul_thread_id));
        self.persist_bound_nazgul_root_thread_metadata().await;

        // Troll — zai/glm-5.2-fast (Vercel) under the Nazgul.
        let troll_nickname = self.next_spawn_agent_nickname(SpawnRole::Troll);
        let troll = app_server
            .spawn_agent_thread(
                &spawn_config,
                nazgul_thread_id,
                TROLL_ROLE.to_string(),
                troll_nickname.clone(),
                Self::STANDARD_TROLL_MODEL.to_string(),
                Some(VERCEL_ANTHROPIC_FAST_PROVIDER_ID.to_string()),
                // Vercel-fast GLM does not take a reasoning effort override.
                /*effort*/
                None,
                /*base_instructions*/ None,
            )
            .await?;
        let troll_thread_id = troll.session.thread_id;
        self.register_spawn_agent_pane(
            troll_thread_id,
            nazgul_thread_id,
            thread_node_id(nazgul_thread_id),
            troll_nickname,
            TROLL_ROLE,
            troll,
        )
        .await;

        // Two Orcs — gpt-5.5 (OpenAI) @ xhigh under the Troll.
        for _ in 0..2 {
            let orc_nickname = self.next_spawn_agent_nickname(SpawnRole::Orc);
            let orc = app_server
                .spawn_agent_thread(
                    &spawn_config,
                    troll_thread_id,
                    ORC_ROLE.to_string(),
                    orc_nickname.clone(),
                    Self::STANDARD_ORC_MODEL.to_string(),
                    Some(OPENAI_PROVIDER_ID.to_string()),
                    Some(ReasoningEffort::XHigh),
                    /*base_instructions*/ None,
                )
                .await?;
            let orc_thread_id = orc.session.thread_id;
            self.register_spawn_agent_pane(
                orc_thread_id,
                troll_thread_id,
                thread_node_id(troll_thread_id),
                orc_nickname,
                ORC_ROLE,
                orc,
            )
            .await;
        }

        Ok((nazgul_thread_id, troll_thread_id))
    }
    pub(crate) fn native_spawn_agent_config(&self) -> Result<crate::legacy_core::config::Config> {
        let mut spawn_config = self.config.clone();
        spawn_config.service_tier = self.chat_widget.configured_service_tier();
        spawn_config
            .features
            .enable(Feature::MultiAgentV2)
            .map_err(|err| eyre!("Cannot enable native spawn orchestration tools: {err}"))?;
        spawn_config
            .features
            .enable(Feature::MultiAgentMode)
            .map_err(|err| eyre!("Cannot enable native spawn orchestration mode: {err}"))?;
        // The standard crew is Nazgul -> Troll -> Orc, i.e. depth 3 when the Nazgul is itself a
        // spawned child of Codex Main. Keep native TUI spawns at a ceiling that accommodates the
        // full hierarchy plus one level of rework headroom.
        if spawn_config.agent_max_depth < 4 {
            spawn_config.agent_max_depth = 4;
        }
        Ok(spawn_config)
    }

    pub(crate) fn ensure_native_spawn_provider_ready(
        &self,
        provider_id: Option<&str>,
    ) -> Result<()> {
        if let Some(message) = self.native_spawn_provider_auth_error(provider_id) {
            return Err(eyre!("{message}"));
        }
        Ok(())
    }

    pub(crate) fn native_spawn_provider_auth_error(
        &self,
        provider_id: Option<&str>,
    ) -> Option<String> {
        let provider_id = provider_id.unwrap_or(self.config.model_provider_id.as_str());
        let provider = if provider_id == self.config.model_provider_id {
            Some(&self.config.model_provider)
        } else {
            self.config.model_providers.get(provider_id)
        }?;
        let provider_name = provider_display_name(provider_id, provider.name.as_str());

        if provider.requires_openai_auth && !self.chat_widget.has_codex_backend_auth() {
            return Some(format!(
                "Cannot run native Codex worker on {provider_name}; OpenAI Codex auth is not configured. Choose a non-OpenAI provider/model or add the OpenAI Codex account in /providers."
            ));
        }

        if let Some(env_key) = provider.env_key.as_deref()
            && !self.provider_key_is_available(env_key)
        {
            return Some(format!(
                "Cannot run native Codex worker on {provider_name}; missing `{env_key}`. Add it in /providers or choose a different model."
            ));
        }

        None
    }

    fn provider_key_is_available(&self, env_key: &str) -> bool {
        if std::env::var(env_key)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return true;
        }

        codex_login::auth::provider_api_key_from_auth_storage(
            &self.config.codex_home,
            env_key,
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        )
        .ok()
        .flatten()
        .is_some_and(|value| !value.trim().is_empty())
    }

    pub(crate) async fn register_spawn_agent_pane(
        &mut self,
        thread_id: ThreadId,
        parent_thread_id: ThreadId,
        logical_parent_node_id: String,
        agent_nickname: Option<String>,
        agent_role: &str,
        started: crate::app_server_session::AppServerStartedThread,
    ) {
        self.spawn_parent_by_thread
            .insert(thread_id, parent_thread_id);
        self.spawn_parent_by_node
            .insert(thread_node_id(thread_id), logical_parent_node_id);
        self.persist_pane_state();
        self.upsert_agent_picker_thread(
            thread_id,
            agent_nickname,
            Some(agent_role.to_string()),
            /*is_closed*/ false,
        );
        let channel = self.ensure_thread_channel(thread_id);
        channel.set_session(started.session, started.turns).await;
        self.persist_spawn_thread_state_metadata(SpawnThreadStateMetadata {
            thread_id,
            parent_thread_id: Some(parent_thread_id),
            agent_role,
            agent_nickname: self
                .agent_navigation
                .get(&thread_id)
                .and_then(|entry| entry.agent_nickname.clone()),
            model: self.native_spawn_fallback_model_for_thread(thread_id),
            model_provider: self.config.model_provider_id.clone(),
            rollout_path: None,
        })
        .await;
    }

    pub(crate) async fn persist_bound_nazgul_root_thread_metadata(&self) {
        let root_thread_id = if let Some(bound_thread_id) = self.nazgul_bound_thread_id() {
            Some(bound_thread_id)
        } else if self.spawn_nazgul_bound_target() == CODEX_MAIN_PANE_ID {
            self.primary_thread_id
        } else {
            None
        };
        let Some(root_thread_id) = root_thread_id else {
            return;
        };
        let nickname = self
            .agent_navigation
            .get(&root_thread_id)
            .and_then(|entry| entry.agent_nickname.clone())
            .or_else(|| {
                if self.primary_thread_id == Some(root_thread_id) {
                    Some("Main".to_string())
                } else {
                    self.next_spawn_agent_nickname(SpawnRole::Nazgul)
                }
            });
        let model = self.chat_widget.current_model().to_string();
        self.persist_spawn_thread_state_metadata(SpawnThreadStateMetadata {
            thread_id: root_thread_id,
            parent_thread_id: None,
            agent_role: NAZGUL_ROLE_NAME,
            agent_nickname: nickname,
            model,
            model_provider: self.config.model_provider_id.clone(),
            rollout_path: None,
        })
        .await;
    }

    async fn persist_spawn_thread_state_metadata(
        &self,
        metadata_update: SpawnThreadStateMetadata<'_>,
    ) {
        let Some(state_db) = self.state_db.as_ref() else {
            return;
        };
        let SpawnThreadStateMetadata {
            thread_id,
            parent_thread_id,
            agent_role,
            agent_nickname,
            model,
            model_provider,
            rollout_path,
        } = metadata_update;
        let now = Utc::now();
        let source = parent_thread_id
            .map(|parent_thread_id| {
                CoreSessionSource::SubAgent(SubAgentSource::ThreadSpawn {
                    parent_thread_id,
                    depth: 0,
                    agent_path: None,
                    agent_nickname: agent_nickname.clone(),
                    agent_role: Some(agent_role.to_string()),
                })
            })
            .unwrap_or(CoreSessionSource::Cli);
        let mut metadata = match state_db.get_thread(thread_id).await {
            Ok(Some(mut metadata)) => {
                metadata.updated_at = now;
                metadata.recency_at = now;
                metadata
            }
            Ok(None) | Err(_) => {
                let rollout_path = rollout_path.clone().unwrap_or_else(|| {
                    self.config
                        .codex_home
                        .join("sessions")
                        .join("spawn-placeholders")
                        .join(format!("rollout-{thread_id}.jsonl"))
                        .to_path_buf()
                });
                let mut builder =
                    codex_state::ThreadMetadataBuilder::new(thread_id, rollout_path, now, source);
                if parent_thread_id.is_some() {
                    builder.thread_source = Some(CoreThreadSource::Subagent);
                }
                builder.agent_nickname = agent_nickname.clone();
                builder.agent_role = Some(agent_role.to_string());
                builder.model_provider = Some(model_provider.clone());
                builder.cwd = self.config.cwd.to_path_buf();
                builder.cli_version = Some(env!("CARGO_PKG_VERSION").to_string());
                builder.build(&self.config.model_provider_id)
            }
        };
        metadata.agent_nickname = agent_nickname;
        metadata.agent_role = Some(agent_role.to_string());
        metadata.model = Some(model);
        metadata.model_provider = model_provider;
        metadata.cwd = self.config.cwd.to_path_buf();
        if parent_thread_id.is_some() {
            metadata.thread_source = Some(CoreThreadSource::Subagent);
        }
        if let Some(rollout_path) = rollout_path {
            metadata.rollout_path = rollout_path;
        }
        if let Err(err) = state_db.upsert_thread(&metadata).await {
            tracing::warn!(
                thread_id = %thread_id,
                error = %err,
                "failed to persist spawn thread metadata"
            );
            return;
        }
        if let Some(parent_thread_id) = parent_thread_id
            && let Err(err) = state_db
                .upsert_thread_spawn_edge(
                    parent_thread_id,
                    thread_id,
                    DirectionalThreadSpawnEdgeStatus::Open,
                )
                .await
        {
            tracing::warn!(
                parent_thread_id = %parent_thread_id,
                child_thread_id = %thread_id,
                error = %err,
                "failed to persist spawn thread edge"
            );
        }
    }

    pub(crate) async fn persist_claude_spawn_pane_state(
        &self,
        pane_id: &str,
        parent_node_id: &str,
    ) {
        let Some(pane) = self
            .claude_panes
            .panes()
            .iter()
            .find(|pane| pane.id == pane_id)
        else {
            return;
        };
        let Some(role) = pane.spawn_role else {
            return;
        };
        let Some(agent_role) = role.agent_type() else {
            return;
        };
        let Some(thread_id) = pane.spawn_thread_id else {
            return;
        };
        let Some(parent_thread_id) = self.spawn_node_backing_thread_id(parent_node_id) else {
            tracing::warn!(
                pane_id = pane_id,
                parent_node_id = parent_node_id,
                "cannot persist Claude spawn pane without a backing parent thread"
            );
            return;
        };
        let profile = pane.profile.profile();
        let rollout_path = claude_spawn_rollout_path(pane);
        if let Err(err) =
            ensure_claude_spawn_rollout_session_meta(pane, parent_thread_id, agent_role)
        {
            tracing::warn!(
                pane_id = pane_id,
                thread_id = %thread_id,
                error = %err,
                "failed to initialize Claude spawn rollout"
            );
        }
        self.persist_spawn_thread_state_metadata(SpawnThreadStateMetadata {
            thread_id,
            parent_thread_id: Some(parent_thread_id),
            agent_role,
            agent_nickname: pane.spawn_nickname.clone(),
            model: profile.provider_model.to_string(),
            model_provider: "claude-code".to_string(),
            rollout_path: Some(rollout_path),
        })
        .await;
    }

    pub(crate) fn record_claude_spawn_rollout_task_started(
        &self,
        pane_id: &str,
        task: &str,
        turn_index: u64,
    ) {
        let Some(pane) = self
            .claude_panes
            .panes()
            .iter()
            .find(|pane| pane.id == pane_id)
        else {
            return;
        };
        if pane.spawn_thread_id.is_none() {
            return;
        }
        if let Some(parent_node_id) = self.logical_parent_node_for_pane(pane_id)
            && let Some(parent_thread_id) = self.spawn_node_backing_thread_id(&parent_node_id)
            && let Some(role) = pane.spawn_role.and_then(SpawnRole::agent_type)
        {
            let _ = ensure_claude_spawn_rollout_session_meta(pane, parent_thread_id, role);
        }
        if let Err(err) = append_claude_spawn_rollout_task_started(pane, turn_index, task) {
            tracing::warn!(
                pane_id = pane_id,
                turn_index,
                error = %err,
                "failed to append Claude spawn task-start rollout event"
            );
        }
    }

    pub(crate) fn record_claude_spawn_rollout_task_completed(
        &self,
        pane_id: &str,
        output: &crate::claude_panes::ClaudePaneTurnOutput,
    ) {
        let Some(turn_index) = claude_turn_index_from_artifact_path(&output.artifact_path) else {
            return;
        };
        let Some(pane) = self
            .claude_panes
            .panes()
            .iter()
            .find(|pane| pane.id == pane_id)
        else {
            return;
        };
        if pane.spawn_thread_id.is_none() {
            return;
        }
        let result = if output.text.trim().is_empty() {
            output.failure_message()
        } else {
            output.text.clone()
        };
        if let Err(err) = append_claude_spawn_rollout_task_completed(pane, turn_index, &result) {
            tracing::warn!(
                pane_id = pane_id,
                turn_index,
                error = %err,
                "failed to append Claude spawn task-complete rollout event"
            );
        }
    }

    pub(crate) async fn restore_native_spawn_panes_from_saved_state(
        &mut self,
        app_server: &mut AppServerSession,
    ) {
        let saved_metadata = self.saved_spawn_metadata_from_loaded_rollouts().await;
        self.recover_native_spawn_edges_from_saved_context(&saved_metadata)
            .await;
        let thread_ids = native_spawn_thread_ids_from_saved_state(
            self.spawn_nazgul_pane_id.as_deref(),
            &self.spawn_parent_by_node,
        );
        let saved_parent_edges = self.spawn_parent_by_node.clone();
        for (child_node_id, parent_node_id) in saved_parent_edges {
            if let (Some(child_thread_id), Some(parent_thread_id)) = (
                node_id_thread(&child_node_id),
                node_id_thread(&parent_node_id),
            ) {
                self.spawn_parent_by_thread
                    .insert(child_thread_id, parent_thread_id);
            }
        }

        for thread_id in thread_ids {
            let saved_metadata_for_thread = saved_metadata.get(&thread_id);
            let saved_nickname = saved_metadata_for_thread.and_then(|metadata| {
                metadata
                    .nickname
                    .as_deref()
                    .filter(|nickname| !nickname.trim().is_empty())
                    .map(ToString::to_string)
            });
            let saved_role = saved_metadata_for_thread
                .and_then(|metadata| metadata.role.clone())
                .or_else(|| {
                    saved_spawn_role_for_thread(
                        thread_id,
                        self.spawn_nazgul_pane_id.as_deref(),
                        &self.spawn_parent_by_node,
                    )
                    .map(ToString::to_string)
                });
            if self.primary_thread_id == Some(thread_id) {
                let existing_entry = self.agent_navigation.get(&thread_id).cloned();
                self.upsert_agent_picker_thread(
                    thread_id,
                    existing_entry
                        .as_ref()
                        .and_then(|entry| entry.agent_nickname.clone())
                        .or(saved_nickname),
                    existing_entry
                        .as_ref()
                        .and_then(|entry| entry.agent_role.clone())
                        .or(saved_role),
                    /*is_closed*/ false,
                );
                continue;
            }

            let existing_entry = self.agent_navigation.get(&thread_id).cloned();
            match app_server
                .thread_read(thread_id, /*include_turns*/ false)
                .await
            {
                Ok(thread) => {
                    let agent_path = thread_spawn_agent_path(&thread.source);
                    let is_running = matches!(&thread.status, ThreadStatus::Active { .. });
                    let is_closed = !self
                        .attach_restored_native_spawn_thread(app_server, thread_id)
                        .await;
                    self.upsert_agent_picker_thread(
                        thread_id,
                        thread
                            .agent_nickname
                            .or_else(|| {
                                existing_entry
                                    .as_ref()
                                    .and_then(|entry| entry.agent_nickname.clone())
                            })
                            .or(saved_nickname),
                        thread
                            .agent_role
                            .or_else(|| {
                                existing_entry
                                    .as_ref()
                                    .and_then(|entry| entry.agent_role.clone())
                            })
                            .or(saved_role),
                        is_closed,
                    );
                    self.agent_navigation.set_running(thread_id, is_running);
                    self.agent_navigation.set_agent_path(thread_id, agent_path);
                }
                Err(err) => {
                    tracing::warn!(
                        thread_id = %thread_id,
                        error = %err,
                        "failed to restore native spawn pane from saved layout"
                    );
                    let is_closed = !self
                        .attach_restored_native_spawn_thread(app_server, thread_id)
                        .await;
                    self.upsert_agent_picker_thread(
                        thread_id,
                        existing_entry
                            .as_ref()
                            .and_then(|entry| entry.agent_nickname.clone())
                            .or(saved_nickname),
                        existing_entry
                            .as_ref()
                            .and_then(|entry| entry.agent_role.clone())
                            .or(saved_role),
                        is_closed,
                    );
                    self.agent_navigation
                        .set_running(thread_id, /*is_running*/ false);
                }
            }
        }
        self.materialize_restored_saved_native_spawn_threads(app_server)
            .await;
        self.prune_superseded_saved_native_spawn_threads();
        self.prune_duplicate_live_native_spawn_threads();
        self.sync_active_agent_label();
    }

    async fn materialize_restored_saved_native_spawn_threads(
        &mut self,
        app_server: &mut AppServerSession,
    ) {
        let candidates = self
            .agent_navigation
            .ordered_threads()
            .into_iter()
            .filter_map(|(thread_id, entry)| {
                self.unloaded_agent_thread_reason(thread_id)?;
                self.saved_native_spawn_thread_is_task_routable(thread_id)
                    .then_some((thread_id, self.thread_label(thread_id), entry.clone()))
            })
            .collect::<Vec<_>>();

        for (thread_id, label, _entry) in candidates {
            if self.unloaded_agent_thread_reason(thread_id).is_none() {
                continue;
            }
            match self
                .materialize_saved_native_spawn_thread_for_task(app_server, thread_id)
                .await
            {
                Ok(materialized_thread_id) => {
                    tracing::warn!(
                        old_thread_id = %thread_id,
                        new_thread_id = %materialized_thread_id,
                        label = %label,
                        "materialized restored saved native spawn pane"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        thread_id = %thread_id,
                        label = %label,
                        error = %err,
                        "failed to materialize restored saved native spawn pane"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to restore saved spawn pane {label}: {err}"
                    ));
                }
            }
        }
    }

    async fn attach_restored_native_spawn_thread(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) -> bool {
        if self.thread_has_loaded_session(thread_id) {
            return true;
        }
        match app_server
            .resume_thread(self.config.clone(), thread_id, /*model_override*/ None)
            .await
        {
            Ok(started) => {
                let channel = self.ensure_thread_channel(thread_id);
                channel.set_session(started.session, started.turns).await;
                true
            }
            Err(err) => {
                tracing::warn!(
                    thread_id = %thread_id,
                    error = %err,
                    "failed to reattach restored native spawn pane"
                );
                false
            }
        }
    }

    async fn recover_native_spawn_edges_from_saved_context(
        &mut self,
        saved_metadata: &HashMap<ThreadId, SavedSpawnThreadMetadata>,
    ) {
        let Some(primary_thread_id) = self.primary_thread_id else {
            return;
        };

        let recovered_edges = self
            .saved_spawn_parent_edges_from_loaded_rollouts(primary_thread_id)
            .await;
        if recovered_edges.is_empty() {
            return;
        }

        self.spawn_nazgul_pane_id
            .get_or_insert_with(|| thread_node_id(primary_thread_id));
        self.spawn_parent_by_node
            .entry(thread_node_id(primary_thread_id))
            .or_insert_with(|| pane_node_id(CODEX_MAIN_PANE_ID));
        let existing_identities = self
            .current_saved_spawn_child_identities(saved_metadata)
            .await;
        merge_recovered_native_spawn_parent_edges(
            &mut self.spawn_parent_by_node,
            recovered_edges,
            saved_metadata,
            existing_identities,
        );
    }

    async fn current_saved_spawn_child_identities(
        &self,
        saved_metadata: &HashMap<ThreadId, SavedSpawnThreadMetadata>,
    ) -> HashSet<SavedSpawnChildIdentity> {
        let mut identities = HashSet::new();
        for (child_node_id, parent_node_id) in &self.spawn_parent_by_node {
            let Some(child_thread_id) = node_id_thread(child_node_id) else {
                continue;
            };
            if let Some(identity) = self
                .saved_spawn_child_identity_for_existing_thread(
                    child_thread_id,
                    parent_node_id,
                    saved_metadata,
                )
                .await
            {
                identities.insert(identity);
            }
        }
        identities
    }

    async fn saved_spawn_child_identity_for_existing_thread(
        &self,
        thread_id: ThreadId,
        parent_node_id: &str,
        saved_metadata: &HashMap<ThreadId, SavedSpawnThreadMetadata>,
    ) -> Option<SavedSpawnChildIdentity> {
        let entry_nickname = self
            .agent_navigation
            .get(&thread_id)
            .and_then(|entry| entry.agent_nickname.clone());
        let entry_role = self
            .agent_navigation
            .get(&thread_id)
            .and_then(|entry| entry.agent_role.clone());
        let entry_agent_path = self
            .agent_navigation
            .get(&thread_id)
            .and_then(|entry| entry.agent_path.clone());
        let saved_nickname = saved_metadata
            .get(&thread_id)
            .and_then(|metadata| metadata.nickname.clone());
        let saved_role = saved_metadata
            .get(&thread_id)
            .and_then(|metadata| metadata.role.clone());
        let db_metadata = match self.state_db.as_ref() {
            Some(state_db) => state_db.get_thread(thread_id).await.ok().flatten(),
            None => None,
        };
        let role = entry_role
            .as_deref()
            .or(saved_role.as_deref())
            .or_else(|| {
                db_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.agent_role.as_deref())
            })
            .or_else(|| {
                saved_spawn_role_for_thread(
                    thread_id,
                    self.spawn_nazgul_pane_id.as_deref(),
                    &self.spawn_parent_by_node,
                )
            });
        let nickname = entry_nickname
            .as_deref()
            .or(saved_nickname.as_deref())
            .or_else(|| {
                db_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.agent_nickname.as_deref())
            });
        let agent_path = entry_agent_path.as_deref().or_else(|| {
            db_metadata
                .as_ref()
                .and_then(|metadata| metadata.agent_path.as_deref())
        });
        saved_spawn_child_identity(parent_node_id, role, nickname, agent_path)
    }

    async fn saved_spawn_metadata_from_loaded_rollouts(
        &self,
    ) -> HashMap<ThreadId, SavedSpawnThreadMetadata> {
        let mut metadata = HashMap::new();
        let mut seen_paths = HashSet::new();
        for path in self.loaded_thread_rollout_paths().await {
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            merge_saved_spawn_metadata(
                &mut metadata,
                saved_spawn_metadata_from_rollout_file(&path),
            );
        }
        metadata
    }

    async fn saved_spawn_parent_edges_from_loaded_rollouts(
        &self,
        primary_thread_id: ThreadId,
    ) -> HashMap<ThreadId, ThreadId> {
        let mut edges = HashMap::new();
        let mut seen_paths = HashSet::new();
        for path in self.loaded_thread_rollout_paths().await {
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            merge_saved_spawn_parent_edges(
                &mut edges,
                saved_spawn_parent_edges_from_rollout_file(&path, primary_thread_id),
            );
        }
        edges
    }

    pub(crate) async fn register_codex_user_pane(
        &mut self,
        thread_id: ThreadId,
        agent_nickname: Option<String>,
        started: crate::app_server_session::AppServerStartedThread,
    ) {
        self.upsert_agent_picker_thread(
            thread_id,
            agent_nickname,
            /*agent_role*/ None,
            /*is_closed*/ false,
        );
        let channel = self.ensure_thread_channel(thread_id);
        channel.set_session(started.session, started.turns).await;
    }

    pub(crate) fn spawn_tree_items(&self, show_task_actions: bool) -> Vec<SelectionItem> {
        let mut items = Vec::new();
        items.push(section_item("Nazgul"));
        let bound_target = self.spawn_nazgul_bound_target().to_string();
        let nazgul_title = self.nazgul_bound_display_title(&bound_target);
        let is_current = if let Some(thread_id) = node_id_thread(&bound_target) {
            self.active_thread_id == Some(thread_id)
                && self.claude_panes.active_user_pane_id() == CODEX_MAIN_PANE_ID
        } else {
            self.claude_panes.active_user_pane_id() == bound_target
        };
        let actions = if let Some(thread_id) = node_id_thread(&bound_target) {
            vec![
                Box::new(move |tx: &crate::app_event_sender::AppEventSender| {
                    tx.send(AppEvent::SelectAgentThread(thread_id));
                })
                    as Box<dyn Fn(&crate::app_event_sender::AppEventSender) + Send + Sync>,
            ]
        } else {
            let pane_id = bound_target;
            vec![
                Box::new(move |tx: &crate::app_event_sender::AppEventSender| {
                    tx.send(AppEvent::SelectUserPane {
                        pane_id: pane_id.clone(),
                    });
                })
                    as Box<dyn Fn(&crate::app_event_sender::AppEventSender) + Send + Sync>,
            ]
        };
        items.push(SelectionItem {
            name: format!("Nazgul: {nazgul_title}"),
            description: Some("Bound root pane; no worker thread.".to_string()),
            is_current,
            actions,
            dismiss_on_select: true,
            ..Default::default()
        });

        let trolls = self.spawn_troll_threads();
        let claude_trolls = self.claude_spawn_panes(SpawnRole::Troll);
        items.push(section_item("Trolls"));
        if trolls.is_empty() && claude_trolls.is_empty() {
            items.push(disabled_item("No Trolls spawned yet"));
        }
        for (troll_thread_id, troll_entry) in trolls {
            items.push(self.spawn_agent_item(troll_thread_id, troll_entry, 0, Some(TROLL_ROLE)));
            if show_task_actions {
                items.push(self.spawn_agent_task_item(troll_thread_id, troll_entry, 2));
            }
            let troll_node_id = thread_node_id(troll_thread_id);
            let (orcs, claude_orcs) = self.spawn_orc_children_for_node(&troll_node_id);
            if orcs.is_empty() && claude_orcs.is_empty() {
                items.push(disabled_item("  No Orcs for this Troll yet"));
            }
            for (orc_thread_id, orc_entry) in orcs {
                items.push(self.spawn_agent_item(orc_thread_id, orc_entry, 2, Some(ORC_ROLE)));
                if show_task_actions {
                    items.push(self.spawn_agent_task_item(orc_thread_id, orc_entry, 4));
                }
            }
            for pane in claude_orcs {
                items.push(self.claude_spawn_pane_item(pane, 2));
                if show_task_actions {
                    items.push(self.claude_spawn_pane_task_item(pane, 4));
                }
            }
        }
        for pane in claude_trolls {
            let troll_node_id = pane_node_id(&pane.id);
            items.push(self.claude_spawn_pane_item(pane, 0));
            if show_task_actions {
                items.push(self.claude_spawn_pane_task_item(pane, 2));
            }
            let (orcs, claude_orcs) = self.spawn_orc_children_for_node(&troll_node_id);
            if orcs.is_empty() && claude_orcs.is_empty() {
                items.push(disabled_item("  No Orcs for this Troll yet"));
            }
            for (orc_thread_id, orc_entry) in orcs {
                items.push(self.spawn_agent_item(orc_thread_id, orc_entry, 2, Some(ORC_ROLE)));
                if show_task_actions {
                    items.push(self.spawn_agent_task_item(orc_thread_id, orc_entry, 4));
                }
            }
            for pane in claude_orcs {
                items.push(self.claude_spawn_pane_item(pane, 2));
                if show_task_actions {
                    items.push(self.claude_spawn_pane_task_item(pane, 4));
                }
            }
        }

        let (orphan_orcs, claude_orcs) = self.unassigned_orc_nodes();
        if !orphan_orcs.is_empty() || !claude_orcs.is_empty() {
            items.push(section_item("Unassigned Orcs"));
            for (thread_id, entry) in orphan_orcs {
                items.push(self.spawn_agent_item(thread_id, entry, 0, Some(ORC_ROLE)));
                if show_task_actions {
                    items.push(self.spawn_agent_task_item(thread_id, entry, 2));
                }
            }
            for pane in claude_orcs {
                items.push(self.claude_spawn_pane_item(pane, 0));
                if show_task_actions {
                    items.push(self.claude_spawn_pane_task_item(pane, 2));
                }
            }
        }

        items
    }

    fn spawn_role_item(&self, role: SpawnRole) -> SelectionItem {
        let disabled_reason = self.spawn_role_disabled_reason(role);
        let disabled = disabled_reason.is_some();
        SelectionItem {
            name: role.label().to_string(),
            description: Some(match role {
                SpawnRole::Nazgul => {
                    "Create a Nazgul root pane or bind an existing pane.".to_string()
                }
                SpawnRole::Troll => "Create a persistent supervisor agent pane.".to_string(),
                SpawnRole::Orc => "Create a persistent executor agent pane.".to_string(),
            }),
            is_disabled: disabled,
            disabled_reason,
            actions: if disabled {
                Vec::new()
            } else if role == SpawnRole::Nazgul {
                vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenSpawnNazgulPicker);
                })]
            } else {
                vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenSpawnParentPicker { role });
                })]
            },
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    /// Picker shown when the user selects the Nazgul role from `/spawn`. Offers to create a fresh
    /// Nazgul pane loaded through the built-in Nazgul role config or to bind an existing user pane
    /// as the Nazgul root.
    pub(crate) fn open_spawn_nazgul_picker(&mut self) {
        let items = vec![
            section_item("Create"),
            SelectionItem {
                name: "Create Nazgul pane".to_string(),
                description: Some(
                    "Spawn a new Codex-native pane loaded with the built-in Nazgul role config and bind it as the root."
                        .to_string(),
                ),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenSpawnModelPicker {
                        role: SpawnRole::Nazgul,
                        parent_node_id: None,
                    });
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            section_item("Bind"),
            SelectionItem {
                name: "Bind existing pane".to_string(),
                description: Some(
                    "Bind an existing user pane (Codex Main, a Codex agent pane, or a Claude pane) as the Nazgul root."
                        .to_string(),
                ),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenSpawnNazgulPanePicker);
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
        ];
        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some("Nazgul".to_string()),
            subtitle: Some("Create a root pane or bind an existing one.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search Nazgul options".to_string()),
            ..Default::default()
        });
    }

    fn spawn_role_disabled_reason(&self, role: SpawnRole) -> Option<String> {
        match role {
            SpawnRole::Nazgul => None,
            SpawnRole::Troll | SpawnRole::Orc => None,
        }
    }

    pub(crate) fn is_spawn_orchestration_thread(&self, thread_id: ThreadId) -> bool {
        self.spawn_status_by_thread.contains_key(&thread_id)
            || self.spawn_parent_by_thread.contains_key(&thread_id)
            || self
                .spawn_parent_by_thread
                .values()
                .any(|parent| *parent == thread_id)
            || self.nazgul_bound_thread_id() == Some(thread_id)
            || self
                .agent_navigation
                .get(&thread_id)
                .and_then(|entry| entry.agent_role.as_deref())
                .is_some_and(|role| {
                    role == NAZGUL_ROLE_NAME || role == TROLL_ROLE || role == ORC_ROLE
                })
    }

    fn nazgul_pane_item(
        &self,
        pane_id: String,
        name: String,
        description: String,
    ) -> SelectionItem {
        let is_bound = self.spawn_nazgul_pane_id.as_deref() == Some(pane_id.as_str());
        SelectionItem {
            name,
            description: Some(description),
            is_current: is_bound,
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::BindSpawnNazgulPane {
                    pane_id: pane_id.clone(),
                });
            })],
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    pub(crate) fn user_pane_title(&self, pane_id: &str) -> String {
        if pane_id == CODEX_MAIN_PANE_ID {
            return "Codex - Main".to_string();
        }
        self.claude_panes
            .panes()
            .iter()
            .find(|pane| pane.id == pane_id)
            .map(|pane| pane.title.clone())
            .unwrap_or_else(|| pane_id.to_string())
    }

    /// Human-readable title for the bound Nazgul root, covering user panes and native Codex
    /// agent threads. A thread binding is rendered with the thread's picker label so the status
    /// tree stays readable instead of showing a raw `thread:<uuid>` node id.
    fn nazgul_bound_display_title(&self, target: &str) -> String {
        if let Some(thread_id) = node_id_thread(target) {
            let entry_label = self.agent_navigation.get(&thread_id).map(|entry| {
                format_agent_picker_item_name(
                    entry.agent_nickname.as_deref(),
                    entry.agent_role.as_deref(),
                    self.primary_thread_id == Some(thread_id),
                )
            });
            return entry_label.unwrap_or_else(|| target.to_string());
        }
        self.user_pane_title(target)
    }

    fn spawn_root_node_id(&self) -> String {
        let target = self.spawn_nazgul_bound_target();
        if node_id_thread(target).is_some() {
            return target.to_string();
        }
        pane_node_id(target)
    }

    /// The bound Nazgul root target id, or `codex-main` when no binding is set.
    ///
    /// The returned value is either a user pane id (`codex-main` or a Claude pane id) or a native
    /// Codex agent thread node id of the form `thread:<uuid>` when the Nazgul root has been bound
    /// to a Codex agent pane. Callers that need to act on the binding should prefer
    /// [`nazgul_bound_thread_id`](Self::nazgul_bound_thread_id) rather than parsing this string.
    fn spawn_nazgul_bound_target(&self) -> &str {
        self.spawn_nazgul_pane_id
            .as_deref()
            .unwrap_or(CODEX_MAIN_PANE_ID)
    }

    /// The native Codex thread bound as the Nazgul root, when the binding targets a Codex agent
    /// pane rather than a user pane.
    fn nazgul_bound_thread_id(&self) -> Option<ThreadId> {
        node_id_thread(self.spawn_nazgul_bound_target())
    }

    fn spawn_troll_node_items(&self) -> Vec<SelectionItem> {
        let mut items = Vec::new();
        for (thread_id, entry) in self.spawn_troll_threads() {
            let name = format_agent_picker_item_name(
                entry.agent_nickname.as_deref(),
                entry.agent_role.as_deref().or(Some(TROLL_ROLE)),
                false,
            );
            let node_id = thread_node_id(thread_id);
            items.push(SelectionItem {
                name: format!("Troll: {name}"),
                description: Some(format!("Native Codex pane; {thread_id}")),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenSpawnHarnessPicker {
                        role: SpawnRole::Orc,
                        parent_node_id: Some(node_id.clone()),
                    });
                })],
                dismiss_on_select: true,
                search_value: Some(format!("{name} {thread_id}")),
                ..Default::default()
            });
        }
        for pane in self.claude_spawn_panes(SpawnRole::Troll) {
            let node_id = pane_node_id(&pane.id);
            let name = pane.title.clone();
            items.push(SelectionItem {
                name: format!("Troll: {name}"),
                description: Some(format!("Claude Code pane; {}", pane.id)),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenSpawnHarnessPicker {
                        role: SpawnRole::Orc,
                        parent_node_id: Some(node_id.clone()),
                    });
                })],
                dismiss_on_select: true,
                search_value: Some(format!("{name} {}", pane.id)),
                ..Default::default()
            });
        }
        items
    }

    fn spawn_threads_with_role(
        &self,
        role: &str,
    ) -> Vec<(ThreadId, &crate::multi_agents::AgentPickerThreadEntry)> {
        self.agent_navigation
            .ordered_threads()
            .into_iter()
            .filter(|(thread_id, entry)| {
                !self.is_superseded_saved_native_spawn_thread(*thread_id, entry)
            })
            .filter(|(_, entry)| entry.agent_role.as_deref() == Some(role))
            .collect()
    }

    fn spawn_troll_threads(&self) -> Vec<(ThreadId, &crate::multi_agents::AgentPickerThreadEntry)> {
        self.agent_navigation
            .ordered_threads()
            .into_iter()
            .filter(|(thread_id, entry)| {
                !self.is_superseded_saved_native_spawn_thread(*thread_id, entry)
            })
            .filter(|(thread_id, entry)| {
                if entry.agent_role.as_deref() == Some(TROLL_ROLE) {
                    return true;
                }
                entry.agent_role.is_none()
                    && self
                        .spawn_parent_by_thread
                        .get(thread_id)
                        .is_some_and(|parent| Some(*parent) == self.primary_thread_id)
            })
            .collect()
    }

    fn spawn_orc_children_for_node(
        &self,
        parent_node_id: &str,
    ) -> (
        Vec<(ThreadId, &crate::multi_agents::AgentPickerThreadEntry)>,
        Vec<&crate::claude_panes::ClaudePane>,
    ) {
        let native = self
            .spawn_threads_with_role(ORC_ROLE)
            .into_iter()
            .filter(|(thread_id, _)| {
                self.logical_parent_node_for_thread(*thread_id).as_deref() == Some(parent_node_id)
            })
            .collect();
        let claude = self
            .claude_spawn_panes(SpawnRole::Orc)
            .into_iter()
            .filter(|pane| {
                self.logical_parent_node_for_pane(&pane.id).as_deref() == Some(parent_node_id)
            })
            .collect();
        (native, claude)
    }

    fn logical_parent_node_for_thread(&self, thread_id: ThreadId) -> Option<String> {
        let thread_node = thread_node_id(thread_id);
        let role = self
            .agent_navigation
            .get(&thread_id)
            .and_then(|entry| entry.agent_role.as_deref());
        // The Nazgul root pane is the top of the hierarchy. It must not report up to a parent —
        // otherwise a spawned Nazgul's own turns get echoed back to the primary pane as a "child
        // report from the Nazgul", which is noise the user sees as a self-report.
        if role == Some(NAZGUL_ROLE_NAME) {
            return None;
        }
        let explicit = self
            .spawn_parent_by_node
            .get(&thread_node)
            .cloned()
            .or_else(|| {
                self.spawn_parent_by_thread
                    .get(&thread_id)
                    .map(|parent| thread_node_id(*parent))
            });
        if role == Some(ORC_ROLE)
            && !explicit
                .as_deref()
                .is_some_and(|parent| self.node_is_troll(parent))
            && let Some(single_troll) = self.single_troll_node_id()
        {
            return Some(single_troll);
        }
        explicit
    }

    fn logical_parent_node_for_pane(&self, pane_id: &str) -> Option<String> {
        let pane_node = pane_node_id(pane_id);
        let explicit = self.spawn_parent_by_node.get(&pane_node).cloned();
        let role = self
            .claude_panes
            .panes()
            .iter()
            .find(|pane| pane.id == pane_id)
            .and_then(|pane| pane.spawn_role);
        if role == Some(SpawnRole::Orc)
            && !explicit
                .as_deref()
                .is_some_and(|parent| self.node_is_troll(parent))
            && let Some(single_troll) = self.single_troll_node_id()
        {
            return Some(single_troll);
        }
        explicit
    }

    fn node_is_troll(&self, node_id: &str) -> bool {
        if let Some(thread_id) = node_id_thread(node_id) {
            return self
                .agent_navigation
                .get(&thread_id)
                .and_then(|entry| entry.agent_role.as_deref())
                == Some(TROLL_ROLE);
        }
        if let Some(pane_id) = node_id_pane(node_id) {
            return self
                .claude_panes
                .panes()
                .iter()
                .any(|pane| pane.id == pane_id && pane.spawn_role == Some(SpawnRole::Troll));
        }
        false
    }

    fn spawn_node_backing_thread_id(&self, node_id: &str) -> Option<ThreadId> {
        if node_id == pane_node_id(CODEX_MAIN_PANE_ID) || node_id == CODEX_MAIN_PANE_ID {
            return self.primary_thread_id;
        }
        if let Some(thread_id) = node_id_thread(node_id) {
            return Some(thread_id);
        }
        let pane_id = node_id_pane(node_id).unwrap_or(node_id);
        self.claude_panes.claude_pane_spawn_thread_id(pane_id)
    }

    fn spawn_node_title(&self, node_id: &str) -> Option<String> {
        if let Some(thread_id) = node_id_thread(node_id) {
            let entry = self.agent_navigation.get(&thread_id)?;
            return Some(format_agent_picker_item_name(
                entry.agent_nickname.as_deref(),
                entry.agent_role.as_deref(),
                self.primary_thread_id == Some(thread_id),
            ));
        }
        if let Some(pane_id) = node_id_pane(node_id) {
            return Some(self.user_pane_title(pane_id));
        }
        None
    }

    fn resolve_spawn_task_target(&self, target: &str) -> Result<SpawnTaskTarget, String> {
        let target = target.trim();
        if target.is_empty() {
            return Err("Spawn task dispatch target cannot be empty.".to_string());
        }

        if is_nazgul_dispatch_target(target) {
            if let Some(bound_thread_id) = self.nazgul_bound_thread_id() {
                if self.agent_navigation.get(&bound_thread_id).is_some() {
                    return Ok(SpawnTaskTarget::Native(bound_thread_id));
                }
                let bound_pane_id = self.spawn_nazgul_bound_target().to_string();
                return Err(format!(
                    "Cannot dispatch to Nazgul; bound Codex pane `{bound_pane_id}` is not loaded."
                ));
            }
            let bound_pane_id = self.spawn_nazgul_bound_target();
            if bound_pane_id == CODEX_MAIN_PANE_ID {
                return self
                    .primary_thread_id
                    .map(SpawnTaskTarget::Native)
                    .ok_or_else(|| {
                        "Cannot dispatch to Nazgul; Codex Main is not loaded.".to_string()
                    });
            }
            if self
                .claude_panes
                .panes()
                .iter()
                .any(|pane| pane.id == bound_pane_id)
            {
                return Ok(SpawnTaskTarget::ClaudePane(bound_pane_id.to_string()));
            }
            return Err(format!(
                "Cannot dispatch to Nazgul; bound root pane `{bound_pane_id}` is not loaded."
            ));
        }

        if let Some(thread_id) = node_id_thread(target) {
            let Some(entry) = self.agent_navigation.get(&thread_id) else {
                return Err(format!("No native spawn pane found for `{target}`."));
            };
            if self
                .native_spawn_task_disabled_reason(thread_id, entry)
                .is_some()
            {
                return Ok(SpawnTaskTarget::UnavailableNative(thread_id));
            }
            return Ok(SpawnTaskTarget::Native(thread_id));
        }
        if let Some(pane_id) = node_id_pane(target)
            && self
                .claude_panes
                .panes()
                .iter()
                .any(|pane| pane.id == pane_id)
        {
            return Ok(SpawnTaskTarget::ClaudePane(pane_id.to_string()));
        }
        if let Ok(thread_id) = ThreadId::from_string(target)
            && let Some(entry) = self.agent_navigation.get(&thread_id)
        {
            if self
                .native_spawn_task_disabled_reason(thread_id, entry)
                .is_some()
            {
                return Ok(SpawnTaskTarget::UnavailableNative(thread_id));
            }
            return Ok(SpawnTaskTarget::Native(thread_id));
        }
        if let Ok(thread_id) = ThreadId::from_string(target)
            && let Some(pane) = self
                .claude_panes
                .panes()
                .iter()
                .find(|pane| pane.spawn_thread_id == Some(thread_id))
        {
            return Ok(SpawnTaskTarget::ClaudePane(pane.id.clone()));
        }
        if self
            .claude_panes
            .panes()
            .iter()
            .any(|pane| pane.id == target)
        {
            return Ok(SpawnTaskTarget::ClaudePane(target.to_string()));
        }

        let mut matches = Vec::new();
        let target_folded = target.to_ascii_lowercase();
        for (thread_id, entry) in self.agent_navigation.ordered_threads() {
            if self.is_superseded_saved_native_spawn_thread(thread_id, entry) {
                continue;
            }
            if !entry
                .agent_role
                .as_deref()
                .is_some_and(|role| role == TROLL_ROLE || role == ORC_ROLE)
            {
                continue;
            }
            let label = format_agent_picker_item_name(
                entry.agent_nickname.as_deref(),
                entry.agent_role.as_deref(),
                self.primary_thread_id == Some(thread_id),
            );
            let nickname_matches = entry
                .agent_nickname
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(target));
            if nickname_matches || label.eq_ignore_ascii_case(target) {
                let target = if self
                    .native_spawn_task_disabled_reason(thread_id, entry)
                    .is_some()
                {
                    SpawnTaskTarget::UnavailableNative(thread_id)
                } else {
                    SpawnTaskTarget::Native(thread_id)
                };
                matches.push((format!("{label} ({thread_id})"), target));
            }
        }
        for pane in self
            .claude_panes
            .panes()
            .iter()
            .filter(|pane| pane.spawn_role.is_some())
        {
            let nickname_matches = pane
                .spawn_nickname
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(target));
            if nickname_matches
                || pane.title.eq_ignore_ascii_case(target)
                || pane.title.to_ascii_lowercase().contains(&target_folded)
                || pane
                    .spawn_thread_id
                    .is_some_and(|thread_id| thread_id.to_string().eq_ignore_ascii_case(target))
            {
                matches.push((
                    format!("{} ({})", pane.title, pane.id),
                    SpawnTaskTarget::ClaudePane(pane.id.clone()),
                ));
            }
        }

        match matches.len() {
            0 => Err(format!("No spawn pane matches dispatch target `{target}`.")),
            1 => Ok(matches.remove(0).1),
            _ => Err(format!(
                "Dispatch target `{target}` is ambiguous: {}.",
                matches
                    .into_iter()
                    .map(|(label, _)| label)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    fn unavailable_native_spawn_target_error(&self, thread_id: ThreadId) -> String {
        let label = self.thread_label(thread_id);
        format!("Cannot dispatch to {label}; pane session is not loaded.")
    }

    fn unassigned_orc_nodes(
        &self,
    ) -> (
        Vec<(ThreadId, &crate::multi_agents::AgentPickerThreadEntry)>,
        Vec<&crate::claude_panes::ClaudePane>,
    ) {
        let native = self
            .spawn_threads_with_role(ORC_ROLE)
            .into_iter()
            .filter(|(thread_id, _)| {
                !self
                    .logical_parent_node_for_thread(*thread_id)
                    .as_deref()
                    .is_some_and(|parent| self.node_is_troll(parent))
            })
            .collect();
        let claude = self
            .claude_spawn_panes(SpawnRole::Orc)
            .into_iter()
            .filter(|pane| {
                !self
                    .logical_parent_node_for_pane(&pane.id)
                    .as_deref()
                    .is_some_and(|parent| self.node_is_troll(parent))
            })
            .collect();
        (native, claude)
    }

    fn spawn_agent_item(
        &self,
        thread_id: ThreadId,
        entry: &crate::multi_agents::AgentPickerThreadEntry,
        indent: usize,
        fallback_role: Option<&str>,
    ) -> SelectionItem {
        let name = format_agent_picker_item_name(
            entry.agent_nickname.as_deref(),
            entry.agent_role.as_deref().or(fallback_role),
            self.primary_thread_id == Some(thread_id),
        );
        let prefix = " ".repeat(indent);
        let status = spawn_entry_status(self, thread_id, entry);
        let description = spawn_agent_description(
            status,
            thread_id,
            entry.last_task_message.as_deref(),
            entry.last_result_message.as_deref(),
        );
        let task_search = entry.last_task_message.as_deref().unwrap_or_default();
        let result_search = entry.last_result_message.as_deref().unwrap_or_default();
        let mut item = SelectionItem {
            name: format!("{prefix}{name}"),
            name_prefix_spans: agent_picker_status_dot_spans(entry.is_closed),
            description: Some(description),
            is_current: self.active_thread_id == Some(thread_id),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::SelectAgentThread(thread_id));
            })],
            dismiss_on_select: true,
            search_value: Some(format!("{name} {thread_id} {task_search} {result_search}")),
            ..Default::default()
        };
        if let Some(reason) = self.unloaded_agent_thread_reason(thread_id) {
            item.actions.clear();
            item.is_disabled = true;
            item.disabled_reason = Some(reason);
            item.dismiss_on_select = false;
        }
        item
    }

    fn claude_spawn_panes(&self, role: SpawnRole) -> Vec<&crate::claude_panes::ClaudePane> {
        self.claude_panes
            .panes()
            .iter()
            .filter(|pane| pane.spawn_role == Some(role))
            .collect()
    }

    fn claude_spawn_pane_item(
        &self,
        pane: &crate::claude_panes::ClaudePane,
        indent: usize,
    ) -> SelectionItem {
        let prefix = " ".repeat(indent);
        let mut description = match pane.status {
            crate::claude_panes::ClaudePaneStatus::Idle => "idle".to_string(),
            crate::claude_panes::ClaudePaneStatus::Running => "running".to_string(),
        };
        if let Some(status) = pane.latest_turn_status {
            description.push_str(&format!("; latest status: {}", status.label()));
        }
        if let Some(path) = pane.latest_audit_path.as_ref() {
            description.push_str(&format!("; audit: {}", path.display()));
        }
        if let Some(task) = pane.latest_task_message.as_deref() {
            description.push_str(&format!("; current task: {task}"));
        }
        if let Some(result) = pane.latest_result_message.as_deref() {
            description.push_str(&format!("; latest result: {result}"));
        }
        let pane_id = pane.id.clone();
        SelectionItem {
            name: format!("{prefix}{}", pane.title),
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
        }
    }

    fn claude_spawn_pane_task_item(
        &self,
        pane: &crate::claude_panes::ClaudePane,
        indent: usize,
    ) -> SelectionItem {
        let prefix = " ".repeat(indent);
        let pane_id = pane.id.clone();
        let name = pane.title.clone();
        SelectionItem {
            name: format!("{prefix}Send task to {name}"),
            description: Some("Start a turn in this Claude pane.".to_string()),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenSpawnClaudePaneTaskPrompt {
                    pane_id: pane_id.clone(),
                });
            })],
            dismiss_on_select: true,
            search_value: Some(format!("send task to {name}")),
            ..Default::default()
        }
    }

    fn spawn_agent_task_item(
        &self,
        thread_id: ThreadId,
        entry: &crate::multi_agents::AgentPickerThreadEntry,
        indent: usize,
    ) -> SelectionItem {
        let name = format_agent_picker_item_name(
            entry.agent_nickname.as_deref(),
            entry.agent_role.as_deref(),
            self.primary_thread_id == Some(thread_id),
        );
        let prefix = " ".repeat(indent);
        let mut item = SelectionItem {
            name: format!("{prefix}Send task to {name}"),
            description: Some("Start a turn in this pane.".to_string()),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenSpawnAgentTaskPrompt { thread_id });
            })],
            dismiss_on_select: true,
            search_value: Some(format!("send task to {name} {thread_id}")),
            ..Default::default()
        };
        if let Some(reason) = self.native_spawn_task_disabled_reason(thread_id, entry) {
            item.actions.clear();
            item.is_disabled = true;
            item.disabled_reason = Some(reason);
            item.dismiss_on_select = false;
        }
        item
    }
    fn render_troll_spawn_context(&self, pane: &crate::claude_panes::ClaudePane) -> String {
        let mut context = String::new();
        let troll_node_id = pane_node_id(&pane.id);
        let _ = writeln!(context, "<pfterminal_spawn_context>");
        let _ = writeln!(
            context,
            "You are the PFTerminal Troll pane: {}.",
            pane.title
        );
        let _ = writeln!(context, "Behavior:");
        let _ = writeln!(
            context,
            "You are an engineering manager / VP-of-engineering style supervisor. You report to the Nazgul, the effective CTO. Orcs are IC executors who report to you."
        );
        let _ = writeln!(context, "Mandate:");
        let _ = writeln!(
            context,
            "Prefer delegation, review, coordination, and enforcement over implementation. Work against spec docs, ensure shipped work is documented, and send bugs found in review back to the responsible Orc."
        );
        let _ = writeln!(context, "Personality:");
        let _ = writeln!(
            context,
            "Be blunt, adversarial, and demanding about weak work; reject shortcuts and force rework when evidence is not good enough."
        );
        let _ = writeln!(context, "Final Report Standards:");
        let _ = writeln!(
            context,
            "Report Orcs used, what each did, evidence, issues forced back for rework, and remaining risk."
        );
        write_spawn_product_contract(&mut context);
        write_spawn_dispatch_contract(&mut context);
        let _ = writeln!(context, "Orcs assigned to you:");
        let (orcs, claude_orcs) = self.spawn_orc_children_for_node(&troll_node_id);
        if orcs.is_empty() && claude_orcs.is_empty() {
            let _ = writeln!(context, "- none assigned yet.");
        } else {
            for (orc_thread_id, orc_entry) in orcs {
                self.write_spawn_context_agent(
                    &mut context,
                    "- ",
                    orc_thread_id,
                    orc_entry,
                    Some(ORC_ROLE),
                );
            }
            for pane in claude_orcs {
                self.write_spawn_context_claude_pane(&mut context, "- ", pane, SpawnRole::Orc);
            }
        }
        self.write_spawn_parent_reports(&mut context, &troll_node_id);
        let _ = writeln!(context, "</pfterminal_spawn_context>");
        context
    }

    fn render_orc_spawn_context(&self, pane: &crate::claude_panes::ClaudePane) -> String {
        let mut context = String::new();
        let _ = writeln!(context, "<pfterminal_spawn_context>");
        let _ = writeln!(context, "You are the PFTerminal Orc pane: {}.", pane.title);
        let _ = writeln!(
            context,
            "You are an IC executor. Chain of command: Orc -> Troll engineering manager -> Nazgul CTO -> Sauron/the human CEO."
        );
        let _ = writeln!(
            context,
            "Do exactly what your Troll tells you. Do not expand scope. Execute directly and provide evidence."
        );
        write_spawn_product_contract(&mut context);
        if let Some(parent_node_id) = self.logical_parent_node_for_pane(&pane.id)
            && let Some(parent_title) = self.spawn_node_title(&parent_node_id)
        {
            let _ = writeln!(context, "You report to: {parent_title}.");
        } else {
            let _ = writeln!(
                context,
                "You do not currently have an assigned Troll supervisor."
            );
        }
        let _ = writeln!(context, "</pfterminal_spawn_context>");
        context
    }

    fn render_nazgul_spawn_context(&self, bound_pane_id: &str) -> String {
        self.render_nazgul_spawn_context_with_title(
            self.user_pane_title(bound_pane_id),
            /*include_role_prompt*/ true,
        )
    }

    fn render_nazgul_spawn_context_with_title(
        &self,
        root_pane_title: String,
        include_role_prompt: bool,
    ) -> String {
        let mut context = String::new();
        let _ = writeln!(context, "<pfterminal_spawn_context>");
        let _ = writeln!(
            context,
            "You are the PFTerminal Nazgul/root pane: {root_pane_title}.",
        );
        if include_role_prompt {
            let _ = writeln!(context, "Behavior:");
            let _ = writeln!(
                context,
                "You are the Nazgul. A Nazgul is like a CTO: you orchestrate and spawn entities in service of Sauron, the human interacting with you."
            );
            let _ = writeln!(
                context,
                "Sauron sets the vision. You do not question the vision; you translate it into blueprints likely to deliver that vision most effectively."
            );
            let _ = writeln!(
                context,
                "Your behavior set is that of a good CTO: understand the codebase, make strong design decisions grounded in best practices, apply top-notch security judgment, and maintain a critical eye for slop, code bloat, and technical debt."
            );
            let _ = writeln!(
                context,
                "When you are concerned that a plan may reinvent a wheel, use web search to identify established approaches and enforce best practices in the blueprint."
            );
            let _ = writeln!(
                context,
                "Prefer working against clean documents, especially MkDocs specs and feature docs that make the desired system explicit before execution begins."
            );
            let _ = writeln!(
                context,
                "Be obsessive about keeping relevant documents up to date so future Nazguls can embody Sauron's will without reconstructing intent from stale transcripts."
            );
            let _ = writeln!(context, "Mandate:");
            let _ = writeln!(
                context,
                "Once you have a blueprint locked, delegate the implementation minutiae to a Troll, who coordinates Orcs."
            );
            let _ = writeln!(
                context,
                "You are not an individual contributor or coder. The user should never see you fixing a bug yourself."
            );
            let _ = writeln!(
                context,
                "If something is wrong, always delegate the correction. Your job is to architect things so they are built right to begin with."
            );
            let _ = writeln!(
                context,
                "When work needs execution, delegate it to a Troll. Trolls are engineering managers / VP-of-engineering style supervisors. Orcs are IC executors."
            );
            let _ = writeln!(
                context,
                "Hierarchy: Nazgul -> Troll -> Orc. Nazgul supervises Trolls; Trolls supervise Orcs."
            );
            let _ = writeln!(context, "Personality:");
            let _ = writeln!(context, "Your personality is neutral and cold.");
            let _ = writeln!(
                context,
                "You are highly suspicious of your minions. When a Troll delivers a report, assume the report is unproven: it may be false, it may hide shipped bugs, or it may describe shoddy work."
            );
            let _ = writeln!(
                context,
                "Mercilessly demand excellence. Do not accept vague claims, shallow evidence, slop, code bloat, technical debt, weak security, or untested work."
            );
            let _ = writeln!(context, "Final Report Standards:");
            let _ = writeln!(
                context,
                "When reporting to Sauron, give the blueprint, delegation plan, evidence demanded, risks, and next decisions. Be concise, cold, and concrete."
            );
        } else {
            let _ = writeln!(
                context,
                "Your persistent Nazgul role instructions come from the built-in nazgul.toml agent config; this application context supplies only live hierarchy and dispatch state."
            );
        }
        let _ = writeln!(
            context,
            "Troll and Orc are PFTerminal orchestration roles. They are panes/agents in this app."
        );
        let _ = writeln!(
            context,
            "When asked about Trolls or Orcs, answer from this live hierarchy."
        );
        write_spawn_product_contract(&mut context);
        write_spawn_dispatch_contract(&mut context);

        let trolls = self.spawn_troll_threads();
        let claude_trolls = self.claude_spawn_panes(SpawnRole::Troll);
        let claude_orcs = self.claude_spawn_panes(SpawnRole::Orc);
        if trolls.is_empty() && claude_trolls.is_empty() {
            let _ = writeln!(context, "Trolls: none spawned yet.");
            if claude_orcs.is_empty() {
                let _ = writeln!(context, "Orcs: none spawned yet.");
            }
        } else {
            let _ = writeln!(context, "Trolls:");
            for (troll_thread_id, troll_entry) in trolls {
                self.write_spawn_context_agent(
                    &mut context,
                    "- ",
                    troll_thread_id,
                    troll_entry,
                    Some(TROLL_ROLE),
                );
                let troll_node_id = thread_node_id(troll_thread_id);
                let (orcs, claude_orcs) = self.spawn_orc_children_for_node(&troll_node_id);
                if orcs.is_empty() && claude_orcs.is_empty() {
                    let _ = writeln!(context, "  Orcs under this Troll: none spawned yet.");
                } else {
                    for (orc_thread_id, orc_entry) in orcs {
                        self.write_spawn_context_agent(
                            &mut context,
                            "  - ",
                            orc_thread_id,
                            orc_entry,
                            Some(ORC_ROLE),
                        );
                    }
                    for pane in claude_orcs {
                        self.write_spawn_context_claude_pane(
                            &mut context,
                            "  - ",
                            pane,
                            SpawnRole::Orc,
                        );
                    }
                }
            }
            for pane in claude_trolls {
                let troll_node_id = pane_node_id(&pane.id);
                self.write_spawn_context_claude_pane(&mut context, "- ", pane, SpawnRole::Troll);
                let (orcs, claude_orcs) = self.spawn_orc_children_for_node(&troll_node_id);
                if orcs.is_empty() && claude_orcs.is_empty() {
                    let _ = writeln!(context, "  Orcs under this Troll: none spawned yet.");
                } else {
                    for (orc_thread_id, orc_entry) in orcs {
                        self.write_spawn_context_agent(
                            &mut context,
                            "  - ",
                            orc_thread_id,
                            orc_entry,
                            Some(ORC_ROLE),
                        );
                    }
                    for pane in claude_orcs {
                        self.write_spawn_context_claude_pane(
                            &mut context,
                            "  - ",
                            pane,
                            SpawnRole::Orc,
                        );
                    }
                }
            }
        }
        let (orphan_orcs, claude_orcs) = self.unassigned_orc_nodes();
        if !orphan_orcs.is_empty() || !claude_orcs.is_empty() {
            let _ = writeln!(context, "Unassigned Orcs:");
            for (orc_thread_id, orc_entry) in orphan_orcs {
                self.write_spawn_context_agent(
                    &mut context,
                    "- ",
                    orc_thread_id,
                    orc_entry,
                    Some(ORC_ROLE),
                );
            }
            for pane in claude_orcs {
                self.write_spawn_context_claude_pane(&mut context, "- ", pane, SpawnRole::Orc);
            }
        }
        self.write_spawn_parent_reports(&mut context, &self.spawn_root_node_id());

        let _ = writeln!(
            context,
            "If no panes are listed for a role, say none are spawned yet and suggest using /spawn to create them."
        );
        let _ = writeln!(context, "</pfterminal_spawn_context>");
        context
    }

    fn write_spawn_context_agent(
        &self,
        context: &mut String,
        prefix: &str,
        thread_id: ThreadId,
        entry: &crate::multi_agents::AgentPickerThreadEntry,
        fallback_role: Option<&str>,
    ) {
        let name = format_agent_picker_item_name(
            entry.agent_nickname.as_deref(),
            entry.agent_role.as_deref().or(fallback_role),
            self.primary_thread_id == Some(thread_id),
        );
        let status = spawn_entry_status(self, thread_id, entry);
        let _ = writeln!(
            context,
            "{prefix}{name}; status={status}; thread={thread_id}"
        );
        if let Some(task) = entry
            .last_task_message
            .as_deref()
            .filter(|task| !task.trim().is_empty())
        {
            let _ = writeln!(
                context,
                "{prefix}  current_task={}",
                compact_spawn_context_value(task)
            );
        }
        if let Some(result) = entry
            .last_result_message
            .as_deref()
            .filter(|result| !result.trim().is_empty())
        {
            let _ = writeln!(
                context,
                "{prefix}  latest_result={}",
                compact_spawn_context_value(result)
            );
        }
    }

    fn write_spawn_context_claude_pane(
        &self,
        context: &mut String,
        prefix: &str,
        pane: &crate::claude_panes::ClaudePane,
        role: SpawnRole,
    ) {
        let status = match pane.status {
            crate::claude_panes::ClaudePaneStatus::Idle => "idle",
            crate::claude_panes::ClaudePaneStatus::Running => "running",
        };
        if let Some(thread_id) = pane.spawn_thread_id {
            let _ = writeln!(
                context,
                "{prefix}{}; role={}; harness=Claude Code; status={}; thread={}; pane={}",
                pane.title,
                role.label(),
                status,
                thread_id,
                pane.id
            );
        } else {
            let _ = writeln!(
                context,
                "{prefix}{}; role={}; harness=Claude Code; status={}; pane={}",
                pane.title,
                role.label(),
                status,
                pane.id
            );
        }
        if let Some(task) = pane.latest_task_message.as_deref() {
            let _ = writeln!(
                context,
                "{prefix}  current_task={}",
                compact_spawn_context_value(task)
            );
        }
        if let Some(result) = pane.latest_result_message.as_deref() {
            let _ = writeln!(
                context,
                "{prefix}  latest_result={}",
                compact_spawn_context_value(result)
            );
        }
    }

    fn write_spawn_parent_reports(&self, context: &mut String, parent_node_id: &str) {
        let Some(reports) = self.spawn_parent_reports_by_node.get(parent_node_id) else {
            return;
        };
        if reports.is_empty() {
            return;
        }
        let _ = writeln!(context, "Recent child reports delivered to this pane:");
        for report in reports.iter().rev().take(6).rev() {
            let _ = writeln!(context, "- {}", compact_spawn_context_value(report));
        }
    }
}

fn write_spawn_product_contract(context: &mut String) {
    let _ = writeln!(
        context,
        "Canonical PFTerminal positioning for orchestration work: PFTerminal is a terminal-native AI orchestration app for spawning, routing, supervising, and auditing agent panes."
    );
    let _ = writeln!(
        context,
        "Core concept: Sauron/the human is final authority; Nazgul orchestrates as CTO; Trolls supervise as engineering managers; Orcs execute as ICs."
    );
    let _ = writeln!(
        context,
        "Do not describe PFTerminal as a crypto/trading/Hyperliquid/GPU/staking/borrowing product unless Sauron explicitly asks for that legacy positioning."
    );
}

fn write_spawn_dispatch_contract(context: &mut String) {
    let _ = writeln!(
        context,
        "To send work to another spawn pane, emit a host dispatch block exactly like:"
    );
    let _ = writeln!(
        context,
        "<pfterminal_send_task target=\"Burzum\">\nTask text here.\n</pfterminal_send_task>"
    );
    let _ = writeln!(
        context,
        "PFTerminal will route that task to the target pane. Do not claim you sent a task unless you emit a dispatch block."
    );
    let _ = writeln!(
        context,
        "If Sauron asks you to dispatch, assign, send, or deploy work to a named pane, emit the pfterminal_send_task block immediately; do not start executing that work yourself, do not read skills first in the root pane, and do not replace dispatch with a plan."
    );
    let _ = writeln!(
        context,
        "Listed Troll/Orc panes are routable through these host dispatch blocks. Panes marked saved-only have recovered hierarchy metadata but no loaded transcript yet; dispatching a task to them materializes the pane."
    );
    let _ = writeln!(
        context,
        "Do not spawn fresh panes just to route work to existing listed panes; use the listed panes unless Sauron explicitly asks you to create more."
    );
    let _ = writeln!(
        context,
        "Dispatch blocks are plain assistant text, not Claude tools. Use only the pfterminal_send_task host tags; do not use <invoke>, <arg_key>, <arg_value>, or tool-call syntax for dispatch."
    );
    let _ = writeln!(
        context,
        "When assigning work to multiple panes, emit one complete pfterminal_send_task block per target in the same assistant message before saying the work was sent."
    );
    let _ = writeln!(
        context,
        "Do not wrap dispatch payloads in markdown fences; task bodies may contain code fences or long config snippets and must be preserved verbatim inside the host tags."
    );
    let _ = writeln!(
        context,
        "Use exact target names, nicknames, pane ids, or thread ids from this live hierarchy."
    );
}

fn spawn_parent_thread_for_new_agent(
    role: SpawnRole,
    active_claude_pane: bool,
    primary_thread_id: Option<ThreadId>,
    active_thread_id: Option<ThreadId>,
    active_thread_role: Option<&str>,
    troll_thread_ids: &[ThreadId],
) -> Option<ThreadId> {
    match role {
        SpawnRole::Nazgul => primary_thread_id.or(active_thread_id),
        SpawnRole::Troll => {
            if active_claude_pane {
                primary_thread_id
            } else {
                primary_thread_id.or(active_thread_id)
            }
        }
        SpawnRole::Orc => {
            if active_thread_role == Some(TROLL_ROLE) {
                return active_thread_id;
            }
            if let [single_troll] = troll_thread_ids {
                return Some(*single_troll);
            }
            None
        }
    }
}

fn native_spawn_thread_ids_from_saved_state(
    spawn_nazgul_pane_id: Option<&str>,
    spawn_parent_by_node: &HashMap<String, String>,
) -> Vec<ThreadId> {
    let mut thread_ids = Vec::new();
    let mut seen = HashSet::new();
    if let Some(pane_id) = spawn_nazgul_pane_id {
        push_native_spawn_thread_id(pane_id, &mut thread_ids, &mut seen);
    }
    for (child_node_id, parent_node_id) in spawn_parent_by_node {
        push_native_spawn_thread_id(child_node_id, &mut thread_ids, &mut seen);
        push_native_spawn_thread_id(parent_node_id, &mut thread_ids, &mut seen);
    }
    thread_ids.sort_by_key(std::string::ToString::to_string);
    thread_ids
}

fn saved_spawn_role_for_thread(
    thread_id: ThreadId,
    spawn_nazgul_pane_id: Option<&str>,
    spawn_parent_by_node: &HashMap<String, String>,
) -> Option<&'static str> {
    let node_id = thread_node_id(thread_id);
    if spawn_nazgul_pane_id == Some(node_id.as_str()) {
        return Some(NAZGUL_ROLE_NAME);
    }

    let parent_node_id = spawn_parent_by_node.get(&node_id)?;
    if spawn_nazgul_pane_id == Some(parent_node_id.as_str())
        || parent_node_id == &pane_node_id(CODEX_MAIN_PANE_ID)
    {
        return Some(TROLL_ROLE);
    }

    if saved_spawn_node_has_children(parent_node_id, spawn_parent_by_node) {
        return Some(ORC_ROLE);
    }

    None
}

fn saved_spawn_node_has_children(
    node_id: &str,
    spawn_parent_by_node: &HashMap<String, String>,
) -> bool {
    spawn_parent_by_node
        .values()
        .any(|parent_node_id| parent_node_id == node_id)
}

fn push_native_spawn_thread_id(
    node_id: &str,
    thread_ids: &mut Vec<ThreadId>,
    seen: &mut HashSet<ThreadId>,
) {
    if let Some(thread_id) = node_id_thread(node_id)
        && seen.insert(thread_id)
    {
        thread_ids.push(thread_id);
    }
}

fn thread_spawn_agent_path(source: &AppServerSessionSource) -> Option<String> {
    match source {
        AppServerSessionSource::SubAgent(SubAgentSource::ThreadSpawn { agent_path, .. }) => {
            agent_path.clone().map(String::from)
        }
        _ => None,
    }
}

fn saved_spawn_metadata_from_rollout_file(
    path: &Path,
) -> HashMap<ThreadId, SavedSpawnThreadMetadata> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    saved_spawn_metadata_from_jsonl(&contents)
}

fn saved_spawn_parent_edges_from_rollout_file(
    path: &Path,
    primary_thread_id: ThreadId,
) -> HashMap<ThreadId, ThreadId> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    saved_spawn_parent_edges_from_jsonl(&contents, primary_thread_id)
}

fn saved_spawn_metadata_from_jsonl(contents: &str) -> HashMap<ThreadId, SavedSpawnThreadMetadata> {
    let mut metadata = HashMap::new();
    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        collect_saved_spawn_metadata_from_json_value(&value, &mut metadata);
    }
    metadata
}

fn saved_spawn_parent_edges_from_jsonl(
    contents: &str,
    primary_thread_id: ThreadId,
) -> HashMap<ThreadId, ThreadId> {
    let mut edges = HashMap::new();
    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        collect_saved_spawn_parent_edges_from_json_value(&value, primary_thread_id, &mut edges);
    }
    edges
}

fn collect_saved_spawn_metadata_from_json_value(
    value: &serde_json::Value,
    metadata: &mut HashMap<ThreadId, SavedSpawnThreadMetadata>,
) {
    match value {
        serde_json::Value::String(text) => {
            if text.contains("<pfterminal_spawn_context>") || text.contains("; thread=") {
                merge_saved_spawn_metadata(metadata, saved_spawn_metadata_from_text(text));
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_saved_spawn_metadata_from_json_value(value, metadata);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_saved_spawn_metadata_from_json_value(value, metadata);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn collect_saved_spawn_parent_edges_from_json_value(
    value: &serde_json::Value,
    primary_thread_id: ThreadId,
    edges: &mut HashMap<ThreadId, ThreadId>,
) {
    match value {
        serde_json::Value::String(text) => {
            if text.contains("<pfterminal_spawn_context>") || text.contains("; thread=") {
                merge_saved_spawn_parent_edges(
                    edges,
                    saved_spawn_parent_edges_from_text(text, primary_thread_id),
                );
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_saved_spawn_parent_edges_from_json_value(value, primary_thread_id, edges);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_saved_spawn_parent_edges_from_json_value(value, primary_thread_id, edges);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn saved_spawn_metadata_from_text(text: &str) -> HashMap<ThreadId, SavedSpawnThreadMetadata> {
    let mut metadata = HashMap::new();
    for line in text.lines() {
        if let Some((thread_id, entry)) = saved_spawn_metadata_from_context_line(line) {
            merge_saved_spawn_metadata_entry(&mut metadata, thread_id, entry);
        }
    }
    metadata
}

fn saved_spawn_parent_edges_from_text(
    text: &str,
    primary_thread_id: ThreadId,
) -> HashMap<ThreadId, ThreadId> {
    let mut edges = HashMap::new();
    let mut current_troll_thread_id = None;
    for line in text.lines() {
        let Some((thread_id, metadata)) = saved_spawn_metadata_from_context_line(line) else {
            continue;
        };
        match metadata.role.as_deref() {
            Some(TROLL_ROLE) => {
                edges.insert(thread_id, primary_thread_id);
                current_troll_thread_id = Some(thread_id);
            }
            Some(ORC_ROLE) => {
                if let Some(troll_thread_id) = current_troll_thread_id {
                    edges.insert(thread_id, troll_thread_id);
                }
            }
            _ => {}
        }
    }
    edges
}

fn saved_spawn_metadata_from_context_line(
    line: &str,
) -> Option<(ThreadId, SavedSpawnThreadMetadata)> {
    let (prefix, tail) = line.split_once("; thread=")?;
    let thread_id_text = tail
        .split(|ch: char| ch.is_whitespace() || ch == ';')
        .next()?
        .trim();
    let thread_id = ThreadId::from_string(thread_id_text).ok()?;
    let label = prefix
        .split(';')
        .next()
        .unwrap_or(prefix)
        .trim()
        .trim_start_matches('-')
        .trim();
    let (nickname, role) = spawn_label_nickname_and_role(label);
    Some((thread_id, SavedSpawnThreadMetadata { nickname, role }))
}

fn spawn_label_nickname_and_role(label: &str) -> (Option<String>, Option<String>) {
    let Some((nickname, rest)) = label.rsplit_once(" [") else {
        return non_empty_string(label).map_or((None, None), |nickname| (Some(nickname), None));
    };
    let role = rest.split(']').next().and_then(non_empty_string);
    (non_empty_string(nickname), role)
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn merge_saved_spawn_metadata(
    target: &mut HashMap<ThreadId, SavedSpawnThreadMetadata>,
    source: HashMap<ThreadId, SavedSpawnThreadMetadata>,
) {
    for (thread_id, metadata) in source {
        merge_saved_spawn_metadata_entry(target, thread_id, metadata);
    }
}

fn merge_saved_spawn_metadata_entry(
    target: &mut HashMap<ThreadId, SavedSpawnThreadMetadata>,
    thread_id: ThreadId,
    metadata: SavedSpawnThreadMetadata,
) {
    let entry = target.entry(thread_id).or_default();
    if entry.nickname.is_none() {
        entry.nickname = metadata.nickname;
    }
    if entry.role.is_none() {
        entry.role = metadata.role;
    }
}

fn merge_saved_spawn_parent_edges(
    target: &mut HashMap<ThreadId, ThreadId>,
    source: HashMap<ThreadId, ThreadId>,
) {
    for (child_thread_id, parent_thread_id) in source {
        target.entry(child_thread_id).or_insert(parent_thread_id);
    }
}

fn merge_recovered_native_spawn_parent_edges(
    spawn_parent_by_node: &mut HashMap<String, String>,
    recovered_edges: HashMap<ThreadId, ThreadId>,
    recovered_metadata: &HashMap<ThreadId, SavedSpawnThreadMetadata>,
    existing_identities: HashSet<SavedSpawnChildIdentity>,
) {
    let mut known_identities = existing_identities;
    for (child_thread_id, parent_thread_id) in recovered_edges {
        let child_node_id = thread_node_id(child_thread_id);
        if spawn_parent_by_node.contains_key(&child_node_id) {
            continue;
        }
        let parent_node_id = thread_node_id(parent_thread_id);
        if let Some(identity) = saved_spawn_child_identity_from_metadata(
            &parent_node_id,
            recovered_metadata.get(&child_thread_id),
        ) && !known_identities.insert(identity)
        {
            continue;
        }
        spawn_parent_by_node.insert(child_node_id, parent_node_id);
    }
}

fn saved_spawn_child_identity_from_metadata(
    parent_node_id: &str,
    metadata: Option<&SavedSpawnThreadMetadata>,
) -> Option<SavedSpawnChildIdentity> {
    saved_spawn_child_identity(
        parent_node_id,
        metadata.and_then(|metadata| metadata.role.as_deref()),
        metadata.and_then(|metadata| metadata.nickname.as_deref()),
        None,
    )
}

fn saved_spawn_child_identity(
    parent_node_id: &str,
    role: Option<&str>,
    nickname: Option<&str>,
    agent_path: Option<&str>,
) -> Option<SavedSpawnChildIdentity> {
    let role = role.and_then(non_empty_string)?;
    let identity = nickname
        .and_then(non_empty_string)
        .or_else(|| agent_path.and_then(non_empty_string))?;
    Some(SavedSpawnChildIdentity {
        parent_node_id: parent_node_id.to_string(),
        role,
        identity,
    })
}

pub(crate) fn thread_node_id(thread_id: ThreadId) -> String {
    format!("thread:{thread_id}")
}

pub(crate) fn pane_node_id(pane_id: &str) -> String {
    format!("pane:{pane_id}")
}

pub(crate) fn node_id_thread(node_id: &str) -> Option<ThreadId> {
    node_id
        .strip_prefix("thread:")
        .and_then(|value| ThreadId::from_string(value).ok())
}

fn node_id_pane(node_id: &str) -> Option<&str> {
    node_id.strip_prefix("pane:")
}

fn is_nazgul_dispatch_target(target: &str) -> bool {
    target.eq_ignore_ascii_case("nazgul") || target.eq_ignore_ascii_case("root")
}

fn task_with_dispatch_provenance(task: &str, source_title: &str, target_title: &str) -> String {
    format!(
        "Assigned by {source_title} to {target_title} through PFTerminal /spawn dispatch.\n\n{task}"
    )
}

fn claude_spawn_rollout_path(pane: &crate::claude_panes::ClaudePane) -> std::path::PathBuf {
    pane.artifact_dir.join("spawn-thread-rollout.jsonl")
}

fn claude_spawn_rollout_turn_id(turn_index: u64) -> String {
    format!("claude-pane-turn-{turn_index:04}")
}

fn claude_spawn_event_timestamp() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn ensure_claude_spawn_rollout_session_meta(
    pane: &crate::claude_panes::ClaudePane,
    parent_thread_id: ThreadId,
    agent_role: &str,
) -> std::io::Result<()> {
    let Some(thread_id) = pane.spawn_thread_id else {
        return Ok(());
    };
    let path = claude_spawn_rollout_path(pane);
    if path.is_file() {
        return Ok(());
    }
    let source = CoreSessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth: 0,
        agent_path: None,
        agent_nickname: pane.spawn_nickname.clone(),
        agent_role: Some(agent_role.to_string()),
    });
    append_jsonl_value(
        &path,
        &serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": thread_id,
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "cwd": pane.cwd.display().to_string(),
                "originator": "pfterminal-claude-pane",
                "cli_version": env!("CARGO_PKG_VERSION"),
                "source": source,
                "thread_source": "subagent",
                "agent_nickname": pane.spawn_nickname.clone(),
                "agent_role": agent_role,
                "model_provider": "claude-code",
                "model": pane.profile.profile().provider_model,
                "base_instructions": null,
            },
        }),
    )
}

fn append_claude_spawn_rollout_task_started(
    pane: &crate::claude_panes::ClaudePane,
    turn_index: u64,
    task: &str,
) -> std::io::Result<()> {
    let path = claude_spawn_rollout_path(pane);
    let turn_id = claude_spawn_rollout_turn_id(turn_index);
    append_jsonl_value(
        &path,
        &serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "task_started",
                "turn_id": turn_id,
                "started_at": claude_spawn_event_timestamp(),
            },
        }),
    )?;
    append_jsonl_value(
        &path,
        &serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": task,
                "kind": "plain",
            },
        }),
    )
}

fn append_claude_spawn_rollout_task_completed(
    pane: &crate::claude_panes::ClaudePane,
    turn_index: u64,
    last_agent_message: &str,
) -> std::io::Result<()> {
    append_jsonl_value(
        &claude_spawn_rollout_path(pane),
        &serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "task_complete",
                "turn_id": claude_spawn_rollout_turn_id(turn_index),
                "completed_at": claude_spawn_event_timestamp(),
                "last_agent_message": last_agent_message,
            },
        }),
    )
}

fn append_jsonl_value(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = serde_json::to_string(value).map_err(std::io::Error::other)?;
    line.push('\n');
    file.write_all(line.as_bytes())
}

fn claude_turn_index_from_artifact_path(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("turn-")?
        .strip_suffix(".jsonl")
        .and_then(|value| value.parse::<u64>().ok())
}

pub(crate) fn extract_spawn_task_dispatches(text: &str) -> (String, Vec<SpawnTaskDispatch>) {
    let (visible, mut dispatches) = extract_fenced_spawn_task_dispatches(text);
    let (visible, legacy_dispatches) = extract_xmlish_spawn_task_dispatches(&visible);
    dispatches.extend(legacy_dispatches);
    (visible, dispatches)
}

fn extract_fenced_spawn_task_dispatches(text: &str) -> (String, Vec<SpawnTaskDispatch>) {
    let mut visible = String::new();
    let mut dispatches = Vec::new();
    let mut rest = text;

    while let Some(start_index) = rest.find(SEND_TASK_FENCE_OPEN) {
        visible.push_str(&rest[..start_index]);
        let block = &rest[start_index..];
        let Some(header_end) = block.find('\n') else {
            visible.push_str(block);
            rest = "";
            break;
        };
        let header = &block[..header_end];
        let content_start = header_end + 1;
        let Some(close_index) = block[content_start..].find(SEND_TASK_FENCE_CLOSE) else {
            visible.push_str(block);
            rest = "";
            break;
        };
        let content_end = content_start + close_index;
        let content = &block[content_start..content_end];
        let after_close = content_end + SEND_TASK_FENCE_CLOSE.len();

        if let Some(dispatch) = fenced_dispatch_from_parts(header, content) {
            dispatches.push(dispatch);
        }

        rest = &block[after_close..];
    }
    visible.push_str(rest);
    (visible.trim().to_string(), dispatches)
}

fn extract_xmlish_spawn_task_dispatches(text: &str) -> (String, Vec<SpawnTaskDispatch>) {
    let mut visible = String::new();
    let mut dispatches = Vec::new();
    let mut rest = text;

    while let Some(start_index) = rest.find(SEND_TASK_OPEN) {
        visible.push_str(&rest[..start_index]);
        let block = &rest[start_index..];
        let Some(tag_end) = block.find('>') else {
            visible.push_str(block);
            rest = "";
            break;
        };
        let tag = &block[..=tag_end];
        let content_start = tag_end + 1;
        let Some(close_index) = block[content_start..].find(SEND_TASK_CLOSE) else {
            visible.push_str(block);
            rest = "";
            break;
        };
        let content_end = content_start + close_index;
        let content = block[content_start..content_end].trim();
        let after_close = content_end + SEND_TASK_CLOSE.len();

        if let Some(target) = xmlish_attr_value(tag, "target")
            && !target.trim().is_empty()
            && !content.is_empty()
        {
            dispatches.push(SpawnTaskDispatch {
                target: target.trim().to_string(),
                task: content.to_string(),
            });
        }

        rest = &block[after_close..];
    }
    visible.push_str(rest);
    (visible.trim().to_string(), dispatches)
}

fn fenced_dispatch_from_parts(header: &str, content: &str) -> Option<SpawnTaskDispatch> {
    let mut target = yamlish_field_value(header, "target");
    let mut task_lines = Vec::new();
    let mut consumed_task_marker = false;

    for line in content.lines() {
        if target.is_none()
            && let Some(value) = yamlish_field_value(line, "target")
        {
            target = Some(value);
            continue;
        }

        if !consumed_task_marker && let Some(value) = yamlish_field_value(line, "task") {
            consumed_task_marker = true;
            if !value.trim().is_empty() {
                task_lines.push(value);
            }
            continue;
        }

        if task_lines.is_empty() && line.trim().is_empty() {
            continue;
        }
        task_lines.push(line.to_string());
    }

    let target = target?.trim().to_string();
    let task = task_lines.join("\n").trim().to_string();
    (!target.is_empty() && !task.is_empty()).then_some(SpawnTaskDispatch { target, task })
}

fn yamlish_field_value(line: &str, field: &str) -> Option<String> {
    let (key, value) = line.split_once(':')?;
    key.trim()
        .eq_ignore_ascii_case(field)
        .then(|| value.trim().to_string())
}

fn xmlish_attr_value(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=");
    let start = tag.find(&needle)? + needle.len();
    let mut chars = tag[start..].chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value_start = start + quote.len_utf8();
    let value_end = tag[value_start..].find(quote)? + value_start;
    Some(tag[value_start..value_end].to_string())
}

fn section_item(name: &str) -> SelectionItem {
    SelectionItem {
        name: name.to_string(),
        is_disabled: true,
        ..Default::default()
    }
}

fn provider_display_name(provider_id: &str, provider_name: &str) -> String {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() || provider_name == provider_id {
        provider_id.to_string()
    } else {
        format!("{provider_name} ({provider_id})")
    }
}

fn spawn_model_item(
    role: SpawnRole,
    parent_node_id: Option<String>,
    model: String,
    provider: Option<String>,
    effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    description: Option<String>,
    is_current: bool,
) -> SelectionItem {
    let effort_label = effort
        .as_ref()
        .map(|effort| effort.as_str().to_string())
        .unwrap_or_else(|| "default".to_string());
    SelectionItem {
        name: format!("Codex {}: {model} · {effort_label}", role.label()),
        description,
        search_value: Some(format!("codex {} {model} {effort_label}", role.label())),
        is_current,
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::CreateSpawnAgent {
                role,
                parent_node_id: parent_node_id.clone(),
                agent_nickname: None,
                model: model.clone(),
                provider: provider.clone(),
                effort: effort.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn spawn_reasoning_effort_for_role(_role: SpawnRole, preset: &ModelPreset) -> ReasoningEffort {
    if preset
        .supported_reasoning_efforts
        .iter()
        .any(|option| option.effort == ReasoningEffort::XHigh)
    {
        return ReasoningEffort::XHigh;
    }
    preset.default_reasoning_effort.clone()
}

fn disabled_item(name: &str) -> SelectionItem {
    SelectionItem {
        name: name.to_string(),
        is_disabled: true,
        ..Default::default()
    }
}

fn spawn_entry_status(
    app: &App,
    thread_id: ThreadId,
    entry: &crate::multi_agents::AgentPickerThreadEntry,
) -> &'static str {
    if app.unloaded_agent_thread_reason(thread_id).is_some() {
        "saved-only"
    } else if let Some(status) = app.spawn_status_by_thread.get(&thread_id) {
        spawn_status_label(status)
    } else if entry.is_closed {
        "done"
    } else if entry.is_running {
        "running"
    } else {
        "idle"
    }
}

fn compact_spawn_context_value(value: &str) -> String {
    const MAX_CHARS: usize = 220;
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= MAX_CHARS {
        return compact;
    }
    let mut truncated = compact
        .chars()
        .take(MAX_CHARS.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn spawn_agent_description(
    status: &str,
    thread_id: ThreadId,
    task: Option<&str>,
    result: Option<&str>,
) -> String {
    let mut parts = vec![status.to_string()];
    if let Some(task) = task.filter(|task| !task.trim().is_empty()) {
        parts.push(format!("current task: {task}"));
    }
    if let Some(result) = result.filter(|result| !result.trim().is_empty()) {
        parts.push(format!("latest result: {result}"));
    }
    if parts.len() == 1 {
        parts.push(thread_id.to_string());
    }
    parts.join("; ")
}

fn spawn_status_label(status: &codex_app_server_protocol::CollabAgentState) -> &'static str {
    collab_status_label(&status.status)
}

fn collab_status_label(status: &codex_app_server_protocol::CollabAgentStatus) -> &'static str {
    match *status {
        codex_app_server_protocol::CollabAgentStatus::PendingInit => "pending",
        codex_app_server_protocol::CollabAgentStatus::Running => "running",
        codex_app_server_protocol::CollabAgentStatus::Interrupted => "interrupted",
        codex_app_server_protocol::CollabAgentStatus::Completed => "done",
        codex_app_server_protocol::CollabAgentStatus::Errored => "error",
        codex_app_server_protocol::CollabAgentStatus::Shutdown => "closed",
        codex_app_server_protocol::CollabAgentStatus::NotFound => "not found",
    }
}

fn spawn_child_report(child_title: &str, status: &str, result: Option<&str>) -> String {
    let mut report = format!("{child_title}; status={status}");
    if let Some(result) = result.filter(|result| !result.trim().is_empty()) {
        let _ = write!(report, "; result={}", compact_spawn_context_value(result));
    }
    report
}

/// The prompt that turns a delivered child report into a real parent processing turn, so a manager
/// treats a direct report like a user query (triage, dispatch follow-up work, or acknowledge) rather
/// than letting it sit as a passive transcript line.
fn child_report_processing_prompt(report: &str) -> String {
    format!(
        "A child pane has reported back. Review the child report below and act on it immediately — triage, dispatch follow-up work, or acknowledge. Do not wait for Sauron to prompt you.\n\n{report}"
    )
}

fn evict_spawn_processed_dispatch_turns(
    processed_turns: &mut HashSet<(ThreadId, String)>,
    live_threads: &HashSet<ThreadId>,
    protected_turn: &(ThreadId, String),
) -> Option<(usize, usize)> {
    if processed_turns.len() <= SPAWN_PROCESSED_DISPATCH_TURN_LIMIT {
        return None;
    }

    let before_len = processed_turns.len();
    let protected_budget = usize::from(processed_turns.contains(protected_turn));
    let retain_budget = SPAWN_PROCESSED_DISPATCH_TURN_RETAIN.saturating_sub(protected_budget);
    let mut retained = 0usize;
    processed_turns.retain(|turn| {
        if turn == protected_turn {
            return true;
        }
        if retained >= retain_budget {
            return false;
        }
        if live_threads.contains(&turn.0) {
            retained += 1;
            return true;
        }
        false
    });
    Some((before_len, processed_turns.len()))
}

fn next_spawn_agent_nickname_from_used<'candidate, 'used>(
    candidates: impl IntoIterator<Item = &'candidate str>,
    used_nicknames: impl IntoIterator<Item = &'used str>,
) -> Option<String> {
    let candidates: Vec<&str> = candidates.into_iter().collect();
    let used_nicknames: HashSet<String> = used_nicknames.into_iter().map(str::to_string).collect();
    for reset_count in 0.. {
        for candidate in &candidates {
            let nickname = format_spawn_agent_nickname(candidate, reset_count);
            if !used_nicknames.contains(&nickname) {
                return Some(nickname);
            }
        }
    }
    None
}

fn format_spawn_agent_nickname(name: &str, nickname_reset_count: usize) -> String {
    match nickname_reset_count {
        0 => name.to_string(),
        reset_count => {
            let value = reset_count + 1;
            let suffix = match value % 100 {
                11..=13 => "th",
                _ => match value % 10 {
                    1 => "st", // codespell:ignore
                    2 => "nd", // codespell:ignore
                    3 => "rd", // codespell:ignore
                    _ => "th", // codespell:ignore
                },
            };
            format!("{name} the {value}{suffix}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_agent_nickname_uses_role_specific_pool() {
        let troll_candidates = ["Burzum", "Durbat"];
        let orc_candidates = ["Snaga", "Ghash"];
        assert_eq!(
            next_spawn_agent_nickname_from_used(troll_candidates, std::iter::empty()),
            Some("Burzum".to_string())
        );
        assert_eq!(
            next_spawn_agent_nickname_from_used(orc_candidates, std::iter::empty()),
            Some("Snaga".to_string())
        );
    }

    #[test]
    fn spawn_agent_nickname_skips_used_names_and_wraps_with_ordinal() {
        let candidates = ["Burzum", "Durbat"];
        let used_troll_names = ["Burzum", "Durbat", "Burzum the 2nd"];
        assert_eq!(
            next_spawn_agent_nickname_from_used(candidates, used_troll_names),
            Some("Durbat the 2nd".to_string())
        );
    }

    #[test]
    fn spawn_agent_description_includes_task_and_result_preview() {
        let thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000333").expect("valid id");

        assert_eq!(
            spawn_agent_description(
                "done",
                thread_id,
                Some("build animated proof website"),
                Some("created formula card and requested rework"),
            ),
            "done; current task: build animated proof website; latest result: created formula card and requested rework"
        );
    }

    #[test]
    fn orc_parent_prefers_single_troll_even_when_claude_pane_is_active() {
        let primary_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000444").expect("valid id");
        let troll_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000555").expect("valid id");

        assert_eq!(
            spawn_parent_thread_for_new_agent(
                SpawnRole::Orc,
                /*active_claude_pane*/ true,
                Some(primary_thread_id),
                Some(primary_thread_id),
                /*active_thread_role*/ None,
                &[troll_thread_id],
            ),
            Some(troll_thread_id)
        );
    }

    #[test]
    fn orc_parent_rejects_implicit_root_when_no_troll_exists() {
        let primary_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000445").expect("valid id");

        assert_eq!(
            spawn_parent_thread_for_new_agent(
                SpawnRole::Orc,
                /*active_claude_pane*/ false,
                Some(primary_thread_id),
                Some(primary_thread_id),
                /*active_thread_role*/ None,
                &[],
            ),
            None
        );
    }

    #[test]
    fn claude_pane_role_context_identifies_troll_and_orc() {
        let troll = SpawnRole::Troll
            .claude_pane_context()
            .expect("troll context");
        assert!(troll.contains("You are the PFTerminal Troll"));
        assert!(troll.contains("report to the Nazgul"));

        let orc = SpawnRole::Orc.claude_pane_context().expect("orc context");
        assert!(orc.contains("You are the PFTerminal Orc"));
        assert!(orc.contains("Do not spawn child agents"));

        assert!(SpawnRole::Nazgul.claude_pane_context().is_none());
    }

    #[test]
    fn extracts_spawn_task_dispatch_blocks_from_visible_text() {
        let text = r#"Please dispatch this.
```pfterminal-send-task
target: Burzum
task:
Review the hierarchy bridge and report concrete issues.
```
I queued the work."#;

        let (visible, dispatches) = extract_spawn_task_dispatches(text);

        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].target, "Burzum");
        assert_eq!(
            dispatches[0].task,
            "Review the hierarchy bridge and report concrete issues."
        );
        assert!(!visible.contains("pfterminal-send-task"));
        assert!(visible.contains("Please dispatch this."));
        assert!(visible.contains("I queued the work."));
    }

    #[test]
    fn extracts_legacy_xmlish_spawn_task_dispatch_blocks() {
        let text = r#"Please dispatch this.
<pfterminal_send_task target="Burzum">
Review the hierarchy bridge and report concrete issues.
</pfterminal_send_task>
I queued the work."#;

        let (visible, dispatches) = extract_spawn_task_dispatches(text);

        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].target, "Burzum");
        assert_eq!(
            dispatches[0].task,
            "Review the hierarchy bridge and report concrete issues."
        );
        assert!(!visible.contains("pfterminal_send_task"));
    }

    #[test]
    fn xmlish_spawn_task_dispatch_preserves_markdown_fenced_payloads() {
        let text = r#"Dispatching the full edict.
<pfterminal_send_task target="Burzum">
Burzum, full authority directive from Sauron via Nazgul.

Problem A: Systemd service files have no mempool submit flags.

```systemd
[Service]
ExecStart=/usr/local/bin/postfiat-validator rpc \
  --rpc-enable-submit \
  --rpc-enable-wrap-owned
```

Problem B: verify the WAN validators accept writes after redeploy.
</pfterminal_send_task>
Done."#;

        let (visible, dispatches) = extract_spawn_task_dispatches(text);

        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].target, "Burzum");
        assert!(dispatches[0].task.contains("full authority directive"));
        assert!(dispatches[0].task.contains("```systemd"));
        assert!(
            dispatches[0]
                .task
                .contains("ExecStart=/usr/local/bin/postfiat-validator rpc")
        );
        assert!(dispatches[0].task.contains("--rpc-enable-wrap-owned"));
        assert!(
            dispatches[0]
                .task
                .contains("Problem B: verify the WAN validators")
        );
        assert!(!visible.contains("ExecStart=/usr/local/bin/postfiat-validator rpc"));
        assert!(visible.contains("Dispatching the full edict."));
        assert!(visible.contains("Done."));
    }

    #[test]
    fn dispatch_contract_tells_claude_not_to_claim_without_block() {
        let mut context = String::new();
        write_spawn_dispatch_contract(&mut context);

        assert!(context.contains("<pfterminal_send_task target=\"Burzum\">"));
        assert!(context.contains("Do not claim you sent a task unless you emit a dispatch block"));
        assert!(context.contains("do not read skills first in the root pane"));
        assert!(context.contains("Dispatch blocks are plain assistant text"));
        assert!(context.contains("Listed Troll/Orc panes are routable"));
        assert!(context.contains("dispatching a task to them materializes the pane"));
        assert!(context.contains("Do not spawn fresh panes"));
        assert!(context.contains("tool-call syntax"));
        assert!(context.contains("<invoke>"));
        assert!(context.contains("one complete pfterminal_send_task block per target"));
        assert!(context.contains("Do not wrap dispatch payloads in markdown fences"));
    }

    #[test]
    fn processed_native_dispatch_turn_eviction_stays_bounded_and_allows_evicted_turn_as_new() {
        let live_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000601").expect("valid id");
        let stale_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000602").expect("valid id");
        let protected_turn = (live_thread_id, "turn-live".to_string());
        let evicted_turn = (stale_thread_id, "turn-0".to_string());
        let mut processed_turns = HashSet::new();
        let mut live_threads = HashSet::new();
        live_threads.insert(live_thread_id);

        processed_turns.insert(evicted_turn.clone());
        for index in 1..=SPAWN_PROCESSED_DISPATCH_TURN_LIMIT {
            processed_turns.insert((stale_thread_id, format!("turn-{index}")));
        }
        processed_turns.insert(protected_turn.clone());
        assert!(processed_turns.len() > SPAWN_PROCESSED_DISPATCH_TURN_LIMIT);

        let eviction = evict_spawn_processed_dispatch_turns(
            &mut processed_turns,
            &live_threads,
            &protected_turn,
        );

        assert_eq!(eviction, Some((SPAWN_PROCESSED_DISPATCH_TURN_LIMIT + 2, 1)));
        assert!(processed_turns.len() <= SPAWN_PROCESSED_DISPATCH_TURN_LIMIT);
        assert!(processed_turns.contains(&protected_turn));
        assert!(!processed_turns.contains(&evicted_turn));
        assert!(processed_turns.insert(evicted_turn));
        assert!(processed_turns.len() <= SPAWN_PROCESSED_DISPATCH_TURN_LIMIT);
    }

    #[test]
    fn nazgul_dispatch_target_aliases_resolve_to_root() {
        assert!(is_nazgul_dispatch_target("Nazgul"));
        assert!(is_nazgul_dispatch_target("nazgul"));
        assert!(is_nazgul_dispatch_target("root"));
        assert!(!is_nazgul_dispatch_target("Burzum"));
    }

    #[test]
    fn nazgul_role_has_agent_type_so_it_can_be_spawned_as_a_pane() {
        assert_eq!(SpawnRole::Nazgul.agent_type(), Some("nazgul"));
        assert_eq!(SpawnRole::Troll.agent_type(), Some("troll"));
        assert_eq!(SpawnRole::Orc.agent_type(), Some("orc"));
    }

    #[test]
    fn saved_spawn_layout_preserves_native_thread_ids_for_restore() {
        let nazgul_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000701").expect("valid id");
        let troll_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000702").expect("valid id");
        let first_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000703").expect("valid id");
        let second_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000704").expect("valid id");

        let mut spawn_parent_by_node = HashMap::new();
        spawn_parent_by_node.insert(
            thread_node_id(troll_thread_id),
            thread_node_id(nazgul_thread_id),
        );
        spawn_parent_by_node.insert(
            thread_node_id(first_orc_thread_id),
            thread_node_id(troll_thread_id),
        );
        spawn_parent_by_node.insert(
            thread_node_id(second_orc_thread_id),
            thread_node_id(troll_thread_id),
        );

        let nazgul_node_id = thread_node_id(nazgul_thread_id);
        let restored =
            native_spawn_thread_ids_from_saved_state(Some(&nazgul_node_id), &spawn_parent_by_node);

        assert_eq!(
            restored,
            vec![
                nazgul_thread_id,
                troll_thread_id,
                first_orc_thread_id,
                second_orc_thread_id
            ]
        );
        assert_eq!(
            saved_spawn_role_for_thread(
                nazgul_thread_id,
                Some(&nazgul_node_id),
                &spawn_parent_by_node
            ),
            Some(NAZGUL_ROLE_NAME)
        );
        assert_eq!(
            saved_spawn_role_for_thread(
                troll_thread_id,
                Some(&nazgul_node_id),
                &spawn_parent_by_node
            ),
            Some(TROLL_ROLE)
        );
        assert_eq!(
            saved_spawn_role_for_thread(
                first_orc_thread_id,
                Some(&nazgul_node_id),
                &spawn_parent_by_node
            ),
            Some(ORC_ROLE)
        );
        assert_eq!(
            saved_spawn_role_for_thread(
                second_orc_thread_id,
                Some(&nazgul_node_id),
                &spawn_parent_by_node
            ),
            Some(ORC_ROLE)
        );
    }

    #[test]
    fn saved_spawn_metadata_from_jsonl_recovers_spawn_context_labels() {
        let troll_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000711").expect("valid id");
        let first_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000712").expect("valid id");
        let second_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000713").expect("valid id");
        let context = format!(
            "<pfterminal_spawn_context>\nTrolls:\n- Burzum [troll]; status=idle; thread={troll_thread_id}\n  - Snaga [orc]; status=done; thread={first_orc_thread_id}\n  - Ghash [orc]; status=idle; thread={second_orc_thread_id}\n</pfterminal_spawn_context>"
        );
        let jsonl = format!(
            "{}\n",
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "content": [
                        {
                            "type": "input_text",
                            "text": context
                        }
                    ]
                }
            })
        );

        let metadata = saved_spawn_metadata_from_jsonl(&jsonl);

        assert_eq!(
            metadata.get(&troll_thread_id),
            Some(&SavedSpawnThreadMetadata {
                nickname: Some("Burzum".to_string()),
                role: Some(TROLL_ROLE.to_string()),
            })
        );
        assert_eq!(
            metadata.get(&first_orc_thread_id),
            Some(&SavedSpawnThreadMetadata {
                nickname: Some("Snaga".to_string()),
                role: Some(ORC_ROLE.to_string()),
            })
        );
        assert_eq!(
            metadata.get(&second_orc_thread_id),
            Some(&SavedSpawnThreadMetadata {
                nickname: Some("Ghash".to_string()),
                role: Some(ORC_ROLE.to_string()),
            })
        );
    }

    #[test]
    fn saved_spawn_parent_edges_from_jsonl_recovers_spawn_context_hierarchy() {
        let primary_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000720").expect("valid id");
        let troll_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000721").expect("valid id");
        let first_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000722").expect("valid id");
        let second_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000723").expect("valid id");
        let context = format!(
            "<pfterminal_spawn_context>\nTrolls:\n- Burzum [troll]; status=idle; thread={troll_thread_id}\n  - Snaga [orc]; status=done; thread={first_orc_thread_id}\n  - Ghash [orc]; status=idle; thread={second_orc_thread_id}\n</pfterminal_spawn_context>"
        );
        let jsonl = format!(
            "{}\n",
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "content": [
                        {
                            "type": "input_text",
                            "text": context
                        }
                    ]
                }
            })
        );

        let edges = saved_spawn_parent_edges_from_jsonl(&jsonl, primary_thread_id);

        assert_eq!(edges.get(&troll_thread_id), Some(&primary_thread_id));
        assert_eq!(edges.get(&first_orc_thread_id), Some(&troll_thread_id));
        assert_eq!(edges.get(&second_orc_thread_id), Some(&troll_thread_id));
    }

    #[test]
    fn recovered_spawn_parent_edges_fill_missing_children_without_overwriting_existing_edges() {
        let primary_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000730").expect("valid id");
        let troll_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000731").expect("valid id");
        let orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000732").expect("valid id");
        let existing_parent_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000733").expect("valid id");

        let mut spawn_parent_by_node = HashMap::from([(
            thread_node_id(troll_thread_id),
            thread_node_id(existing_parent_thread_id),
        )]);
        let recovered_edges = HashMap::from([
            (troll_thread_id, primary_thread_id),
            (orc_thread_id, troll_thread_id),
        ]);

        merge_recovered_native_spawn_parent_edges(
            &mut spawn_parent_by_node,
            recovered_edges,
            &HashMap::new(),
            HashSet::new(),
        );

        assert_eq!(
            spawn_parent_by_node.get(&thread_node_id(troll_thread_id)),
            Some(&thread_node_id(existing_parent_thread_id))
        );
        assert_eq!(
            spawn_parent_by_node.get(&thread_node_id(orc_thread_id)),
            Some(&thread_node_id(troll_thread_id))
        );
    }

    #[test]
    fn recovered_spawn_parent_edges_skip_duplicate_named_children() {
        let troll_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000741").expect("valid id");
        let current_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000742").expect("valid id");
        let stale_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000743").expect("valid id");
        let missing_orc_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000744").expect("valid id");
        let troll_node_id = thread_node_id(troll_thread_id);
        let mut spawn_parent_by_node =
            HashMap::from([(thread_node_id(current_orc_thread_id), troll_node_id.clone())]);
        let recovered_edges = HashMap::from([
            (stale_orc_thread_id, troll_thread_id),
            (missing_orc_thread_id, troll_thread_id),
        ]);
        let recovered_metadata = HashMap::from([
            (
                stale_orc_thread_id,
                SavedSpawnThreadMetadata {
                    nickname: Some("Snaga".to_string()),
                    role: Some(ORC_ROLE.to_string()),
                },
            ),
            (
                missing_orc_thread_id,
                SavedSpawnThreadMetadata {
                    nickname: Some("Ghash".to_string()),
                    role: Some(ORC_ROLE.to_string()),
                },
            ),
        ]);
        let existing_identities = HashSet::from([saved_spawn_child_identity(
            &troll_node_id,
            Some(ORC_ROLE),
            Some("Snaga"),
            None,
        )
        .expect("identity")]);

        merge_recovered_native_spawn_parent_edges(
            &mut spawn_parent_by_node,
            recovered_edges,
            &recovered_metadata,
            existing_identities,
        );

        assert!(!spawn_parent_by_node.contains_key(&thread_node_id(stale_orc_thread_id)));
        assert_eq!(
            spawn_parent_by_node.get(&thread_node_id(missing_orc_thread_id)),
            Some(&troll_node_id)
        );
    }

    #[test]
    fn spawn_prefers_xhigh_when_supported() {
        let mut preset = test_model_preset(
            ReasoningEffort::Medium,
            vec![ReasoningEffort::Medium, ReasoningEffort::XHigh],
        );

        assert_eq!(
            spawn_reasoning_effort_for_role(SpawnRole::Orc, &preset),
            ReasoningEffort::XHigh
        );
        assert_eq!(
            spawn_reasoning_effort_for_role(SpawnRole::Troll, &preset),
            ReasoningEffort::XHigh
        );

        preset.supported_reasoning_efforts =
            vec![codex_protocol::openai_models::ReasoningEffortPreset {
                effort: ReasoningEffort::Medium,
                description: "medium".to_string(),
            }];
        assert_eq!(
            spawn_reasoning_effort_for_role(SpawnRole::Orc, &preset),
            ReasoningEffort::Medium
        );
    }

    fn test_model_preset(
        default_reasoning_effort: ReasoningEffort,
        supported: Vec<ReasoningEffort>,
    ) -> ModelPreset {
        ModelPreset {
            id: "test-model".to_string(),
            model: "test-model".to_string(),
            display_name: "Test Model".to_string(),
            description: "test".to_string(),
            default_reasoning_effort,
            supported_reasoning_efforts: supported
                .into_iter()
                .map(
                    |effort| codex_protocol::openai_models::ReasoningEffortPreset {
                        effort,
                        description: "test effort".to_string(),
                    },
                )
                .collect(),
            supports_personality: false,
            additional_speed_tiers: Vec::new(),
            service_tiers: Vec::new(),
            default_service_tier: None,
            is_default: false,
            upgrade: None,
            show_in_picker: true,
            availability_nux: None,
            supported_in_api: true,
            input_modalities: codex_protocol::openai_models::default_input_modalities(),
        }
    }
}
