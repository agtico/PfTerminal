//! Turn output, progress, audit, and command plan types.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::OwnedMutexGuard;
use tokio_util::sync::CancellationToken;

use super::pane::ClaudeCommandMode;
use super::pane::ClaudePaneTurnStatus;
use super::pane::ClaudePaneUsageStatus;

pub(crate) struct PreparedClaudePaneTurn {
    pub(crate) pane_id: String,
    pub(crate) plan: ClaudeCommandPlan,
    pub(crate) cancel_token: CancellationToken,
    pub(crate) _lock: OwnedMutexGuard<()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudePaneTurnOutput {
    pub(crate) text: String,
    pub(crate) status: ClaudePaneTurnStatus,
    pub(crate) session_id: Option<String>,
    pub(crate) usage_summary: Option<String>,
    pub(crate) usage_status: ClaudePaneUsageStatus,
    pub(crate) artifact_path: PathBuf,
    pub(crate) audit_path: PathBuf,
    pub(crate) duration_ms: i64,
    pub(crate) terminal_reason: Option<String>,
    pub(crate) error_summary: Option<String>,
    pub(crate) tool_names: Vec<String>,
    pub(crate) tool_events: Vec<ClaudePaneToolEvent>,
    pub(crate) reasoning_events: Vec<ClaudePaneReasoningEvent>,
    pub(crate) command_mode: ClaudeCommandMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClaudePaneToolEvent {
    pub(crate) name: String,
    pub(crate) preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClaudePaneReasoningEvent {
    pub(crate) preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClaudePaneTurnProgress {
    pub(crate) pane_id: String,
    pub(crate) phase: String,
    pub(crate) summary: String,
    pub(crate) assistant_text_delta: Option<String>,
    /// Non-rendered diagnostic metadata used to deduplicate progress events.
    /// Artifact/audit paths are intentionally shown only in final turn messages.
    pub(crate) hint: Option<String>,
    pub(crate) elapsed_ms: i64,
    pub(crate) artifact_path: PathBuf,
    pub(crate) audit_path: PathBuf,
}

pub(crate) struct ClaudeCommandPlan {
    pub(crate) executable: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) env_remove: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) pane_id: String,
    pub(crate) pane_title: String,
    pub(crate) profile_title: String,
    pub(crate) provider_model: String,
    pub(crate) turn_index: u64,
    pub(crate) command_mode: ClaudeCommandMode,
    pub(crate) command_session_id: String,
    pub(crate) max_turns: Option<String>,
    pub(crate) artifact_path: PathBuf,
    pub(crate) audit_path: PathBuf,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) bridge: Option<ClaudeBridgePlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClaudePaneTurnAudit {
    pub(crate) pane_id: String,
    pub(crate) pane_title: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) session_id: Option<String>,
    pub(crate) turn_index: u64,
    pub(crate) command_mode: ClaudeCommandMode,
    pub(crate) max_turns: Option<String>,
    pub(crate) artifact_path: PathBuf,
    pub(crate) audit_path: PathBuf,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) started_at_unix_ms: u128,
    pub(crate) ended_at_unix_ms: u128,
    pub(crate) last_progress_elapsed_ms: Option<i64>,
    pub(crate) duration_ms: i64,
    pub(crate) usage: Option<Value>,
    pub(crate) usage_status: ClaudePaneUsageStatus,
    pub(crate) terminal_reason: Option<String>,
    pub(crate) status: ClaudePaneTurnStatus,
    pub(crate) error_summary: Option<String>,
    pub(crate) reasoning_event_count: usize,
    pub(crate) reasoning_events: Vec<ClaudePaneReasoningEvent>,
    pub(crate) tool_use_count: usize,
    pub(crate) tool_names: Vec<String>,
    pub(crate) tool_events: Vec<ClaudePaneToolEvent>,
}

impl ClaudePaneTurnOutput {
    pub(crate) fn audit_hint(&self) -> String {
        let tools = if self.tool_names.is_empty() {
            "tools: none".to_string()
        } else {
            format!("tools: {}", self.tool_names.join(", "))
        };
        let reasoning = if self.reasoning_events.is_empty() {
            String::new()
        } else {
            format!("; reasoning: {}", self.reasoning_events.len())
        };
        let terminal = self
            .terminal_reason
            .as_deref()
            .map(|reason| format!("; terminal_reason: {reason}"))
            .unwrap_or_default();
        let usage = self
            .usage_hint()
            .map(|usage| format!("; usage: {usage}"))
            .unwrap_or_default();
        format!(
            "status: {}; mode: {}; {tools}{reasoning}{terminal}{usage}; artifact: {}; audit: {}",
            self.status.label(),
            self.command_mode.label(),
            self.artifact_path.display(),
            self.audit_path.display()
        )
    }

    pub(crate) fn failure_message(&self) -> String {
        let summary = self
            .error_summary
            .as_deref()
            .unwrap_or("Claude pane turn did not complete successfully.");
        match self.status {
            ClaudePaneTurnStatus::MaxTurnsPause => format!(
                "Claude pane paused at max turns. Type `continue` in this pane to resume the same Claude session. {summary}"
            ),
            ClaudePaneTurnStatus::TimeoutPause => format!(
                "Claude pane timed out locally. Type `continue` in this pane to resume if the audit captured a Claude session id. {summary}"
            ),
            ClaudePaneTurnStatus::Interrupted => {
                format!("Claude pane turn interrupted. {summary}")
            }
            ClaudePaneTurnStatus::ProviderError => {
                format!("Claude pane provider error. {summary}")
            }
            ClaudePaneTurnStatus::ParseFailure => {
                format!("Claude pane output could not be parsed. {summary}")
            }
            ClaudePaneTurnStatus::Success => summary.to_string(),
        }
    }

    pub(crate) fn usage_hint(&self) -> Option<String> {
        match self.usage_status {
            ClaudePaneUsageStatus::Reported => self.usage_summary.clone(),
            ClaudePaneUsageStatus::Missing => Some("missing".to_string()),
            ClaudePaneUsageStatus::Unknown => Some("unknown".to_string()),
            ClaudePaneUsageStatus::Untrusted => {
                Some("untrusted provider-reported zero or incomplete usage".to_string())
            }
        }
    }
}

pub(crate) struct ClaudeBridgePlan {
    pub(crate) kind: ClaudeBridgeKind,
    pub(crate) listener: StdTcpListener,
    pub(crate) bind_addr: SocketAddr,
    pub(crate) upstream_base_url: String,
    pub(crate) upstream_api_key: String,
    pub(crate) upstream_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeBridgeKind {
    AmbientChat,
    AnthropicPassthrough,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BridgeToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) input: Value,
}

impl std::fmt::Debug for ClaudeCommandPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_keys = self.env.keys().cloned().collect::<Vec<_>>();
        f.debug_struct("ClaudeCommandPlan")
            .field("executable", &self.executable)
            .field("args", &self.args)
            .field("env_keys", &env_keys)
            .field("env_remove", &self.env_remove)
            .field("cwd", &self.cwd)
            .field("pane_id", &self.pane_id)
            .field("profile_title", &self.profile_title)
            .field("provider_model", &self.provider_model)
            .field("turn_index", &self.turn_index)
            .field("command_mode", &self.command_mode)
            .field("command_session_id", &self.command_session_id)
            .field("artifact_path", &self.artifact_path)
            .field("audit_path", &self.audit_path)
            .field("timeout_ms", &self.timeout_ms)
            .field(
                "bridge_addr",
                &self.bridge.as_ref().map(|bridge| bridge.bind_addr),
            )
            .finish()
    }
}
