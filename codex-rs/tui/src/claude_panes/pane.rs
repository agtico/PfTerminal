//! Core pane types: status enums, pane struct, live turn tracking.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::spawn_orchestration::SpawnRole;
use codex_protocol::ThreadId;

use super::progress::format_elapsed_ms;
use super::progress::progress_status_text;
use super::progress::reasoning_blurb_from_progress;
use super::progress::tool_blurb_from_progress;
use super::progress_summarize::ASSISTANT_UPDATE_VISIBLE_COUNT;
use super::progress_summarize::REASONING_VISIBLE_COUNT;
use super::progress_summarize::TOOL_VISIBLE_COUNT;
use super::progress_summarize::assistant_update_blurbs_from_buffer;
use super::progress_summarize::visible_assistant_text_from_buffer;
use super::provider::ClaudeProviderProfileKind;
use super::turn_types::ClaudePaneTurnProgress;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaudePaneStatus {
    Idle,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudePaneTurnStatus {
    Success,
    MaxTurnsPause,
    TimeoutPause,
    Interrupted,
    ProviderError,
    ParseFailure,
}

impl ClaudePaneTurnStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::MaxTurnsPause => "max-turn-pause",
            Self::TimeoutPause => "timeout-pause",
            Self::Interrupted => "interrupted",
            Self::ProviderError => "provider-error",
            Self::ParseFailure => "parse-failure",
        }
    }

    pub(crate) fn is_success(self) -> bool {
        self == Self::Success
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudePaneUsageStatus {
    Reported,
    Missing,
    Unknown,
    Untrusted,
}

impl ClaudePaneUsageStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Missing => "missing",
            Self::Unknown => "unknown",
            Self::Untrusted => "untrusted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeCommandMode {
    NewSession,
    Resume,
}

impl ClaudeCommandMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NewSession => "session-id",
            Self::Resume => "resume",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ClaudePane {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) profile: ClaudeProviderProfileKind,
    pub(crate) spawn_role: Option<SpawnRole>,
    pub(crate) spawn_nickname: Option<String>,
    pub(crate) spawn_thread_id: Option<ThreadId>,
    pub(crate) cwd: PathBuf,
    pub(crate) claude_session_id: Option<String>,
    pub(crate) status: ClaudePaneStatus,
    pub(crate) latest_usage_summary: Option<String>,
    pub(crate) latest_usage_status: Option<ClaudePaneUsageStatus>,
    pub(crate) latest_turn_status: Option<ClaudePaneTurnStatus>,
    pub(crate) latest_audit_path: Option<PathBuf>,
    pub(crate) latest_task_message: Option<String>,
    pub(crate) latest_result_message: Option<String>,
    pub(crate) artifact_dir: PathBuf,
    pub(crate) live_turn: Option<ClaudePaneLiveTurn>,
    pub(crate) cancel_token: Option<CancellationToken>,
    pub(crate) lock: Arc<Mutex<()>>,
    pub(crate) next_turn_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedClaudePaneMetadata {
    pub(crate) version: u32,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) profile: ClaudeProviderProfileKind,
    pub(crate) spawn_role: Option<String>,
    pub(crate) spawn_nickname: Option<String>,
    pub(crate) spawn_thread_id: Option<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) claude_session_id: Option<String>,
    pub(crate) latest_usage_summary: Option<String>,
    pub(crate) latest_usage_status: Option<ClaudePaneUsageStatus>,
    pub(crate) latest_turn_status: Option<ClaudePaneTurnStatus>,
    pub(crate) latest_audit_path: Option<PathBuf>,
    pub(crate) latest_task_message: Option<String>,
    pub(crate) latest_result_message: Option<String>,
    pub(crate) next_turn_index: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct RestoredClaudePane {
    pub(crate) pane: ClaudePane,
    pub(crate) sort_key_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PaneLayoutState {
    pub(crate) version: u32,
    pub(crate) codex_thread_id: Option<String>,
    pub(crate) active_user_pane_id: Option<String>,
    pub(crate) spawn_nazgul_pane_id: Option<String>,
    pub(crate) claude_pane_ids: Vec<String>,
    pub(crate) spawn_parent_by_node: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClaudePaneLiveTurn {
    pub(crate) elapsed_ms: i64,
    pub(crate) current: String,
    pub(crate) phase: String,
    pub(crate) thinking_tokens: Option<String>,
    pub(crate) assistant_commentary_buffer: String,
    pub(crate) assistant_transcript_emitted: String,
    pub(crate) assistant_blurbs: Vec<String>,
    pub(crate) reasoning_blurbs: Vec<String>,
    pub(crate) tool_blurbs: Vec<String>,
    pub(crate) assistant_dispatch_buffer: String,
    pub(crate) sent_dispatch_keys: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudePaneLiveStatus {
    pub(crate) header: String,
    pub(crate) details: Option<String>,
}

impl ClaudePaneLiveTurn {
    pub(crate) fn starting() -> Self {
        Self {
            elapsed_ms: 0,
            current: "starting Claude".to_string(),
            phase: "starting".to_string(),
            thinking_tokens: None,
            assistant_commentary_buffer: String::new(),
            assistant_transcript_emitted: String::new(),
            assistant_blurbs: Vec::new(),
            reasoning_blurbs: Vec::new(),
            tool_blurbs: Vec::new(),
            assistant_dispatch_buffer: String::new(),
            sent_dispatch_keys: HashSet::new(),
        }
    }

    pub(crate) fn update(&mut self, progress: &ClaudePaneTurnProgress) {
        self.elapsed_ms = progress.elapsed_ms;
        self.phase = progress.phase.clone();
        match progress.phase.as_str() {
            "assistant-text" => {
                if let Some(delta) = progress.assistant_text_delta.as_deref() {
                    self.assistant_commentary_buffer.push_str(delta);
                    self.assistant_blurbs =
                        assistant_update_blurbs_from_buffer(&self.assistant_commentary_buffer);
                }
                if let Some(update) = self.assistant_blurbs.last() {
                    self.current = format!("Claude note: {update}");
                } else if self.current.trim().is_empty() {
                    self.current = "Claude is responding".to_string();
                }
            }
            "tool-call" => {
                let tool = tool_blurb_from_progress(progress);
                self.current = tool.clone();
                if self.tool_blurbs.last() != Some(&tool) {
                    self.tool_blurbs.push(tool);
                }
            }
            "reasoning" => {
                let reasoning = reasoning_blurb_from_progress(progress);
                self.current = format!("thinking: {reasoning}");
                if self.reasoning_blurbs.last() != Some(&reasoning) {
                    self.reasoning_blurbs.push(reasoning);
                }
            }
            "reasoning-tokens" => {
                let reasoning = reasoning_blurb_from_progress(progress);
                self.current = reasoning.clone();
                self.thinking_tokens = Some(reasoning);
            }
            "waiting" => {
                // Keep the last tool visible during Claude-side thinking so heartbeat ticks update
                // elapsed time without flickering the panel back to a generic waiting label.
                if self.current.trim().is_empty() {
                    self.current = "waiting for Claude".to_string();
                }
            }
            "assistant-result" => {
                self.current = "finalizing result".to_string();
            }
            "system" => {
                self.current = progress_status_text(progress);
            }
            "error" => {
                self.current = progress_status_text(progress);
            }
            _ => {
                self.current = progress_status_text(progress);
            }
        }
    }

    pub(crate) fn display(&self) -> ClaudePaneLiveStatus {
        let header = format!("Claude running · {}", format_elapsed_ms(self.elapsed_ms));
        let mut lines = vec![format!("Current: {}", self.current)];
        if !self.assistant_blurbs.is_empty() {
            lines.push("Claude notes:".to_string());
            let hidden = self
                .assistant_blurbs
                .len()
                .saturating_sub(ASSISTANT_UPDATE_VISIBLE_COUNT);
            if hidden > 0 {
                lines.push(format!("  ... {hidden} earlier notes hidden"));
            }
            let visible_start = self
                .assistant_blurbs
                .len()
                .saturating_sub(ASSISTANT_UPDATE_VISIBLE_COUNT);
            for update in self.assistant_blurbs.iter().skip(visible_start) {
                lines.push(format!("  {update}"));
            }
        }
        if self.thinking_tokens.is_some() || !self.reasoning_blurbs.is_empty() {
            lines.push("Thinking:".to_string());
            if let Some(thinking_tokens) = &self.thinking_tokens {
                lines.push(format!("  {thinking_tokens}"));
            }
            let hidden = self
                .reasoning_blurbs
                .len()
                .saturating_sub(REASONING_VISIBLE_COUNT);
            if hidden > 0 {
                lines.push(format!("  ... {hidden} earlier thoughts hidden"));
            }
            let visible_start = self
                .reasoning_blurbs
                .len()
                .saturating_sub(REASONING_VISIBLE_COUNT);
            for reasoning in self.reasoning_blurbs.iter().skip(visible_start) {
                lines.push(format!("  {reasoning}"));
            }
        }
        if !self.tool_blurbs.is_empty() {
            lines.push("Tools:".to_string());
            let hidden = self.tool_blurbs.len().saturating_sub(TOOL_VISIBLE_COUNT);
            if hidden > 0 {
                lines.push(format!("  +{hidden} earlier"));
            }
            let visible_start = self.tool_blurbs.len().saturating_sub(TOOL_VISIBLE_COUNT);
            let all_done = matches!(
                self.phase.as_str(),
                "assistant-result" | "assistant-text" | "error" | "reasoning" | "reasoning-tokens"
            );
            for (index, tool) in self.tool_blurbs.iter().enumerate().skip(visible_start) {
                let is_last = index + 1 == self.tool_blurbs.len();
                let state = if is_last && !all_done {
                    "running"
                } else {
                    "done"
                };
                lines.push(format!("  {state:<7} {tool}"));
            }
        }
        ClaudePaneLiveStatus {
            header,
            details: Some(lines.join("\n")),
        }
    }

    pub(crate) fn filter_new_dispatches(
        &mut self,
        dispatches: Vec<crate::spawn_orchestration::SpawnTaskDispatch>,
    ) -> Vec<crate::spawn_orchestration::SpawnTaskDispatch> {
        dispatches
            .into_iter()
            .filter(|dispatch| self.sent_dispatch_keys.insert(spawn_dispatch_key(dispatch)))
            .collect()
    }

    pub(crate) fn take_visible_assistant_transcript_delta(&mut self) -> Option<String> {
        let visible = visible_assistant_text_from_buffer(&self.assistant_commentary_buffer);
        self.take_visible_assistant_transcript_delta_from(visible)
    }

    pub(crate) fn take_final_visible_assistant_transcript_delta(
        &mut self,
        final_visible_text: &str,
    ) -> Option<String> {
        self.take_visible_assistant_transcript_delta_from(final_visible_text.to_string())
    }

    fn take_visible_assistant_transcript_delta_from(&mut self, visible: String) -> Option<String> {
        if visible == self.assistant_transcript_emitted {
            return None;
        }

        let delta = if visible.starts_with(&self.assistant_transcript_emitted) {
            visible[self.assistant_transcript_emitted.len()..].to_string()
        } else {
            visible.clone()
        };
        self.assistant_transcript_emitted = visible;
        (!delta.trim().is_empty()).then_some(delta)
    }

    pub(crate) fn has_emitted_visible_assistant_transcript(&self) -> bool {
        !self.assistant_transcript_emitted.trim().is_empty()
    }
}

pub(crate) fn spawn_dispatch_key(
    dispatch: &crate::spawn_orchestration::SpawnTaskDispatch,
) -> String {
    format!("{}\n{}", dispatch.target.trim(), dispatch.task.trim())
}
