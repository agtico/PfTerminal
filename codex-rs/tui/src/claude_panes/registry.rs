//! Registry of Claude panes with layout persistence.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::spawn_orchestration::SpawnRole;
use codex_protocol::ThreadId;

use super::command_plan::build_claude_command_plan;
use super::command_plan::claude_pane_title;
use super::command_plan::ensure_vault_label_exists;
use super::pane::ClaudePane;
use super::pane::ClaudePaneLiveStatus;
use super::pane::ClaudePaneLiveTurn;
use super::pane::ClaudePaneStatus;
use super::pane::ClaudePaneTurnStatus;
use super::pane::PaneLayoutState;
use super::persistence::persist_claude_pane_metadata;
use super::persistence::restore_claude_panes_from_disk;
use super::progress_summarize::compact_claude_pane_metadata;
use super::provider::ClaudeProviderProfileKind;
use super::turn_types::ClaudePaneTurnOutput;
use super::turn_types::ClaudePaneTurnProgress;
use super::turn_types::PreparedClaudePaneTurn;

pub(crate) const CODEX_MAIN_PANE_ID: &str = "codex-main";
pub(crate) const PANE_LAYOUT_FILE: &str = "pane-layout.json";
pub(crate) const PANE_LAYOUT_VERSION: u32 = 1;
const PANE_LAYOUTS_DIR: &str = "pane-layouts";
#[derive(Debug)]
pub(crate) struct ClaudePaneRegistry {
    active_user_pane_id: String,
    pub(crate) panes: Vec<ClaudePane>,
}

impl ClaudePaneRegistry {
    pub(crate) fn new() -> Self {
        Self {
            active_user_pane_id: CODEX_MAIN_PANE_ID.to_string(),
            panes: Vec::new(),
        }
    }

    pub(crate) fn restore_from_disk(codex_home: &Path, layout: Option<&PaneLayoutState>) -> Self {
        let mut restored = restore_claude_panes_from_disk(codex_home, layout);
        restored.sort_by(|left, right| {
            left.sort_key_ms
                .cmp(&right.sort_key_ms)
                .then_with(|| left.pane.title.cmp(&right.pane.title))
        });
        let panes: Vec<ClaudePane> = restored.into_iter().map(|restored| restored.pane).collect();
        let active_user_pane_id = layout
            .and_then(|layout| layout.active_user_pane_id.as_deref())
            .filter(|pane_id| {
                *pane_id == CODEX_MAIN_PANE_ID
                    || panes.iter().any(|pane: &ClaudePane| pane.id == *pane_id)
            })
            .unwrap_or(CODEX_MAIN_PANE_ID)
            .to_string();
        Self {
            active_user_pane_id,
            panes,
        }
    }

    pub(crate) fn active_user_pane_id(&self) -> &str {
        &self.active_user_pane_id
    }

    pub(crate) fn active_claude_pane_id(&self) -> Option<&str> {
        (self.active_user_pane_id != CODEX_MAIN_PANE_ID)
            .then_some(self.active_user_pane_id.as_str())
    }

    pub(crate) fn panes(&self) -> &[ClaudePane] {
        &self.panes
    }

    pub(crate) fn active_claude_pane_title(&self) -> Option<&str> {
        let pane_id = self.active_claude_pane_id()?;
        self.panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .map(|pane| pane.title.as_str())
    }

    pub(crate) fn active_claude_pane_model_label(&self) -> Option<String> {
        let pane_id = self.active_claude_pane_id()?;
        self.panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .map(|pane| pane.profile.status_model_label())
    }

    pub(crate) fn claude_pane_spawn_role(&self, pane_id: &str) -> Option<SpawnRole> {
        self.panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .and_then(|pane| pane.spawn_role)
    }

    pub(crate) fn claude_pane_is_running(&self, pane_id: &str) -> bool {
        self.panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .is_some_and(|pane| pane.status == ClaudePaneStatus::Running)
    }

    pub(crate) fn live_status_for_pane(&self, pane_id: &str) -> Option<ClaudePaneLiveStatus> {
        self.panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .and_then(|pane| pane.live_turn.as_ref())
            .map(ClaudePaneLiveTurn::display)
    }

    pub(crate) fn set_active_user_pane(&mut self, pane_id: &str) -> Result<()> {
        if pane_id == CODEX_MAIN_PANE_ID {
            self.active_user_pane_id = CODEX_MAIN_PANE_ID.to_string();
            return Ok(());
        }
        if self.panes.iter().any(|pane| pane.id == pane_id) {
            self.active_user_pane_id = pane_id.to_string();
            Ok(())
        } else {
            Err(anyhow!("Claude pane `{pane_id}` does not exist"))
        }
    }

    pub(crate) fn create_pane(
        &mut self,
        profile: ClaudeProviderProfileKind,
        cwd: PathBuf,
        codex_home: &Path,
    ) -> Result<String> {
        self.create_pane_with_role(profile, cwd, codex_home, None, None)
    }

    pub(crate) fn create_pane_with_role(
        &mut self,
        profile: ClaudeProviderProfileKind,
        cwd: PathBuf,
        codex_home: &Path,
        spawn_role: Option<SpawnRole>,
        spawn_nickname: Option<String>,
    ) -> Result<String> {
        let profile_config = profile.profile();
        if let Some(label) = profile_config.vault_label {
            ensure_vault_label_exists(codex_home, label)?;
        }
        self.push_pane(profile, cwd, codex_home, spawn_role, spawn_nickname)
    }

    #[cfg(test)]
    pub(crate) fn create_pane_without_vault_for_test(
        &mut self,
        profile: ClaudeProviderProfileKind,
        cwd: PathBuf,
        codex_home: &Path,
    ) -> Result<String> {
        self.push_pane(profile, cwd, codex_home, None, None)
    }

    fn push_pane(
        &mut self,
        profile: ClaudeProviderProfileKind,
        cwd: PathBuf,
        codex_home: &Path,
        spawn_role: Option<SpawnRole>,
        spawn_nickname: Option<String>,
    ) -> Result<String> {
        let id = format!("claude-{}", Uuid::new_v4());
        let artifact_dir = codex_home.join("panes").join(&id);
        std::fs::create_dir_all(&artifact_dir).with_context(|| {
            format!(
                "failed to create Claude pane artifact directory `{}`",
                artifact_dir.display()
            )
        })?;
        let pane = ClaudePane {
            id: id.clone(),
            title: claude_pane_title(profile, spawn_role, spawn_nickname.as_deref()),
            profile,
            spawn_role,
            spawn_nickname,
            spawn_thread_id: spawn_role.map(|_| ThreadId::new()),
            cwd,
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
        };
        persist_claude_pane_metadata(&pane)?;
        self.panes.push(pane);
        // Spawned workers (panes created with a spawn role) must not steal the
        // operator's control surface: only user-created panes become active.
        if spawn_role.is_none() {
            self.active_user_pane_id = id.clone();
        }
        Ok(id)
    }

    pub(crate) fn claude_pane_spawn_thread_id(&self, pane_id: &str) -> Option<ThreadId> {
        self.panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .and_then(|pane| pane.spawn_thread_id)
    }

    pub(crate) fn prepare_turn(
        &mut self,
        pane_id: &str,
        prompt: String,
        codex_home: &Path,
    ) -> Result<PreparedClaudePaneTurn> {
        let pane = self
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            .ok_or_else(|| anyhow!("Claude pane `{pane_id}` does not exist"))?;
        if pane.status == ClaudePaneStatus::Running {
            return Err(anyhow!("Claude pane `{}` is already running", pane.title));
        }
        let lock = pane
            .lock
            .clone()
            .try_lock_owned()
            .map_err(|_| anyhow!("Claude pane `{}` is already running", pane.title))?;

        let plan = build_claude_command_plan(pane, prompt, codex_home)?;
        let cancel_token = CancellationToken::new();
        pane.status = ClaudePaneStatus::Running;
        pane.live_turn = Some(ClaudePaneLiveTurn::starting());
        pane.cancel_token = Some(cancel_token.clone());
        Ok(PreparedClaudePaneTurn {
            pane_id: pane.id.clone(),
            plan,
            cancel_token,
            _lock: lock,
        })
    }

    pub(crate) fn interrupt_turn(&mut self, pane_id: &str) -> Result<()> {
        let pane = self
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            .ok_or_else(|| anyhow!("Claude pane `{pane_id}` does not exist"))?;
        if pane.status != ClaudePaneStatus::Running {
            return Err(anyhow!("Claude pane `{}` is not running", pane.title));
        }
        let Some(cancel_token) = pane.cancel_token.as_ref() else {
            return Err(anyhow!(
                "Claude pane `{}` has no cancellable turn",
                pane.title
            ));
        };
        cancel_token.cancel();
        if let Some(live_turn) = pane.live_turn.as_mut() {
            live_turn.phase = "interrupted".to_string();
            live_turn.current = "interrupting Claude".to_string();
        }
        Ok(())
    }

    pub(crate) fn finish_turn(
        &mut self,
        pane_id: &str,
        result: &Result<ClaudePaneTurnOutput, String>,
    ) {
        let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == pane_id) else {
            return;
        };
        pane.status = ClaudePaneStatus::Idle;
        pane.live_turn = None;
        pane.cancel_token = None;
        if let Ok(output) = result {
            match output.status {
                ClaudePaneTurnStatus::Success
                | ClaudePaneTurnStatus::MaxTurnsPause
                | ClaudePaneTurnStatus::TimeoutPause
                | ClaudePaneTurnStatus::Interrupted => {
                    if let Some(session_id) = &output.session_id {
                        pane.claude_session_id = Some(session_id.clone());
                    }
                }
                ClaudePaneTurnStatus::ProviderError | ClaudePaneTurnStatus::ParseFailure => {
                    pane.claude_session_id = None;
                }
            }
            pane.latest_usage_summary = output.usage_summary.clone();
            pane.latest_usage_status = Some(output.usage_status);
            pane.latest_turn_status = Some(output.status);
            pane.latest_audit_path = Some(output.audit_path.clone());
            if !output.text.trim().is_empty() {
                pane.latest_result_message = Some(compact_claude_pane_metadata(&output.text, 240));
            }
            pane.next_turn_index = pane.next_turn_index.saturating_add(1);
            if let Err(err) = persist_claude_pane_metadata(pane) {
                tracing::warn!(pane_id = %pane.id, error = %err, "failed to persist Claude pane metadata");
            }
        }
    }

    pub(crate) fn set_latest_task_message(&mut self, pane_id: &str, task: Option<String>) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == pane_id) {
            pane.latest_task_message = task.map(|task| compact_claude_pane_metadata(&task, 240));
            if let Err(err) = persist_claude_pane_metadata(pane) {
                tracing::warn!(pane_id = %pane.id, error = %err, "failed to persist Claude pane task metadata");
            }
        }
    }

    pub(crate) fn update_live_progress(
        &mut self,
        progress: &ClaudePaneTurnProgress,
    ) -> Option<ClaudePaneLiveStatus> {
        let pane = self
            .panes
            .iter_mut()
            .find(|pane| pane.id == progress.pane_id)?;
        let live_turn = pane
            .live_turn
            .get_or_insert_with(ClaudePaneLiveTurn::starting);
        live_turn.update(progress);
        Some(live_turn.display())
    }

    pub(crate) fn take_visible_assistant_transcript_delta(
        &mut self,
        pane_id: &str,
    ) -> Option<String> {
        self.panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)?
            .live_turn
            .as_mut()?
            .take_visible_assistant_transcript_delta()
    }

    pub(crate) fn take_final_visible_assistant_transcript_delta(
        &mut self,
        pane_id: &str,
        final_visible_text: &str,
    ) -> Option<String> {
        self.panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)?
            .live_turn
            .as_mut()?
            .take_final_visible_assistant_transcript_delta(final_visible_text)
    }

    pub(crate) fn has_emitted_visible_assistant_transcript(&self, pane_id: &str) -> bool {
        self.panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .and_then(|pane| pane.live_turn.as_ref())
            .is_some_and(ClaudePaneLiveTurn::has_emitted_visible_assistant_transcript)
    }

    pub(crate) fn collect_spawn_dispatches_from_assistant_delta(
        &mut self,
        pane_id: &str,
        delta: &str,
    ) -> Vec<crate::spawn_orchestration::SpawnTaskDispatch> {
        let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == pane_id) else {
            return Vec::new();
        };
        let live_turn = pane
            .live_turn
            .get_or_insert_with(ClaudePaneLiveTurn::starting);
        live_turn.assistant_dispatch_buffer.push_str(delta);
        let (_, dispatches) = crate::spawn_orchestration::extract_spawn_task_dispatches(
            &live_turn.assistant_dispatch_buffer,
        );
        live_turn.filter_new_dispatches(dispatches)
    }

    pub(crate) fn filter_new_spawn_dispatches(
        &mut self,
        pane_id: &str,
        dispatches: Vec<crate::spawn_orchestration::SpawnTaskDispatch>,
    ) -> Vec<crate::spawn_orchestration::SpawnTaskDispatch> {
        let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == pane_id) else {
            return dispatches;
        };
        let Some(live_turn) = pane.live_turn.as_mut() else {
            return dispatches;
        };
        live_turn.filter_new_dispatches(dispatches)
    }
}

impl Default for ClaudePaneRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn load_pane_layout(
    codex_home: &Path,
    codex_thread_id: Option<&str>,
) -> Option<PaneLayoutState> {
    let thread_id = codex_thread_id?;
    let thread_scoped_path = thread_scoped_pane_layout_path(codex_home, thread_id);
    let thread_scoped_layout = read_pane_layout(&thread_scoped_path)
        .filter(|layout| layout.codex_thread_id.as_deref() == Some(thread_id));
    if thread_scoped_layout
        .as_ref()
        .is_some_and(pane_layout_has_panes)
    {
        return thread_scoped_layout;
    }
    if let Some(layout) = find_related_pane_layout(codex_home, thread_id) {
        return Some(layout);
    }

    let legacy_path = codex_home.join("panes").join(PANE_LAYOUT_FILE);
    let legacy_layout = read_pane_layout(&legacy_path)
        .filter(|layout| layout.codex_thread_id.as_deref() == Some(thread_id));
    thread_scoped_layout.or(legacy_layout)
}

fn find_related_pane_layout(codex_home: &Path, thread_id: &str) -> Option<PaneLayoutState> {
    let layout_dir = codex_home.join("panes").join(PANE_LAYOUTS_DIR);
    let mut best: Option<(u128, PaneLayoutState)> = None;
    let thread_node_id = format!("thread:{thread_id}");
    for entry in fs::read_dir(layout_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(layout) = read_pane_layout(&path) else {
            continue;
        };
        if !pane_layout_has_panes(&layout) || !pane_layout_mentions_thread(&layout, &thread_node_id)
        {
            continue;
        }
        let modified_ms = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        if best
            .as_ref()
            .is_none_or(|(best_modified_ms, _)| modified_ms > *best_modified_ms)
        {
            best = Some((modified_ms, layout));
        }
    }
    best.map(|(_, layout)| layout)
}

fn pane_layout_has_panes(layout: &PaneLayoutState) -> bool {
    layout.spawn_nazgul_pane_id.is_some()
        || !layout.claude_pane_ids.is_empty()
        || !layout.spawn_parent_by_node.is_empty()
}

fn pane_layout_mentions_thread(layout: &PaneLayoutState, thread_node_id: &str) -> bool {
    layout.spawn_nazgul_pane_id.as_deref() == Some(thread_node_id)
        || layout
            .spawn_parent_by_node
            .iter()
            .any(|(child, parent)| child == thread_node_id || parent == thread_node_id)
}

fn read_pane_layout(path: &Path) -> Option<PaneLayoutState> {
    let contents = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<PaneLayoutState>(&contents) {
        Ok(layout) => Some(layout),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "failed to load pane layout");
            None
        }
    }
}

pub(crate) fn persist_pane_layout(codex_home: &Path, layout: &PaneLayoutState) -> Result<()> {
    let panes_dir = codex_home.join("panes");
    fs::create_dir_all(&panes_dir).with_context(|| {
        format!(
            "failed to create pane layout directory `{}`",
            panes_dir.display()
        )
    })?;
    let mut layout = layout.clone();
    layout.version = PANE_LAYOUT_VERSION;
    let contents = serde_json::to_string_pretty(&layout)
        .context("failed to serialize pane layout metadata")?;
    if let Some(thread_id) = layout.codex_thread_id.as_deref() {
        let thread_scoped_path = thread_scoped_pane_layout_path(codex_home, thread_id);
        if let Some(parent) = thread_scoped_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create pane layout directory `{}`",
                    parent.display()
                )
            })?;
        }
        fs::write(&thread_scoped_path, &contents).with_context(|| {
            format!(
                "failed to write pane layout `{}`",
                thread_scoped_path.display()
            )
        })?;
    }
    let legacy_path = panes_dir.join(PANE_LAYOUT_FILE);
    fs::write(&legacy_path, contents)
        .with_context(|| format!("failed to write pane layout `{}`", legacy_path.display()))
}

fn thread_scoped_pane_layout_path(codex_home: &Path, thread_id: &str) -> PathBuf {
    codex_home
        .join("panes")
        .join(PANE_LAYOUTS_DIR)
        .join(format!("{thread_id}.json"))
}
