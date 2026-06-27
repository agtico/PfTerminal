use std::collections::BTreeMap;
use std::collections::HashSet;
use std::io::Write as _;
use std::net::SocketAddr;
use std::net::TcpListener as StdTcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use codex_app_server_protocol::UserInput;
use codex_model_provider_info::AMBIENT_DEFAULT_MODEL;
use codex_model_provider_info::BASETEN_DEFAULT_MODEL;
use codex_model_provider_info::OPENROUTER_DEFAULT_MODEL;
use codex_model_provider_info::VERCEL_DEFAULT_MODEL;
use codex_model_provider_info::VERCEL_GLM_5_2_FAST_MODEL;
use codex_model_provider_info::ZAI_DEFAULT_MODEL;
use codex_vault::Vault;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::OwnedMutexGuard;
use tokio::time::MissedTickBehavior;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::App;
use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::chatwidget::ChatWidget;
use crate::spawn_orchestration::SpawnRole;
use crate::tui;

pub(crate) const CODEX_MAIN_PANE_ID: &str = "codex-main";
const CLAUDE_PANE_PROGRESS_HEARTBEAT: Duration = Duration::from_secs(30);
const AMBIENT_BRIDGE_UPSTREAM_MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ClaudeProviderProfileKind {
    ClaudePlan,
    AmbientGlm52,
    ZaiGlm52,
    BasetenGlm52,
    OpenRouterGlm52,
    VercelGlm52,
    VercelGlm52Fast,
}

impl ClaudeProviderProfileKind {
    pub(crate) fn profile(self) -> ClaudeProviderProfile {
        match self {
            Self::ClaudePlan => ClaudeProviderProfile {
                kind: self,
                title: "Claude Code - Claude Plan",
                description: "Use Claude Code's native auth and selected Claude model.",
                claude_model: "sonnet",
                provider_model: "sonnet",
                small_model: "haiku",
                base_url: None,
                vault_label: None,
                uses_bare_mode: false,
                transport: ClaudeProviderTransport::DirectAnthropic,
            },
            Self::AmbientGlm52 => ClaudeProviderProfile {
                kind: self,
                title: "Claude Code - GLM 5.2 Ambient",
                description: "Use Ambient's Claude Code endpoint with the Ambient vault key.",
                claude_model: "opus",
                provider_model: "glm-5.2[1m]",
                small_model: "glm-4.7",
                base_url: Some("https://api.ambient.xyz"),
                vault_label: Some("provider/ambient_api_key"),
                uses_bare_mode: true,
                transport: ClaudeProviderTransport::AmbientChatBridge,
            },
            Self::ZaiGlm52 => ClaudeProviderProfile {
                kind: self,
                title: "Claude Code - GLM 5.2 Z.AI",
                description: "Experimental direct Z.AI Anthropic-compatible route; smoke test before relying on it.",
                claude_model: "opus",
                provider_model: "glm-5.2[1m]",
                small_model: "glm-4.7",
                base_url: Some("https://api.z.ai/api/anthropic"),
                vault_label: Some("provider/zai_api_key"),
                uses_bare_mode: true,
                transport: ClaudeProviderTransport::DirectAnthropic,
            },
            Self::BasetenGlm52 => ClaudeProviderProfile {
                kind: self,
                title: "Claude Code - GLM 5.2 Baseten",
                description: "Experimental Baseten Anthropic-compatible route; smoke test before relying on it.",
                claude_model: "opus",
                provider_model: "zai-org/GLM-5.2",
                small_model: "zai-org/GLM-5.2",
                base_url: Some("https://inference.baseten.co"),
                vault_label: Some("provider/baseten_api_key"),
                uses_bare_mode: true,
                transport: ClaudeProviderTransport::DirectAnthropic,
            },
            Self::OpenRouterGlm52 => ClaudeProviderProfile {
                kind: self,
                title: "Claude Code - GLM 5.2 OpenRouter",
                description: "Experimental OpenRouter Anthropic-compatible route; smoke test before relying on it.",
                claude_model: "opus",
                provider_model: "z-ai/glm-5.2",
                small_model: "z-ai/glm-5.2",
                base_url: Some("https://openrouter.ai/api"),
                vault_label: Some("provider/openrouter_api_key"),
                uses_bare_mode: true,
                transport: ClaudeProviderTransport::DirectAnthropic,
            },
            Self::VercelGlm52 => ClaudeProviderProfile {
                kind: self,
                title: "Claude Code - GLM 5.2 Vercel",
                description: "Use Vercel AI Gateway's Anthropic-compatible Claude Code route with the Vercel vault key.",
                claude_model: "opus",
                provider_model: "zai/glm-5.2",
                small_model: "zai/glm-5.2-fast",
                base_url: Some("https://ai-gateway.vercel.sh"),
                vault_label: Some("provider/ai_gateway_api_key"),
                uses_bare_mode: true,
                transport: ClaudeProviderTransport::AnthropicPassthroughBridge,
            },
            Self::VercelGlm52Fast => ClaudeProviderProfile {
                kind: self,
                title: "Claude Code - GLM 5.2 Fast Vercel",
                description: "Use Vercel AI Gateway's fast GLM 5.2 route with the Vercel vault key.",
                claude_model: "opus",
                provider_model: "zai/glm-5.2-fast",
                small_model: "zai/glm-5.2-fast",
                base_url: Some("https://ai-gateway.vercel.sh"),
                vault_label: Some("provider/ai_gateway_api_key"),
                uses_bare_mode: true,
                transport: ClaudeProviderTransport::AnthropicPassthroughBridge,
            },
        }
    }

    pub(crate) fn status_model_label(self) -> String {
        let profile = self.profile();
        profile
            .title
            .strip_prefix("Claude Code - ")
            .unwrap_or(profile.title)
            .to_string()
    }

    pub(crate) fn native_codex_model(self) -> Option<&'static str> {
        match self {
            Self::ClaudePlan => None,
            Self::AmbientGlm52 => Some(AMBIENT_DEFAULT_MODEL),
            Self::ZaiGlm52 => Some(ZAI_DEFAULT_MODEL),
            Self::BasetenGlm52 => Some(BASETEN_DEFAULT_MODEL),
            Self::OpenRouterGlm52 => Some(OPENROUTER_DEFAULT_MODEL),
            Self::VercelGlm52 => Some(VERCEL_DEFAULT_MODEL),
            Self::VercelGlm52Fast => Some(VERCEL_GLM_5_2_FAST_MODEL),
        }
    }

    pub(crate) fn creation_options() -> &'static [Self] {
        &[
            Self::AmbientGlm52,
            Self::ZaiGlm52,
            Self::BasetenGlm52,
            Self::OpenRouterGlm52,
            Self::VercelGlm52,
            Self::VercelGlm52Fast,
            Self::ClaudePlan,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeProviderTransport {
    DirectAnthropic,
    AmbientChatBridge,
    AnthropicPassthroughBridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClaudeProviderProfile {
    pub(crate) kind: ClaudeProviderProfileKind,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) claude_model: &'static str,
    pub(crate) provider_model: &'static str,
    pub(crate) small_model: &'static str,
    pub(crate) base_url: Option<&'static str>,
    pub(crate) vault_label: Option<&'static str>,
    pub(crate) uses_bare_mode: bool,
    pub(crate) transport: ClaudeProviderTransport,
}

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

    fn is_success(self) -> bool {
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
    fn label(self) -> &'static str {
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
    fn label(self) -> &'static str {
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
    live_turn: Option<ClaudePaneLiveTurn>,
    cancel_token: Option<CancellationToken>,
    lock: Arc<Mutex<()>>,
    next_turn_index: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ClaudePaneLiveTurn {
    elapsed_ms: i64,
    current: String,
    phase: String,
    thinking_tokens: Option<String>,
    assistant_commentary_buffer: String,
    assistant_blurbs: Vec<String>,
    reasoning_blurbs: Vec<String>,
    tool_blurbs: Vec<String>,
    assistant_dispatch_buffer: String,
    sent_dispatch_keys: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudePaneLiveStatus {
    pub(crate) header: String,
    pub(crate) details: Option<String>,
}

impl ClaudePaneLiveTurn {
    fn starting() -> Self {
        Self {
            elapsed_ms: 0,
            current: "starting Claude".to_string(),
            phase: "starting".to_string(),
            thinking_tokens: None,
            assistant_commentary_buffer: String::new(),
            assistant_blurbs: Vec::new(),
            reasoning_blurbs: Vec::new(),
            tool_blurbs: Vec::new(),
            assistant_dispatch_buffer: String::new(),
            sent_dispatch_keys: HashSet::new(),
        }
    }

    fn update(&mut self, progress: &ClaudePaneTurnProgress) {
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
                    self.current = format!("Claude: {update}");
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
                self.current = format!("reasoning: {reasoning}");
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

    fn display(&self) -> ClaudePaneLiveStatus {
        let header = format!("Claude running · {}", format_elapsed_ms(self.elapsed_ms));
        let mut lines = vec![format!("Current: {}", self.current)];
        if !self.assistant_blurbs.is_empty() {
            lines.push("Updates:".to_string());
            let hidden = self.assistant_blurbs.len().saturating_sub(4);
            if hidden > 0 {
                lines.push(format!("  +{hidden} earlier"));
            }
            let visible_start = self.assistant_blurbs.len().saturating_sub(4);
            for update in self.assistant_blurbs.iter().skip(visible_start) {
                lines.push(format!("  {update}"));
            }
        }
        if self.thinking_tokens.is_some() || !self.reasoning_blurbs.is_empty() {
            lines.push("Reasoning:".to_string());
            if let Some(thinking_tokens) = &self.thinking_tokens {
                lines.push(format!("  {thinking_tokens}"));
            }
            let hidden = self.reasoning_blurbs.len().saturating_sub(3);
            if hidden > 0 {
                lines.push(format!("  +{hidden} earlier"));
            }
            let visible_start = self.reasoning_blurbs.len().saturating_sub(3);
            for reasoning in self.reasoning_blurbs.iter().skip(visible_start) {
                lines.push(format!("  {reasoning}"));
            }
        }
        if !self.tool_blurbs.is_empty() {
            lines.push("Tools:".to_string());
            let hidden = self.tool_blurbs.len().saturating_sub(5);
            if hidden > 0 {
                lines.push(format!("  +{hidden} earlier"));
            }
            let visible_start = self.tool_blurbs.len().saturating_sub(5);
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

    fn filter_new_dispatches(
        &mut self,
        dispatches: Vec<crate::spawn_orchestration::SpawnTaskDispatch>,
    ) -> Vec<crate::spawn_orchestration::SpawnTaskDispatch> {
        dispatches
            .into_iter()
            .filter(|dispatch| self.sent_dispatch_keys.insert(spawn_dispatch_key(dispatch)))
            .collect()
    }
}

fn spawn_dispatch_key(dispatch: &crate::spawn_orchestration::SpawnTaskDispatch) -> String {
    format!("{}\n{}", dispatch.target.trim(), dispatch.task.trim())
}

#[derive(Debug)]
pub(crate) struct ClaudePaneRegistry {
    active_user_pane_id: String,
    panes: Vec<ClaudePane>,
}

impl ClaudePaneRegistry {
    pub(crate) fn new() -> Self {
        Self {
            active_user_pane_id: CODEX_MAIN_PANE_ID.to_string(),
            panes: Vec::new(),
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
        self.panes.push(pane);
        self.active_user_pane_id = id.clone();
        Ok(id)
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
                ClaudePaneTurnStatus::Success | ClaudePaneTurnStatus::MaxTurnsPause => {
                    if let Some(session_id) = &output.session_id {
                        pane.claude_session_id = Some(session_id.clone());
                    }
                }
                ClaudePaneTurnStatus::TimeoutPause
                | ClaudePaneTurnStatus::Interrupted
                | ClaudePaneTurnStatus::ProviderError
                | ClaudePaneTurnStatus::ParseFailure => {
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
        }
    }

    pub(crate) fn set_latest_task_message(&mut self, pane_id: &str, task: Option<String>) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == pane_id) {
            pane.latest_task_message = task.map(|task| compact_claude_pane_metadata(&task, 240));
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

pub(crate) struct PreparedClaudePaneTurn {
    pub(crate) pane_id: String,
    plan: ClaudeCommandPlan,
    cancel_token: CancellationToken,
    _lock: OwnedMutexGuard<()>,
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
    executable: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: PathBuf,
    pane_id: String,
    pane_title: String,
    profile_title: String,
    provider_model: String,
    turn_index: u64,
    command_mode: ClaudeCommandMode,
    max_turns: Option<String>,
    artifact_path: PathBuf,
    audit_path: PathBuf,
    timeout_ms: Option<u64>,
    bridge: Option<ClaudeBridgePlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClaudePaneTurnAudit {
    pane_id: String,
    pane_title: String,
    provider: String,
    model: String,
    session_id: Option<String>,
    turn_index: u64,
    command_mode: ClaudeCommandMode,
    max_turns: Option<String>,
    artifact_path: PathBuf,
    audit_path: PathBuf,
    timeout_ms: Option<u64>,
    started_at_unix_ms: u128,
    ended_at_unix_ms: u128,
    last_progress_elapsed_ms: Option<i64>,
    duration_ms: i64,
    usage: Option<Value>,
    usage_status: ClaudePaneUsageStatus,
    terminal_reason: Option<String>,
    status: ClaudePaneTurnStatus,
    error_summary: Option<String>,
    reasoning_event_count: usize,
    reasoning_events: Vec<ClaudePaneReasoningEvent>,
    tool_use_count: usize,
    tool_names: Vec<String>,
    tool_events: Vec<ClaudePaneToolEvent>,
}

impl ClaudePaneTurnOutput {
    fn audit_hint(&self) -> String {
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

    fn failure_message(&self) -> String {
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

    fn usage_hint(&self) -> Option<String> {
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

struct ClaudeBridgePlan {
    kind: ClaudeBridgeKind,
    listener: StdTcpListener,
    bind_addr: SocketAddr,
    upstream_base_url: String,
    upstream_api_key: String,
    upstream_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeBridgeKind {
    AmbientChat,
    AnthropicPassthrough,
}

#[derive(Debug, Clone, PartialEq)]
struct BridgeToolCall {
    id: String,
    name: String,
    input: Value,
}

impl std::fmt::Debug for ClaudeCommandPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_keys = self.env.keys().cloned().collect::<Vec<_>>();
        f.debug_struct("ClaudeCommandPlan")
            .field("executable", &self.executable)
            .field("args", &self.args)
            .field("env_keys", &env_keys)
            .field("cwd", &self.cwd)
            .field("pane_id", &self.pane_id)
            .field("profile_title", &self.profile_title)
            .field("provider_model", &self.provider_model)
            .field("turn_index", &self.turn_index)
            .field("command_mode", &self.command_mode)
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

fn ensure_vault_label_exists(codex_home: &Path, label: &str) -> Result<()> {
    let vault = Vault::new(codex_home.to_path_buf());
    match vault.exists(label) {
        Ok(true) => Ok(()),
        Ok(false) => Err(anyhow!(
            "Missing vault credential `{label}`. Add it from /providers before creating this Claude pane."
        )),
        Err(err) => Err(anyhow!("Could not read vault credential `{label}`: {err}")),
    }
}

fn reveal_provider_secret(codex_home: &Path, label: &str) -> Result<String> {
    if !allowed_provider_vault_label(label) {
        return Err(anyhow!(
            "Vault label `{label}` is not allowed for Claude pane auth"
        ));
    }
    let vault = Vault::new(codex_home.to_path_buf());
    vault
        .reveal(label)
        .with_context(|| format!("failed to read vault credential `{label}`"))
}

pub(crate) fn allowed_provider_vault_label(label: &str) -> bool {
    matches!(
        label,
        "provider/zai_api_key"
            | "provider/ambient_api_key"
            | "provider/baseten_api_key"
            | "provider/openrouter_api_key"
            | "provider/ai_gateway_api_key"
    )
}

fn build_claude_command_plan(
    pane: &ClaudePane,
    prompt: String,
    codex_home: &Path,
) -> Result<ClaudeCommandPlan> {
    let profile = pane.profile.profile();
    let turn_index = pane.next_turn_index;
    let settings_path = pane.artifact_dir.join("settings.json");
    let artifact_path = pane
        .artifact_dir
        .join(format!("turn-{turn_index:04}.jsonl"));
    let audit_path = pane
        .artifact_dir
        .join(format!("turn-{turn_index:04}.audit.json"));
    let mut bridge = None;
    let mut base_url_override = None;
    if matches!(
        profile.transport,
        ClaudeProviderTransport::AmbientChatBridge
            | ClaudeProviderTransport::AnthropicPassthroughBridge
    ) {
        let Some(label) = profile.vault_label else {
            return Err(anyhow!("Claude bridge requires a provider vault label"));
        };
        let secret = reveal_provider_secret(codex_home, label)?;
        let listener = StdTcpListener::bind("127.0.0.1:0")
            .context("failed to bind Claude bridge loopback listener")?;
        listener
            .set_nonblocking(true)
            .context("failed to set Claude bridge listener nonblocking")?;
        let bind_addr = listener
            .local_addr()
            .context("failed to read Claude bridge listener address")?;
        let (kind, upstream_base_url, upstream_model) = match profile.transport {
            ClaudeProviderTransport::AmbientChatBridge => (
                ClaudeBridgeKind::AmbientChat,
                "https://api.ambient.xyz/v1/chat/completions".to_string(),
                "zai-org/GLM-5.2-FP8".to_string(),
            ),
            ClaudeProviderTransport::AnthropicPassthroughBridge => (
                ClaudeBridgeKind::AnthropicPassthrough,
                profile
                    .base_url
                    .ok_or_else(|| anyhow!("Anthropic passthrough bridge requires base URL"))?
                    .trim_end_matches('/')
                    .to_string(),
                profile.provider_model.to_string(),
            ),
            ClaudeProviderTransport::DirectAnthropic => {
                unreachable!("direct providers do not use bridge")
            }
        };
        base_url_override = Some(format!("http://{bind_addr}"));
        bridge = Some(ClaudeBridgePlan {
            kind,
            listener,
            bind_addr,
            upstream_base_url,
            upstream_api_key: secret,
            upstream_model,
        });
    }
    let settings = settings_json_with_base_url(
        profile,
        if bridge.is_some() {
            None
        } else {
            Some("pfterminal")
        },
        base_url_override.as_deref(),
    );
    std::fs::write(&settings_path, settings.to_string()).with_context(|| {
        format!(
            "failed to write Claude pane settings `{}`",
            settings_path.display()
        )
    })?;

    let mut env = BTreeMap::new();
    if let Some(base_url) = base_url_override.as_deref().or(profile.base_url) {
        env.insert("ANTHROPIC_BASE_URL".to_string(), base_url.to_string());
    }
    if bridge.is_some() {
        env.insert("ANTHROPIC_API_KEY".to_string(), String::new());
        env.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "pfterminal-local-bridge".to_string(),
        );
    } else if let Some(label) = profile.vault_label {
        let secret = reveal_provider_secret(codex_home, label)?;
        env.insert("ANTHROPIC_API_KEY".to_string(), String::new());
        env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), secret);
    }
    if profile.uses_bare_mode {
        env.insert(
            "ANTHROPIC_MODEL".to_string(),
            profile.claude_model.to_string(),
        );
        env.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
            profile.provider_model.to_string(),
        );
        env.insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
            profile.provider_model.to_string(),
        );
        env.insert(
            "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
            profile.small_model.to_string(),
        );
        env.insert(
            "ANTHROPIC_SMALL_FAST_MODEL".to_string(),
            profile.small_model.to_string(),
        );
        env.insert(
            "CLAUDE_CODE_SUBAGENT_MODEL".to_string(),
            profile.provider_model.to_string(),
        );
        env.insert(
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW".to_string(),
            "1000000".to_string(),
        );
        env.insert("API_TIMEOUT_MS".to_string(), "3000000".to_string());
        env.insert(
            "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS".to_string(),
            "1".to_string(),
        );
        env.insert(
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
            "1".to_string(),
        );
        env.insert(
            "CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK".to_string(),
            "1".to_string(),
        );
        env.insert("CLAUDECODE".to_string(), String::new());
    }

    let mut args = Vec::new();
    if profile.uses_bare_mode {
        args.push("--bare".to_string());
    }
    args.extend([
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--settings".to_string(),
        settings_path.to_string_lossy().into_owned(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
        "--exclude-dynamic-system-prompt-sections".to_string(),
        "--model".to_string(),
        profile.claude_model.to_string(),
    ]);
    if profile.uses_bare_mode {
        args.extend(["--setting-sources".to_string(), "project".to_string()]);
    }
    let command_mode = if let Some(session_id) = &pane.claude_session_id {
        args.push("--resume".to_string());
        args.push(session_id.clone());
        ClaudeCommandMode::Resume
    } else {
        args.push("--session-id".to_string());
        args.push(Uuid::new_v4().to_string());
        ClaudeCommandMode::NewSession
    };
    args.push(prompt);

    Ok(ClaudeCommandPlan {
        executable: "claude".to_string(),
        args,
        env,
        cwd: pane.cwd.clone(),
        pane_id: pane.id.clone(),
        pane_title: pane.title.clone(),
        profile_title: profile.title.to_string(),
        provider_model: profile.provider_model.to_string(),
        turn_index,
        command_mode,
        max_turns: None,
        artifact_path,
        audit_path,
        timeout_ms: None,
        bridge,
    })
}

fn settings_json_with_base_url(
    profile: ClaudeProviderProfile,
    helper_program: Option<&str>,
    base_url_override: Option<&str>,
) -> Value {
    let mut env = serde_json::Map::new();
    if profile.uses_bare_mode {
        if let Some(base_url) = base_url_override.or(profile.base_url) {
            env.insert(
                "ANTHROPIC_BASE_URL".to_string(),
                Value::String(base_url.to_string()),
            );
        }
        env.insert(
            "ANTHROPIC_API_KEY".to_string(),
            Value::String(String::new()),
        );
        env.insert(
            "ANTHROPIC_MODEL".to_string(),
            Value::String(profile.claude_model.to_string()),
        );
        env.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
            Value::String(profile.provider_model.to_string()),
        );
        env.insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
            Value::String(profile.provider_model.to_string()),
        );
        env.insert(
            "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
            Value::String(profile.small_model.to_string()),
        );
        env.insert(
            "ANTHROPIC_SMALL_FAST_MODEL".to_string(),
            Value::String(profile.small_model.to_string()),
        );
        env.insert(
            "CLAUDE_CODE_SUBAGENT_MODEL".to_string(),
            Value::String(profile.provider_model.to_string()),
        );
        env.insert(
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW".to_string(),
            Value::String("1000000".to_string()),
        );
        env.insert(
            "API_TIMEOUT_MS".to_string(),
            Value::String("3000000".to_string()),
        );
        env.insert(
            "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS".to_string(),
            Value::String("1".to_string()),
        );
        env.insert(
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
            Value::String("1".to_string()),
        );
        env.insert(
            "CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK".to_string(),
            Value::String("1".to_string()),
        );
    }

    let mut settings = serde_json::Map::new();
    settings.insert("env".to_string(), Value::Object(env));
    if profile.uses_bare_mode
        && let (Some(helper_program), Some(label)) = (helper_program, profile.vault_label)
    {
        settings.insert(
            "apiKeyHelper".to_string(),
            Value::String(format!("{helper_program} vault auth-helper {label}")),
        );
    }
    Value::Object(settings)
}

pub(crate) fn prompt_from_user_turn(op: &AppCommand) -> Result<Option<String>> {
    let AppCommand::UserTurn { items, .. } = op else {
        return Ok(None);
    };
    let mut chunks = Vec::new();
    for item in items {
        match item {
            UserInput::Text { text, .. } => chunks.push(text.clone()),
            UserInput::Skill { name, path } => {
                chunks.push(format!("[Selected skill: {name} at {}]", path.display()))
            }
            UserInput::Mention { name, path } => {
                chunks.push(format!("[Mention: {name} at {path}]"));
            }
            UserInput::Image { .. } | UserInput::LocalImage { .. } => {
                return Err(anyhow!(
                    "Claude panes currently accept text, skills, and mentions only; image input is not supported yet."
                ));
            }
        }
    }
    Ok(Some(chunks.join("\n\n")))
}

pub(crate) fn compose_claude_pane_prompt(prompt: String, spawn_context: Option<&str>) -> String {
    let Some(spawn_context) = spawn_context
        .map(str::trim)
        .filter(|context| !context.is_empty())
    else {
        return prompt;
    };
    format!("{spawn_context}\n\nUser message:\n{prompt}")
}

fn claude_pane_title(
    profile: ClaudeProviderProfileKind,
    spawn_role: Option<SpawnRole>,
    spawn_nickname: Option<&str>,
) -> String {
    match (spawn_role, spawn_nickname) {
        (Some(role), Some(nickname)) => format!(
            "Claude Code {} [{}] - {}",
            nickname,
            role.agent_type().unwrap_or_else(|| role.label()),
            profile.status_model_label()
        ),
        (Some(role), None) => format!(
            "Claude Code {} - {}",
            role.label(),
            profile.status_model_label()
        ),
        (None, _) => profile.profile().title.to_string(),
    }
}

pub(crate) async fn run_prepared_claude_turn(
    prepared: PreparedClaudePaneTurn,
    progress_tx: Option<AppEventSender>,
) -> Result<ClaudePaneTurnOutput, String> {
    run_claude_command_plan(prepared.plan, prepared.cancel_token, progress_tx)
        .await
        .map_err(|err| format!("{err:#}"))
}

#[derive(Debug, Clone)]
pub struct ClaudePaneSmokeOptions {
    pub codex_home: PathBuf,
    pub cwd: PathBuf,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudePaneSmokeReport {
    pub report_path: PathBuf,
    pub passed: bool,
    pub summary: String,
    pub entries: Vec<ClaudePaneSmokeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudePaneSmokeEntry {
    pub provider: String,
    pub profile: Option<String>,
    pub status: String,
    pub first_turn_status: Option<ClaudePaneTurnStatus>,
    pub second_turn_status: Option<ClaudePaneTurnStatus>,
    pub artifact_path: Option<PathBuf>,
    pub audit_path: Option<PathBuf>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClaudePaneWorkflowOptions {
    pub codex_home: PathBuf,
    pub cwd: PathBuf,
    pub providers: Vec<String>,
    pub workflows: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudePaneWorkflowReport {
    pub report_path: PathBuf,
    pub passed: bool,
    pub summary: String,
    pub entries: Vec<ClaudePaneWorkflowEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudePaneWorkflowEntry {
    pub provider: String,
    pub profile: Option<String>,
    pub workflow: String,
    pub status: String,
    pub artifact_path: Option<PathBuf>,
    pub audit_path: Option<PathBuf>,
    pub fixture_path: Option<PathBuf>,
    pub error: Option<String>,
    pub output_excerpt: Option<String>,
}

pub async fn run_claude_pane_smoke(
    options: ClaudePaneSmokeOptions,
) -> Result<ClaudePaneSmokeReport> {
    let uses_default_baseline = options.providers.is_empty();
    let provider_names = if uses_default_baseline {
        vec![
            "ambient".to_string(),
            "zai".to_string(),
            "baseten".to_string(),
            "openrouter".to_string(),
            "claude-plan".to_string(),
        ]
    } else {
        options.providers
    };
    let mut entries = Vec::new();
    for provider_name in provider_names {
        entries.push(
            run_single_smoke_provider(
                &options.codex_home,
                &options.cwd,
                provider_name.trim().to_string(),
            )
            .await,
        );
    }

    let passed = if uses_default_baseline {
        entries
            .iter()
            .any(|entry| entry.status == "passed" && entry.provider == "ambient")
    } else {
        !entries.is_empty() && entries.iter().all(|entry| entry.status == "passed")
    };
    let report_dir = options.codex_home.join("panes").join("smoke-reports");
    std::fs::create_dir_all(&report_dir).with_context(|| {
        format!(
            "failed to create Claude pane smoke report directory `{}`",
            report_dir.display()
        )
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let report_path = report_dir.join(format!("claude-pane-smoke-{timestamp}.json"));
    let summary = format!(
        "Claude pane smoke: {} passed, {} checked; report: {}",
        entries
            .iter()
            .filter(|entry| entry.status == "passed")
            .count(),
        entries.len(),
        report_path.display()
    );
    let report = ClaudePaneSmokeReport {
        report_path: report_path.clone(),
        passed,
        summary,
        entries,
    };
    let bytes = serde_json::to_vec_pretty(&report).context("failed to serialize smoke report")?;
    std::fs::write(&report_path, bytes).with_context(|| {
        format!(
            "failed to write Claude pane smoke report `{}`",
            report_path.display()
        )
    })?;
    Ok(report)
}

pub async fn run_claude_pane_workflow_suite(
    options: ClaudePaneWorkflowOptions,
) -> Result<ClaudePaneWorkflowReport> {
    let provider_names = if options.providers.is_empty() {
        vec!["ambient".to_string()]
    } else {
        options.providers
    };
    let workflow_names = if options.workflows.is_empty() {
        vec![
            "mock-website".to_string(),
            "numpy-pandas-benchmark".to_string(),
            "code-review".to_string(),
            "auditability".to_string(),
        ]
    } else {
        options.workflows
    };
    let report_root = options.codex_home.join("panes").join("workflow-reports");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let fixture_root = report_root.join(format!("fixtures-{timestamp}"));
    std::fs::create_dir_all(&fixture_root).with_context(|| {
        format!(
            "failed to create Claude pane workflow fixture directory `{}`",
            fixture_root.display()
        )
    })?;

    let mut entries = Vec::new();
    for provider_name in provider_names {
        for workflow_name in &workflow_names {
            entries.push(
                run_single_workflow(
                    &options.codex_home,
                    &options.cwd,
                    &fixture_root,
                    provider_name.trim().to_string(),
                    workflow_name.trim().to_string(),
                )
                .await,
            );
        }
    }
    let passed = entries.iter().all(|entry| entry.status == "passed");
    let report_path = report_root.join(format!("claude-pane-workflow-suite-{timestamp}.json"));
    let summary = format!(
        "Claude pane workflow suite: {} passed, {} checked; report: {}",
        entries
            .iter()
            .filter(|entry| entry.status == "passed")
            .count(),
        entries.len(),
        report_path.display()
    );
    let report = ClaudePaneWorkflowReport {
        report_path: report_path.clone(),
        passed,
        summary,
        entries,
    };
    let bytes =
        serde_json::to_vec_pretty(&report).context("failed to serialize workflow report")?;
    std::fs::write(&report_path, bytes).with_context(|| {
        format!(
            "failed to write Claude pane workflow report `{}`",
            report_path.display()
        )
    })?;
    Ok(report)
}

async fn run_single_workflow(
    codex_home: &Path,
    cwd: &Path,
    fixture_root: &Path,
    provider_name: String,
    workflow_name: String,
) -> ClaudePaneWorkflowEntry {
    let Some(profile) = smoke_provider_profile(&provider_name) else {
        return workflow_entry_error(
            provider_name,
            None,
            workflow_name,
            None,
            None,
            None,
            "unknown workflow provider".to_string(),
        );
    };
    let profile_title = Some(profile.profile().title.to_string());
    match workflow_name.as_str() {
        "mock-website" => {
            run_mock_website_workflow(codex_home, fixture_root, provider_name, profile).await
        }
        "numpy-pandas-benchmark" => {
            run_numpy_pandas_benchmark_workflow(codex_home, fixture_root, provider_name, profile)
                .await
        }
        "code-review" => run_code_review_workflow(codex_home, cwd, provider_name, profile).await,
        "auditability" => {
            run_auditability_workflow(codex_home, fixture_root, provider_name, profile).await
        }
        _ => workflow_entry_error(
            provider_name,
            profile_title,
            workflow_name,
            None,
            None,
            None,
            "unknown workflow".to_string(),
        ),
    }
}

async fn run_mock_website_workflow(
    codex_home: &Path,
    fixture_root: &Path,
    provider_name: String,
    profile: ClaudeProviderProfileKind,
) -> ClaudePaneWorkflowEntry {
    let workflow = "mock-website".to_string();
    let fixture_path = workflow_fixture_path(fixture_root, &provider_name, &workflow);
    if let Err(err) = std::fs::create_dir_all(&fixture_path) {
        return workflow_entry_error(
            provider_name,
            Some(profile.profile().title.to_string()),
            workflow,
            None,
            None,
            Some(fixture_path),
            format!("failed to create fixture: {err}"),
        );
    }
    let prompt = concat!(
        "Build a tiny mock website in the current directory for a product named ",
        "PFT Pane Observatory. Create index.html plus either styles.css or script.js. ",
        "The page must include the exact text PFT Pane Observatory and one styled or ",
        "interactive element. After writing files, reply with marker PFT_MOCK_SITE_DONE ",
        "and list the files you created."
    )
    .to_string();
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = match registry.create_pane(profile, fixture_path.clone(), codex_home) {
        Ok(id) => id,
        Err(err) => {
            return workflow_entry_error(
                provider_name,
                Some(profile.profile().title.to_string()),
                workflow,
                None,
                None,
                Some(fixture_path),
                err.to_string(),
            );
        }
    };
    let output = match run_smoke_turn(&mut registry, &pane_id, prompt, codex_home).await {
        Ok(output) => output,
        Err(err) => {
            return workflow_entry_error(
                provider_name,
                Some(profile.profile().title.to_string()),
                workflow,
                None,
                None,
                Some(fixture_path),
                err,
            );
        }
    };
    let index_path = fixture_path.join("index.html");
    let index = std::fs::read_to_string(&index_path).unwrap_or_default();
    let has_asset =
        fixture_path.join("styles.css").exists() || fixture_path.join("script.js").exists();
    if output.status.is_success()
        && output.text.contains("PFT_MOCK_SITE_DONE")
        && index.contains("PFT Pane Observatory")
        && has_asset
    {
        workflow_entry_pass(provider_name, profile, workflow, output, Some(fixture_path))
    } else {
        workflow_entry_from_output(
            provider_name,
            profile,
            workflow,
            output,
            Some(fixture_path),
            "mock website verification failed".to_string(),
        )
    }
}

async fn run_numpy_pandas_benchmark_workflow(
    codex_home: &Path,
    fixture_root: &Path,
    provider_name: String,
    profile: ClaudeProviderProfileKind,
) -> ClaudePaneWorkflowEntry {
    let workflow = "numpy-pandas-benchmark".to_string();
    let fixture_path = workflow_fixture_path(fixture_root, &provider_name, &workflow);
    if let Err(err) = std::fs::create_dir_all(&fixture_path) {
        return workflow_entry_error(
            provider_name,
            Some(profile.profile().title.to_string()),
            workflow,
            None,
            None,
            Some(fixture_path),
            format!("failed to create fixture: {err}"),
        );
    }
    let prompt = concat!(
        "Create and run a Python benchmark comparing NumPy vs Pandas for filtering ",
        "and aggregating numeric rows. Use a deterministic random seed and a data size ",
        "small enough to finish quickly. Output a markdown table with columns ",
        "Implementation, Mean time, Fastest run, and Notes. Include marker ",
        "PFT_NUMPY_PANDAS_BENCH_DONE. If numpy or pandas is missing, report the missing ",
        "dependency clearly instead of hanging."
    )
    .to_string();
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = match registry.create_pane(profile, fixture_path.clone(), codex_home) {
        Ok(id) => id,
        Err(err) => {
            return workflow_entry_error(
                provider_name,
                Some(profile.profile().title.to_string()),
                workflow,
                None,
                None,
                Some(fixture_path),
                err.to_string(),
            );
        }
    };
    let output = match run_smoke_turn(&mut registry, &pane_id, prompt, codex_home).await {
        Ok(output) => output,
        Err(err) => {
            return workflow_entry_error(
                provider_name,
                Some(profile.profile().title.to_string()),
                workflow,
                None,
                None,
                Some(fixture_path),
                err,
            );
        }
    };
    let has_table = output.text.contains('|')
        && output.text.to_lowercase().contains("numpy")
        && output.text.to_lowercase().contains("pandas")
        && output.text.contains("PFT_NUMPY_PANDAS_BENCH_DONE");
    if output.status.is_success() && has_table {
        workflow_entry_pass(provider_name, profile, workflow, output, Some(fixture_path))
    } else {
        workflow_entry_from_output(
            provider_name,
            profile,
            workflow,
            output,
            Some(fixture_path),
            "NumPy vs Pandas benchmark verification failed".to_string(),
        )
    }
}

async fn run_code_review_workflow(
    codex_home: &Path,
    cwd: &Path,
    provider_name: String,
    profile: ClaudeProviderProfileKind,
) -> ClaudePaneWorkflowEntry {
    let workflow = "code-review".to_string();
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = match registry.create_pane(profile, cwd.to_path_buf(), codex_home) {
        Ok(id) => id,
        Err(err) => {
            return workflow_entry_error(
                provider_name,
                Some(profile.profile().title.to_string()),
                workflow,
                None,
                None,
                None,
                err.to_string(),
            );
        }
    };
    let prompt = concat!(
        "Perform a read-only code review of the active implementation diff in this repo. ",
        "You must inspect the actual patch body, not only commit metadata or --stat. ",
        "Start with `git diff --find-renames --find-copies --unified=80`. ",
        "If there is no working-tree diff, review `git show --format=fuller --find-renames --find-copies --unified=80 HEAD` instead. ",
        "If the output is too large, continue with narrower `git diff --patch -- <path>` or `git show --patch HEAD -- <path>` ",
        "commands until you have inspected real diff hunks for the changed files. ",
        "Review that patch as the source of truth and stop reading once the changed diff hunks are understood. ",
        "Return marker PFT_CODE_REVIEW_DONE, include `DIFF_INSPECTED: yes`, and give concrete ",
        "findings with file references or say no findings with a short rationale. ",
        "Do not edit files."
    )
    .to_string();
    let first_output = match run_smoke_turn(&mut registry, &pane_id, prompt, codex_home).await {
        Ok(output) => output,
        Err(err) => {
            return workflow_entry_error(
                provider_name,
                Some(profile.profile().title.to_string()),
                workflow,
                None,
                None,
                None,
                err,
            );
        }
    };
    let has_review = first_output.text.contains("PFT_CODE_REVIEW_DONE")
        && first_output.text.contains("DIFF_INSPECTED: yes")
        && artifact_contains_patch_body(&first_output.artifact_path)
        && shallow_review_rejection_reason(&first_output.text).is_none();
    if !(first_output.status.is_success() && has_review && !first_output.tool_names.is_empty()) {
        let error = shallow_review_rejection_reason(&first_output.text)
            .unwrap_or_else(|| "fresh code review did not prove full diff inspection".to_string());
        return workflow_entry_from_output(
            provider_name,
            profile,
            workflow,
            first_output,
            None,
            error,
        );
    }

    let resume_prompt = concat!(
        "Continue the same read-only code review. Use the context already gathered. ",
        "You may use additional filesystem tools if needed. Return marker ",
        "PFT_CODE_REVIEW_RESUME_DONE and include either one additional concrete finding ",
        "with a file reference or `NO_ADDITIONAL_FINDINGS` with a short rationale. ",
        "Do not edit files."
    )
    .to_string();
    let resume_output =
        match run_smoke_turn(&mut registry, &pane_id, resume_prompt, codex_home).await {
            Ok(output) => output,
            Err(err) => {
                return workflow_entry_error(
                    provider_name,
                    Some(profile.profile().title.to_string()),
                    workflow,
                    None,
                    None,
                    None,
                    err,
                );
            }
        };
    let has_resume_review = resume_output.text.contains("PFT_CODE_REVIEW_RESUME_DONE")
        && shallow_review_rejection_reason(&resume_output.text).is_none();
    if resume_output.status.is_success()
        && has_resume_review
        && matches!(resume_output.command_mode, ClaudeCommandMode::Resume)
    {
        workflow_entry_pass(provider_name, profile, workflow, resume_output, None)
    } else {
        workflow_entry_from_output(
            provider_name,
            profile,
            workflow,
            resume_output,
            None,
            "resumed code review verification failed".to_string(),
        )
    }
}

async fn run_auditability_workflow(
    codex_home: &Path,
    fixture_root: &Path,
    provider_name: String,
    profile: ClaudeProviderProfileKind,
) -> ClaudePaneWorkflowEntry {
    let workflow = "auditability".to_string();
    let fixture_path = workflow_fixture_path(fixture_root, &provider_name, &workflow);
    if let Err(err) = std::fs::create_dir_all(&fixture_path) {
        return workflow_entry_error(
            provider_name,
            Some(profile.profile().title.to_string()),
            workflow,
            None,
            None,
            Some(fixture_path),
            format!("failed to create fixture: {err}"),
        );
    }
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = match registry.create_pane(profile, fixture_path.clone(), codex_home) {
        Ok(id) => id,
        Err(err) => {
            return workflow_entry_error(
                provider_name,
                Some(profile.profile().title.to_string()),
                workflow,
                None,
                None,
                Some(fixture_path),
                err.to_string(),
            );
        }
    };
    let prompts = [
        "Reply exactly PFT_AUDIT_TURN_1.",
        "Use Bash to run `printf PFT_AUDIT_TURN_2` and then reply with PFT_AUDIT_TURN_2.",
        "Use Bash to run `false`; then explain that the command failed and include marker PFT_AUDIT_FAILURE_PATH.",
    ];
    let mut last_output = None;
    for prompt in prompts {
        let output =
            match run_smoke_turn(&mut registry, &pane_id, prompt.to_string(), codex_home).await {
                Ok(output) => output,
                Err(err) => {
                    return workflow_entry_error(
                        provider_name,
                        Some(profile.profile().title.to_string()),
                        workflow,
                        None,
                        None,
                        Some(fixture_path),
                        err,
                    );
                }
            };
        if !output.audit_path.exists() {
            return workflow_entry_from_output(
                provider_name,
                profile,
                workflow,
                output,
                Some(fixture_path),
                "audit file was not written".to_string(),
            );
        }
        last_output = Some(output);
    }
    let Some(output) = last_output else {
        return workflow_entry_error(
            provider_name,
            Some(profile.profile().title.to_string()),
            workflow,
            None,
            None,
            Some(fixture_path),
            "audit workflow did not run any turns".to_string(),
        );
    };
    if output.status.is_success()
        && output.text.contains("PFT_AUDIT_FAILURE_PATH")
        && !output.tool_names.is_empty()
    {
        workflow_entry_pass(provider_name, profile, workflow, output, Some(fixture_path))
    } else {
        workflow_entry_from_output(
            provider_name,
            profile,
            workflow,
            output,
            Some(fixture_path),
            "auditability workflow verification failed".to_string(),
        )
    }
}

fn artifact_contains_patch_body(path: &Path) -> bool {
    let Ok(artifact) = std::fs::read_to_string(path) else {
        return false;
    };
    artifact.contains("diff --git")
        && artifact.contains("@@")
        && (artifact.contains("+") || artifact.contains("-"))
}

fn shallow_review_rejection_reason(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let rejected = [
        "couldn't pull the full diff",
        "could not pull the full diff",
        "couldn't read the full diff",
        "could not read the full diff",
        "unable to pull the full diff",
        "unable to read the full diff",
        "unable to inspect the full diff",
        "based on the commit metadata",
        "based on the commit message",
        "based on the change description",
        "local tool budget",
        "tool budget was hit",
        "without seeing the full diff",
    ];
    rejected
        .iter()
        .find(|phrase| lower.contains(**phrase))
        .map(|phrase| format!("shallow code review output: `{phrase}`"))
}

fn workflow_fixture_path(fixture_root: &Path, provider: &str, workflow: &str) -> PathBuf {
    fixture_root.join(provider).join(workflow)
}

fn workflow_entry_pass(
    provider: String,
    profile: ClaudeProviderProfileKind,
    workflow: String,
    output: ClaudePaneTurnOutput,
    fixture_path: Option<PathBuf>,
) -> ClaudePaneWorkflowEntry {
    ClaudePaneWorkflowEntry {
        provider,
        profile: Some(profile.profile().title.to_string()),
        workflow,
        status: "passed".to_string(),
        artifact_path: Some(output.artifact_path),
        audit_path: Some(output.audit_path),
        fixture_path,
        error: None,
        output_excerpt: Some(truncate_for_display(&output.text, 1_000)),
    }
}

fn workflow_entry_from_output(
    provider: String,
    profile: ClaudeProviderProfileKind,
    workflow: String,
    output: ClaudePaneTurnOutput,
    fixture_path: Option<PathBuf>,
    error: String,
) -> ClaudePaneWorkflowEntry {
    let failure = output.failure_message();
    let excerpt = truncate_for_display(&output.text, 1_000);
    ClaudePaneWorkflowEntry {
        provider,
        profile: Some(profile.profile().title.to_string()),
        workflow,
        status: "failed".to_string(),
        artifact_path: Some(output.artifact_path),
        audit_path: Some(output.audit_path),
        fixture_path,
        error: Some(format!("{error}: {failure}")),
        output_excerpt: Some(excerpt),
    }
}

fn workflow_entry_error(
    provider: String,
    profile: Option<String>,
    workflow: String,
    artifact_path: Option<PathBuf>,
    audit_path: Option<PathBuf>,
    fixture_path: Option<PathBuf>,
    error: String,
) -> ClaudePaneWorkflowEntry {
    ClaudePaneWorkflowEntry {
        provider,
        profile,
        workflow,
        status: "failed".to_string(),
        artifact_path,
        audit_path,
        fixture_path,
        error: Some(error),
        output_excerpt: None,
    }
}

async fn run_single_smoke_provider(
    codex_home: &Path,
    cwd: &Path,
    provider_name: String,
) -> ClaudePaneSmokeEntry {
    let Some(profile) = smoke_provider_profile(&provider_name) else {
        return ClaudePaneSmokeEntry {
            provider: provider_name,
            profile: None,
            status: "unknown-provider".to_string(),
            first_turn_status: None,
            second_turn_status: None,
            artifact_path: None,
            audit_path: None,
            error: Some("unknown Claude pane smoke provider".to_string()),
        };
    };
    let profile_config = profile.profile();
    let mut registry = ClaudePaneRegistry::new();
    let pane_id = match registry.create_pane(profile, cwd.to_path_buf(), codex_home) {
        Ok(pane_id) => pane_id,
        Err(err) => {
            return ClaudePaneSmokeEntry {
                provider: provider_name,
                profile: Some(profile_config.title.to_string()),
                status: "unavailable".to_string(),
                first_turn_status: None,
                second_turn_status: None,
                artifact_path: None,
                audit_path: None,
                error: Some(err.to_string()),
            };
        }
    };

    let first_result = run_smoke_turn(
        &mut registry,
        &pane_id,
        smoke_first_turn_prompt(),
        codex_home,
    )
    .await;
    let first_output = match first_result {
        Ok(output) => output,
        Err(err) => {
            return ClaudePaneSmokeEntry {
                provider: provider_name,
                profile: Some(profile_config.title.to_string()),
                status: "failed".to_string(),
                first_turn_status: None,
                second_turn_status: None,
                artifact_path: None,
                audit_path: None,
                error: Some(err),
            };
        }
    };
    let artifact_path = Some(first_output.artifact_path.clone());
    let audit_path = Some(first_output.audit_path.clone());
    if !first_output.status.is_success() {
        return ClaudePaneSmokeEntry {
            provider: provider_name,
            profile: Some(profile_config.title.to_string()),
            status: "failed".to_string(),
            first_turn_status: Some(first_output.status),
            second_turn_status: None,
            artifact_path,
            audit_path,
            error: Some(first_output.failure_message()),
        };
    }

    let second_result = run_smoke_turn(
        &mut registry,
        &pane_id,
        "Continue from the same Claude pane session. Reply with exactly: PFT_CLAUDE_SMOKE_RESUME_OK"
            .to_string(),
        codex_home,
    )
    .await;
    match second_result {
        Ok(second_output) if second_output.status.is_success() => ClaudePaneSmokeEntry {
            provider: provider_name,
            profile: Some(profile_config.title.to_string()),
            status: "passed".to_string(),
            first_turn_status: Some(first_output.status),
            second_turn_status: Some(second_output.status),
            artifact_path: Some(second_output.artifact_path),
            audit_path: Some(second_output.audit_path),
            error: None,
        },
        Ok(second_output) => {
            let error = second_output.failure_message();
            ClaudePaneSmokeEntry {
                provider: provider_name,
                profile: Some(profile_config.title.to_string()),
                status: "failed".to_string(),
                first_turn_status: Some(first_output.status),
                second_turn_status: Some(second_output.status),
                artifact_path: Some(second_output.artifact_path),
                audit_path: Some(second_output.audit_path),
                error: Some(error),
            }
        }
        Err(err) => ClaudePaneSmokeEntry {
            provider: provider_name,
            profile: Some(profile_config.title.to_string()),
            status: "failed".to_string(),
            first_turn_status: Some(first_output.status),
            second_turn_status: None,
            artifact_path,
            audit_path,
            error: Some(err),
        },
    }
}

async fn run_smoke_turn(
    registry: &mut ClaudePaneRegistry,
    pane_id: &str,
    prompt: String,
    codex_home: &Path,
) -> Result<ClaudePaneTurnOutput, String> {
    let prepared = registry
        .prepare_turn(pane_id, prompt, codex_home)
        .map_err(|err| err.to_string())?;
    let result = run_prepared_claude_turn(prepared, None).await;
    registry.finish_turn(pane_id, &result);
    result
}

fn smoke_provider_profile(provider_name: &str) -> Option<ClaudeProviderProfileKind> {
    match provider_name {
        "ambient" | "ambient-glm-52" => Some(ClaudeProviderProfileKind::AmbientGlm52),
        "zai" | "zai-glm-52" => Some(ClaudeProviderProfileKind::ZaiGlm52),
        "baseten" | "baseten-glm-52" => Some(ClaudeProviderProfileKind::BasetenGlm52),
        "openrouter" | "openrouter-glm-52" => Some(ClaudeProviderProfileKind::OpenRouterGlm52),
        "vercel" | "vercel-glm-52" => Some(ClaudeProviderProfileKind::VercelGlm52),
        "vercel-fast" | "vercel-glm-52-fast" => Some(ClaudeProviderProfileKind::VercelGlm52Fast),
        "claude-plan" | "claude" => Some(ClaudeProviderProfileKind::ClaudePlan),
        _ => None,
    }
}

fn smoke_first_turn_prompt() -> String {
    concat!(
        "Perform a read-only PFTerminal Claude pane smoke test. ",
        "Use Claude Code filesystem tools to inspect Cargo.toml, ",
        "codex-rs/tui/src/claude_panes.rs, and ",
        "docs/current-sprint/claude-code-integration-completion-spec.md. ",
        "Then reply with a compact JSON object containing marker ",
        "PFT_CLAUDE_SMOKE_OK, files_checked, tools_used, and two concrete ",
        "code-review observations about the Claude pane implementation. ",
        "Do not edit files."
    )
    .to_string()
}

async fn run_claude_command_plan(
    mut plan: ClaudeCommandPlan,
    cancel_token: CancellationToken,
    progress_tx: Option<AppEventSender>,
) -> Result<ClaudePaneTurnOutput> {
    let started_at = Instant::now();
    let started_at_unix_ms = unix_epoch_ms();
    let mut last_progress_elapsed_ms = Some(0);
    emit_claude_progress(
        &progress_tx,
        &plan,
        &started_at,
        "starting",
        "Claude pane starting.",
        Some(format!(
            "mode: {}; artifact: {}; audit: {}",
            plan.command_mode.label(),
            plan.artifact_path.display(),
            plan.audit_path.display()
        )),
    );
    let bridge_handle = plan
        .bridge
        .take()
        .map(|bridge| tokio::spawn(run_claude_bridge(bridge)));
    let mut command = Command::new(&plan.executable);
    command.kill_on_drop(true);
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let mut child = command
        .args(&plan.args)
        .envs(&plan.env)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run `{}`", plan.executable))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Claude stdout pipe was not available"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Claude stderr pipe was not available"))?;
    let stderr_task = tokio::spawn(async move {
        let mut stderr_reader = BufReader::new(stderr);
        let mut stderr_text = String::new();
        let _ = stderr_reader.read_to_string(&mut stderr_text).await;
        stderr_text
    });

    let mut artifact = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&plan.artifact_path)
        .with_context(|| {
            format!(
                "failed to open Claude pane artifact `{}`",
                plan.artifact_path.display()
            )
        })?;
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stdout_text = String::new();
    let mut heartbeat = interval(CLAUDE_PANE_PROGRESS_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut timed_out = false;
    let mut interrupted = false;
    let mut cleanup_error: Option<String> = None;
    let mut last_progress_key: Option<String> = None;
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                interrupted = true;
                if let Err(err) = stop_claude_child(&mut child).await {
                    cleanup_error = Some(err.to_string());
                }
                break;
            }
            line = stdout_lines.next_line() => {
                let Some(line) = line.context("failed to read Claude stdout")? else {
                    break;
                };
                stdout_text.push_str(&line);
                stdout_text.push('\n');
                use std::io::Write as _;
                writeln!(artifact, "{line}").with_context(|| {
                    format!(
                        "failed to append Claude pane artifact `{}`",
                        plan.artifact_path.display()
                    )
                })?;
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    for progress in progresses_from_claude_value(&plan, &started_at, &value) {
                        last_progress_elapsed_ms = Some(progress.elapsed_ms);
                        let is_assistant_text = progress.phase == "assistant-text";
                        let key = progress_key(&progress);
                        if (is_assistant_text || last_progress_key.as_deref() != Some(key.as_str()))
                            && let Some(tx) = progress_tx.as_ref()
                        {
                            tx.send(AppEvent::ClaudePaneTurnProgress { progress });
                        }
                        if !is_assistant_text {
                            last_progress_key = Some(key);
                        }
                    }
                }
            }
            _ = heartbeat.tick() => {
                last_progress_elapsed_ms = Some(elapsed_ms(&started_at));
                emit_claude_progress(
                    &progress_tx,
                    &plan,
                    &started_at,
                    "waiting",
                    "Claude running.",
                    None,
                );
            }
        }
    }
    artifact.flush().with_context(|| {
        format!(
            "failed to flush Claude pane artifact `{}`",
            plan.artifact_path.display()
        )
    })?;

    let wait_result = if timed_out || interrupted {
        None
    } else {
        Some(
            tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .unwrap_or_else(|_| {
                    timed_out = true;
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Claude process did not exit after stdout closed",
                    ))
                }),
        )
    };
    if timed_out && let Err(err) = stop_claude_child(&mut child).await {
        cleanup_error = Some(err.to_string());
    }
    let stderr = if timed_out || interrupted {
        stderr_task.abort();
        String::new()
    } else {
        stderr_task.await.unwrap_or_default()
    };
    if let Some(handle) = bridge_handle {
        handle.abort();
    }
    let duration_ms = elapsed_ms(&started_at);
    let ended_at_unix_ms = unix_epoch_ms();

    if let Some(err) = cleanup_error {
        let output = partial_failed_turn_output(
            &plan,
            duration_ms,
            ClaudePaneTurnStatus::ProviderError,
            Some("cleanup_failed".to_string()),
            format!(
                "Claude pane process cleanup failed after interrupt/timeout; the turn is not considered safely stopped: {err}"
            ),
            &stdout_text,
        );
        write_turn_audit(
            &plan,
            &output,
            started_at_unix_ms,
            ended_at_unix_ms,
            last_progress_elapsed_ms,
        )?;
        return Ok(output);
    }

    if interrupted {
        let output = partial_failed_turn_output(
            &plan,
            duration_ms,
            ClaudePaneTurnStatus::Interrupted,
            Some("interrupted".to_string()),
            "Claude pane turn interrupted by user.".to_string(),
            &stdout_text,
        );
        write_turn_audit(
            &plan,
            &output,
            started_at_unix_ms,
            ended_at_unix_ms,
            last_progress_elapsed_ms,
        )?;
        return Ok(output);
    }

    if timed_out {
        let output = partial_failed_turn_output(
            &plan,
            duration_ms,
            ClaudePaneTurnStatus::TimeoutPause,
            Some("process_wait_timeout".to_string()),
            "Claude stdout closed, but the Claude process did not exit within the cleanup grace period. Type `continue` in this pane to resume if a Claude session id was captured.".to_string(),
            &stdout_text,
        );
        write_turn_audit(
            &plan,
            &output,
            started_at_unix_ms,
            ended_at_unix_ms,
            last_progress_elapsed_ms,
        )?;
        return Ok(output);
    }

    let status = match wait_result {
        Some(Ok(status)) => status,
        Some(Err(err)) => {
            let output = partial_failed_turn_output(
                &plan,
                duration_ms,
                ClaudePaneTurnStatus::ProviderError,
                Some("process_wait".to_string()),
                format!("failed to wait for Claude process: {err}"),
                &stdout_text,
            );
            write_turn_audit(
                &plan,
                &output,
                started_at_unix_ms,
                ended_at_unix_ms,
                last_progress_elapsed_ms,
            )?;
            return Ok(output);
        }
        None => unreachable!("timeout branch returned earlier"),
    };

    if !stdout_text.trim().is_empty() {
        let output = match parse_claude_output(&stdout_text) {
            Ok(parsed) => turn_output_from_parsed(&plan, parsed, duration_ms),
            Err(err) => failed_turn_output(
                &plan,
                duration_ms,
                ClaudePaneTurnStatus::ParseFailure,
                Some("parse_failure".to_string()),
                format!("{err:#}"),
            ),
        };
        write_turn_audit(
            &plan,
            &output,
            started_at_unix_ms,
            ended_at_unix_ms,
            last_progress_elapsed_ms,
        )?;
        return Ok(output);
    }

    if !status.success() {
        let output = failed_turn_output(
            &plan,
            duration_ms,
            ClaudePaneTurnStatus::ProviderError,
            Some("process_exit".to_string()),
            format!(
                "Claude exited with status {}: {}",
                status,
                truncate_for_display(stderr.trim(), 1_000)
            ),
        );
        write_turn_audit(
            &plan,
            &output,
            started_at_unix_ms,
            ended_at_unix_ms,
            last_progress_elapsed_ms,
        )?;
        return Ok(output);
    }

    let output = failed_turn_output(
        &plan,
        duration_ms,
        ClaudePaneTurnStatus::ParseFailure,
        Some("empty_output".to_string()),
        "Claude returned empty output".to_string(),
    );
    write_turn_audit(
        &plan,
        &output,
        started_at_unix_ms,
        ended_at_unix_ms,
        last_progress_elapsed_ms,
    )?;
    Ok(output)
}

async fn stop_claude_child(child: &mut Child) -> Result<()> {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // Claude may have an active tool subprocess. The pane starts Claude in its own process
        // group, so kill the group first, then reap the direct child below.
        let kill_result = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        if kill_result == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(anyhow!(
                    "failed to send SIGKILL to Claude process group {pid}: {err}"
                ));
            }
        }
    }

    if let Err(err) = child.start_kill()
        && err.kind() != std::io::ErrorKind::InvalidInput
    {
        return Err(anyhow!("failed to kill Claude process: {err}"));
    }

    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(anyhow!(
            "failed to wait for Claude process after kill: {err}"
        )),
        Err(_) => Err(anyhow!(
            "Claude process did not exit after SIGKILL within cleanup timeout"
        )),
    }
}

fn turn_output_from_parsed(
    plan: &ClaudeCommandPlan,
    parsed: ParsedClaudeOutput,
    duration_ms: i64,
) -> ClaudePaneTurnOutput {
    ClaudePaneTurnOutput {
        text: parsed.text,
        status: parsed.status,
        session_id: parsed.session_id,
        usage_status: usage_status_from_summary(parsed.usage_summary.as_deref()),
        usage_summary: parsed.usage_summary,
        artifact_path: plan.artifact_path.clone(),
        audit_path: plan.audit_path.clone(),
        duration_ms,
        terminal_reason: parsed.terminal_reason,
        error_summary: parsed.error_summary,
        tool_names: parsed.tool_names,
        tool_events: parsed.tool_events,
        reasoning_events: parsed.reasoning_events,
        command_mode: plan.command_mode,
    }
}

fn failed_turn_output(
    plan: &ClaudeCommandPlan,
    duration_ms: i64,
    status: ClaudePaneTurnStatus,
    terminal_reason: Option<String>,
    error_summary: String,
) -> ClaudePaneTurnOutput {
    ClaudePaneTurnOutput {
        text: String::new(),
        status,
        session_id: None,
        usage_summary: None,
        usage_status: ClaudePaneUsageStatus::Missing,
        artifact_path: plan.artifact_path.clone(),
        audit_path: plan.audit_path.clone(),
        duration_ms,
        terminal_reason,
        error_summary: Some(error_summary),
        tool_names: Vec::new(),
        tool_events: Vec::new(),
        reasoning_events: Vec::new(),
        command_mode: plan.command_mode,
    }
}

fn partial_failed_turn_output(
    plan: &ClaudeCommandPlan,
    duration_ms: i64,
    status: ClaudePaneTurnStatus,
    terminal_reason: Option<String>,
    error_summary: String,
    stdout: &str,
) -> ClaudePaneTurnOutput {
    let mut output = failed_turn_output(plan, duration_ms, status, terminal_reason, error_summary);
    if let Ok(parsed) = parse_claude_output(stdout) {
        if !parsed.text.trim().is_empty() {
            output.text = parsed.text;
        }
        output.session_id = parsed.session_id;
        output.usage_status = usage_status_from_summary(parsed.usage_summary.as_deref());
        output.usage_summary = parsed.usage_summary;
        if output.terminal_reason.is_none() {
            output.terminal_reason = parsed.terminal_reason;
        }
        output.tool_names = parsed.tool_names;
        output.tool_events = parsed.tool_events;
        output.reasoning_events = parsed.reasoning_events;
    } else {
        output.tool_events = tool_events_from_stdout(stdout);
        output.tool_names = dedupe_tool_names(
            output
                .tool_events
                .iter()
                .map(|event| event.name.clone())
                .collect(),
        );
        output.reasoning_events = reasoning_events_from_stdout(stdout);
    }
    output
}

fn write_turn_audit(
    plan: &ClaudeCommandPlan,
    output: &ClaudePaneTurnOutput,
    started_at_unix_ms: u128,
    ended_at_unix_ms: u128,
    last_progress_elapsed_ms: Option<i64>,
) -> Result<()> {
    let audit = ClaudePaneTurnAudit {
        pane_id: plan.pane_id.clone(),
        pane_title: plan.pane_title.clone(),
        provider: plan.profile_title.clone(),
        model: plan.provider_model.clone(),
        session_id: output.session_id.clone(),
        turn_index: plan.turn_index,
        command_mode: plan.command_mode,
        max_turns: plan.max_turns.clone(),
        artifact_path: output.artifact_path.clone(),
        audit_path: output.audit_path.clone(),
        timeout_ms: plan.timeout_ms,
        started_at_unix_ms,
        ended_at_unix_ms,
        last_progress_elapsed_ms,
        duration_ms: output.duration_ms,
        usage: output
            .usage_summary
            .as_deref()
            .and_then(|usage| serde_json::from_str::<Value>(usage).ok()),
        usage_status: output.usage_status,
        terminal_reason: output.terminal_reason.clone(),
        status: output.status,
        error_summary: output.error_summary.clone(),
        reasoning_event_count: output.reasoning_events.len(),
        reasoning_events: output.reasoning_events.clone(),
        tool_use_count: output.tool_events.len(),
        tool_names: output.tool_names.clone(),
        tool_events: output.tool_events.clone(),
    };
    let bytes =
        serde_json::to_vec_pretty(&audit).context("failed to serialize Claude turn audit")?;
    std::fs::write(&plan.audit_path, bytes).with_context(|| {
        format!(
            "failed to write Claude pane audit `{}`",
            plan.audit_path.display()
        )
    })
}

async fn run_claude_bridge(plan: ClaudeBridgePlan) -> Result<()> {
    let listener = TcpListener::from_std(plan.listener)
        .context("failed to create async Claude bridge listener")?;
    let api_key = Arc::new(plan.upstream_api_key);
    let upstream_base_url = Arc::new(plan.upstream_base_url);
    let upstream_model = Arc::new(plan.upstream_model);
    let kind = plan.kind;
    let http = reqwest::Client::new();
    loop {
        let (stream, _) = listener.accept().await?;
        let api_key = api_key.clone();
        let upstream_base_url = upstream_base_url.clone();
        let upstream_model = upstream_model.clone();
        let http = http.clone();
        tokio::spawn(async move {
            let result = match kind {
                ClaudeBridgeKind::AmbientChat => {
                    handle_ambient_bridge_connection(stream, api_key, upstream_model, http).await
                }
                ClaudeBridgeKind::AnthropicPassthrough => {
                    handle_anthropic_passthrough_bridge_connection(
                        stream,
                        api_key,
                        upstream_base_url,
                        http,
                    )
                    .await
                }
            };
            if let Err(err) = result {
                tracing::debug!(error = %err, "Claude bridge connection failed");
            }
        });
    }
}

async fn handle_ambient_bridge_connection(
    mut stream: tokio::net::TcpStream,
    api_key: Arc<String>,
    upstream_model: Arc<String>,
    http: reqwest::Client,
) -> Result<()> {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut temp).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&temp[..read]);
        if let Some(pos) = find_header_end(&buffer) {
            break pos;
        }
        if buffer.len() > 1024 * 1024 {
            return Err(anyhow!("Ambient Claude bridge request headers too large"));
        }
    };

    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let request_line = headers.lines().next().unwrap_or_default().to_string();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);

    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = stream.read(&mut temp).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }
    let body = &buffer[body_start..buffer.len().min(body_start + content_length)];

    if request_line.contains("/v1/messages/count_tokens") {
        write_json_response(&mut stream, serde_json::json!({ "input_tokens": 1 })).await?;
        return Ok(());
    }

    if !request_line.contains("/v1/messages") {
        write_json_status_response(
            &mut stream,
            404,
            serde_json::json!({ "error": { "type": "not_found", "message": "not found" } }),
        )
        .await?;
        return Ok(());
    }

    let request: Value = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => {
            write_json_status_response(
                &mut stream,
                400,
                serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "message": format!("invalid Claude Messages request: {err}")
                    }
                }),
            )
            .await?;
            return Ok(());
        }
    };
    let wants_stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_tokens = request
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(1024)
        .max(1);
    let chat_messages = match ambient_chat_messages_from_claude_request(&request) {
        Ok(messages) => messages,
        Err(err) => {
            write_json_status_response(
                &mut stream,
                400,
                serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "request_translation_error",
                        "message": err.to_string()
                    }
                }),
            )
            .await?;
            return Ok(());
        }
    };
    let chat_tools = ambient_chat_tools_from_claude_request(&request);
    let mut upstream_body = serde_json::json!({
        "model": upstream_model.as_str(),
        "messages": chat_messages,
        "max_tokens": max_tokens,
    });
    if !chat_tools.is_empty() {
        upstream_body["tools"] = Value::Array(chat_tools);
        upstream_body["tool_choice"] = Value::String("auto".to_string());
    }
    let response = if wants_stream {
        send_ambient_chat_request_with_stream_heartbeat(
            &mut stream,
            upstream_model.as_str(),
            &http,
            api_key.as_str(),
            &upstream_body,
        )
        .await
    } else {
        send_ambient_chat_request_with_retry(&http, api_key.as_str(), &upstream_body).await
    };
    let (status, response_text) = match response {
        Ok(response) => response,
        Err(err) => {
            if wants_stream {
                write_anthropic_stream_error(
                    &mut stream,
                    "upstream_transport_error",
                    &format!("Ambient Claude bridge upstream transport error: {err}"),
                )
                .await?;
            } else {
                write_json_status_response(
                    &mut stream,
                    502,
                    serde_json::json!({
                        "type": "error",
                        "error": {
                            "type": "upstream_transport_error",
                            "message": err.to_string()
                        }
                    }),
                )
                .await?;
            }
            return Ok(());
        }
    };
    if !status.is_success() {
        if wants_stream {
            write_anthropic_stream_error(
                &mut stream,
                "upstream_error",
                &format!(
                    "Ambient Claude bridge upstream returned HTTP {}: {response_text}",
                    status.as_u16()
                ),
            )
            .await?;
        } else {
            write_json_status_response(
                &mut stream,
                status.as_u16(),
                serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "upstream_error",
                        "message": response_text
                    }
                }),
            )
            .await?;
        }
        return Ok(());
    }

    let upstream: Value = match serde_json::from_str(&response_text) {
        Ok(upstream) => upstream,
        Err(err) => {
            if wants_stream {
                write_anthropic_stream_error(
                    &mut stream,
                    "upstream_invalid_json",
                    &format!("Ambient Chat response was not JSON: {err}"),
                )
                .await?;
            } else {
                write_json_status_response(
                    &mut stream,
                    502,
                    serde_json::json!({
                        "type": "error",
                        "error": {
                            "type": "upstream_invalid_json",
                            "message": format!("Ambient Chat response was not JSON: {err}")
                        }
                    }),
                )
                .await?;
            }
            return Ok(());
        }
    };
    let usage = upstream.get("usage").cloned().unwrap_or_else(|| {
        serde_json::json!({
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0
        })
    });
    let tool_calls = bridge_tool_calls_from_ambient_response(&upstream);
    if !tool_calls.is_empty() {
        if wants_stream {
            write_anthropic_stream_tool_use_completion(
                &mut stream,
                upstream_model.as_str(),
                &tool_calls,
                &usage,
            )
            .await?;
        } else {
            write_json_response(
                &mut stream,
                anthropic_tool_use_response(upstream_model.as_str(), &tool_calls, &usage),
            )
            .await?;
        }
        return Ok(());
    }

    let text = upstream
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("OK")
        .to_string();
    if wants_stream {
        write_anthropic_stream_text_completion(&mut stream, upstream_model.as_str(), &text, &usage)
            .await?;
    } else {
        write_json_response(
            &mut stream,
            anthropic_message_response(upstream_model.as_str(), &text, &usage),
        )
        .await?;
    }
    Ok(())
}

async fn handle_anthropic_passthrough_bridge_connection(
    mut stream: tokio::net::TcpStream,
    api_key: Arc<String>,
    upstream_base_url: Arc<String>,
    http: reqwest::Client,
) -> Result<()> {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut temp).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&temp[..read]);
        if let Some(pos) = find_header_end(&buffer) {
            break pos;
        }
        if buffer.len() > 1024 * 1024 {
            return Err(anyhow!(
                "Anthropic passthrough bridge request headers too large"
            ));
        }
    };

    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let request_line = headers.lines().next().unwrap_or_default().to_string();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);

    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = stream.read(&mut temp).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }
    let body = &buffer[body_start..buffer.len().min(body_start + content_length)];

    if request_line.contains("/v1/messages/count_tokens") {
        write_json_response(&mut stream, serde_json::json!({ "input_tokens": 1 })).await?;
        return Ok(());
    }

    if !request_line.contains("/v1/messages") {
        write_json_status_response(
            &mut stream,
            404,
            serde_json::json!({ "error": { "type": "not_found", "message": "not found" } }),
        )
        .await?;
        return Ok(());
    }

    let upstream_path = request_target_from_request_line(&request_line).unwrap_or("/v1/messages");
    let upstream_url = format!(
        "{}{}",
        upstream_base_url.trim_end_matches('/'),
        upstream_path
    );
    let response = http
        .post(upstream_url)
        .bearer_auth(api_key.as_str())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("anthropic-version", "2023-06-01")
        .body(body.to_vec())
        .send()
        .await
        .context("Anthropic passthrough bridge upstream request failed")?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let response_body = response
        .bytes()
        .await
        .context("failed to read Anthropic passthrough bridge response")?;
    write_raw_http_response(
        &mut stream,
        status.as_u16(),
        status.canonical_reason().unwrap_or("OK"),
        &content_type,
        response_body.as_ref(),
    )
    .await?;
    Ok(())
}

async fn send_ambient_chat_request_with_retry(
    http: &reqwest::Client,
    api_key: &str,
    upstream_body: &Value,
) -> Result<(reqwest::StatusCode, String)> {
    let mut last_error = None;
    for attempt in 1..=AMBIENT_BRIDGE_UPSTREAM_MAX_ATTEMPTS {
        let response = http
            .post("https://api.ambient.xyz/v1/chat/completions")
            .bearer_auth(api_key)
            .json(upstream_body)
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status();
                let should_retry =
                    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
                let retry_delay = ambient_retry_after_delay(response.headers())
                    .unwrap_or_else(|| ambient_bridge_retry_delay(attempt));
                match response.text().await {
                    Ok(response_text) => {
                        if should_retry && attempt < AMBIENT_BRIDGE_UPSTREAM_MAX_ATTEMPTS {
                            tracing::warn!(
                                status = status.as_u16(),
                                attempt,
                                max_attempts = AMBIENT_BRIDGE_UPSTREAM_MAX_ATTEMPTS,
                                "Ambient Claude bridge upstream returned retriable status"
                            );
                            sleep_ambient_bridge_retry(retry_delay).await;
                            continue;
                        }
                        return Ok((status, response_text));
                    }
                    Err(err) => {
                        let error = anyhow!("Ambient Chat bridge failed to read response: {err}");
                        if should_retry && attempt < AMBIENT_BRIDGE_UPSTREAM_MAX_ATTEMPTS {
                            tracing::warn!(
                                status = status.as_u16(),
                                attempt,
                                max_attempts = AMBIENT_BRIDGE_UPSTREAM_MAX_ATTEMPTS,
                                error = %error,
                                "Ambient Claude bridge failed to read retriable upstream response"
                            );
                            sleep_ambient_bridge_retry(retry_delay).await;
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
            Err(err) => {
                last_error = Some(anyhow!(
                    "Ambient Chat bridge upstream request failed: {err}"
                ));
            }
        }

        if attempt < AMBIENT_BRIDGE_UPSTREAM_MAX_ATTEMPTS {
            let error = last_error
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "unknown upstream transport failure".to_string());
            tracing::warn!(
                attempt,
                max_attempts = AMBIENT_BRIDGE_UPSTREAM_MAX_ATTEMPTS,
                error = %error,
                "Ambient Claude bridge upstream transport failed"
            );
            sleep_ambient_bridge_retry(ambient_bridge_retry_delay(attempt)).await;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("Ambient Chat bridge upstream request failed")))
}

async fn send_ambient_chat_request_with_stream_heartbeat(
    stream: &mut tokio::net::TcpStream,
    _model: &str,
    http: &reqwest::Client,
    api_key: &str,
    upstream_body: &Value,
) -> Result<(reqwest::StatusCode, String)> {
    write_anthropic_stream_headers(stream).await?;
    let request = send_ambient_chat_request_with_retry(http, api_key, upstream_body);
    tokio::pin!(request);
    let mut heartbeat = interval(Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut request => return result,
            _ = heartbeat.tick() => {
                write_anthropic_stream_ping(stream).await?;
            }
        }
    }
}

fn ambient_retry_after_delay(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let retry_after = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    let seconds = retry_after.parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds.min(300)))
}

fn ambient_bridge_retry_delay(attempt: usize) -> Duration {
    Duration::from_millis((attempt as u64).saturating_mul(250))
}

async fn sleep_ambient_bridge_retry(delay: Duration) {
    tokio::time::sleep(delay).await;
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn request_target_from_request_line(request_line: &str) -> Option<&str> {
    let mut parts = request_line.split_whitespace();
    let _method = parts.next()?;
    parts.next()
}

fn ambient_chat_messages_from_claude_request(request: &Value) -> Result<Vec<Value>> {
    let mut messages = Vec::new();
    if let Some(system) = request.get("system") {
        let system_text = claude_content_to_text(system);
        if !system_text.trim().is_empty() {
            messages.push(serde_json::json!({ "role": "system", "content": system_text }));
        }
    }
    for message in request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Claude Messages request missing messages array"))?
    {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = message.get("content").unwrap_or(&Value::Null);
        if role == "assistant" {
            let text = claude_text_blocks_to_text(content);
            let tool_calls = ambient_assistant_tool_calls_from_claude_content(content);
            if text.trim().is_empty() && tool_calls.is_empty() {
                continue;
            }
            let mut assistant = serde_json::Map::new();
            assistant.insert("role".to_string(), Value::String("assistant".to_string()));
            assistant.insert(
                "content".to_string(),
                if text.trim().is_empty() {
                    Value::Null
                } else {
                    Value::String(text)
                },
            );
            if !tool_calls.is_empty() {
                assistant.insert("tool_calls".to_string(), Value::Array(tool_calls));
            }
            messages.push(Value::Object(assistant));
            continue;
        }

        let text = claude_text_blocks_to_text(content);
        if !text.trim().is_empty() {
            messages.push(serde_json::json!({ "role": role, "content": text }));
        }
        for tool_result in ambient_tool_result_messages_from_claude_content(content) {
            messages.push(tool_result);
        }
    }
    if messages.is_empty() {
        messages.push(serde_json::json!({ "role": "user", "content": "Continue." }));
    }
    Ok(messages)
}

fn ambient_chat_tools_from_claude_request(request: &Value) -> Vec<Value> {
    request
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?;
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let parameters = tool
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
            Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": parameters
                }
            }))
        })
        .collect()
}

fn ambient_assistant_tool_calls_from_claude_content(content: &Value) -> Vec<Value> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?;
            let name = item.get("name").and_then(Value::as_str)?;
            let input = item
                .get("input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            Some(serde_json::json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": input.to_string()
                }
            }))
        })
        .collect()
}

fn ambient_tool_result_messages_from_claude_content(content: &Value) -> Vec<Value> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|item| {
            let tool_call_id = item.get("tool_use_id").and_then(Value::as_str)?;
            Some(serde_json::json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": claude_content_to_text(item.get("content").unwrap_or(&Value::Null))
            }))
        })
        .collect()
}

fn claude_text_blocks_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn claude_content_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    return Some(text.to_string());
                }
                if let Some(text) = item.get("content").and_then(Value::as_str) {
                    return Some(text.to_string());
                }
                None
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn anthropic_message_response(model: &str, text: &str, usage: &Value) -> Value {
    serde_json::json!({
        "id": format!("msg_pfterminal_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{ "type": "text", "text": text }],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": anthropic_response_usage(usage)
    })
}

fn bridge_tool_calls_from_ambient_response(upstream: &Value) -> Vec<BridgeToolCall> {
    upstream
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool_call| {
            let id = tool_call.get("id").and_then(Value::as_str)?;
            let function = tool_call.get("function")?;
            let name = function.get("name").and_then(Value::as_str)?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let input = serde_json::from_str(arguments).unwrap_or_else(|_| {
                serde_json::json!({
                    "_raw_arguments": arguments
                })
            });
            Some(BridgeToolCall {
                id: id.to_string(),
                name: name.to_string(),
                input,
            })
        })
        .collect()
}

fn anthropic_tool_use_response(model: &str, tool_calls: &[BridgeToolCall], usage: &Value) -> Value {
    let content = tool_calls
        .iter()
        .map(|tool_call| {
            serde_json::json!({
                "type": "tool_use",
                "id": tool_call.id,
                "name": tool_call.name,
                "input": tool_call.input
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "id": format!("msg_pfterminal_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": anthropic_response_usage(usage)
    })
}

fn anthropic_response_usage(usage: &Value) -> Value {
    let mut usage_map = serde_json::Map::new();
    usage_map.insert(
        "input_tokens".to_string(),
        Value::from(
            usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
    );
    usage_map.insert(
        "output_tokens".to_string(),
        Value::from(
            usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
    );
    for source in ["cached_tokens", "cache_read_input_tokens"] {
        if let Some(value) = usage.get(source).and_then(Value::as_u64) {
            usage_map.insert("cache_read_input_tokens".to_string(), Value::from(value));
        }
    }
    Value::Object(usage_map)
}

async fn write_json_response(stream: &mut tokio::net::TcpStream, body: Value) -> Result<()> {
    write_json_status_response(stream, 200, body).await
}

async fn write_json_status_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    body: Value,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

async fn write_raw_http_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await?;
    Ok(())
}

async fn write_anthropic_stream_headers(stream: &mut tokio::net::TcpStream) -> Result<()> {
    let response = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n";
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

async fn write_anthropic_stream_start(
    stream: &mut tokio::net::TcpStream,
    model: &str,
    usage: &Value,
) -> Result<()> {
    write_sse_event(
        stream,
        "message_start",
        &anthropic_stream_start_event(model, usage),
    )
    .await
}

async fn write_anthropic_stream_ping(stream: &mut tokio::net::TcpStream) -> Result<()> {
    write_sse_event(stream, "ping", &serde_json::json!({ "type": "ping" })).await
}

async fn write_anthropic_stream_error(
    stream: &mut tokio::net::TcpStream,
    error_type: &str,
    message: &str,
) -> Result<()> {
    write_sse_event(
        stream,
        "error",
        &anthropic_stream_error_event(error_type, message),
    )
    .await
}

fn anthropic_stream_error_event(error_type: &str, message: &str) -> Value {
    serde_json::json!({
        "type": "error",
        "error": {
            "type": error_type,
            "message": message
        }
    })
}

fn anthropic_stream_start_event(model: &str, usage: &Value) -> Value {
    let mut usage_map = serde_json::Map::new();
    usage_map.insert(
        "input_tokens".to_string(),
        Value::from(
            usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
    );
    usage_map.insert("output_tokens".to_string(), Value::from(0_u64));
    for source in ["cached_tokens", "cache_read_input_tokens"] {
        if let Some(value) = usage.get(source).and_then(Value::as_u64) {
            usage_map.insert("cache_read_input_tokens".to_string(), Value::from(value));
        }
    }
    serde_json::json!({
        "type": "message_start",
        "message": {
            "id": format!("msg_pfterminal_{}", Uuid::new_v4().simple()),
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": Value::Object(usage_map)
        }
    })
}

async fn write_anthropic_stream_text_completion(
    stream: &mut tokio::net::TcpStream,
    model: &str,
    text: &str,
    usage: &Value,
) -> Result<()> {
    write_anthropic_stream_start(stream, model, usage).await?;
    write_sse_event(
        stream,
        "content_block_start",
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
    )
    .await?;
    write_sse_event(
        stream,
        "content_block_delta",
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": text }
        }),
    )
    .await?;
    write_sse_event(
        stream,
        "content_block_stop",
        &serde_json::json!({ "type": "content_block_stop", "index": 0 }),
    )
    .await?;
    write_anthropic_stream_stop(stream, "end_turn", usage).await
}

async fn write_anthropic_stream_tool_use_completion(
    stream: &mut tokio::net::TcpStream,
    model: &str,
    tool_calls: &[BridgeToolCall],
    usage: &Value,
) -> Result<()> {
    write_anthropic_stream_start(stream, model, usage).await?;
    for (index, tool_call) in tool_calls.iter().enumerate() {
        let partial_json = tool_call.input.to_string();
        write_sse_event(
            stream,
            "content_block_start",
            &serde_json::json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "tool_use",
                    "id": tool_call.id,
                    "name": tool_call.name,
                    "input": {}
                }
            }),
        )
        .await?;
        write_sse_event(
            stream,
            "content_block_delta",
            &serde_json::json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "input_json_delta", "partial_json": partial_json }
            }),
        )
        .await?;
        write_sse_event(
            stream,
            "content_block_stop",
            &serde_json::json!({ "type": "content_block_stop", "index": index }),
        )
        .await?;
    }
    write_anthropic_stream_stop(stream, "tool_use", usage).await
}

async fn write_anthropic_stream_stop(
    stream: &mut tokio::net::TcpStream,
    stop_reason: &str,
    usage: &Value,
) -> Result<()> {
    write_sse_event(
        stream,
        "message_delta",
        &anthropic_stream_stop_event(stop_reason, usage),
    )
    .await?;
    write_sse_event(
        stream,
        "message_stop",
        &serde_json::json!({ "type": "message_stop" }),
    )
    .await
}

fn anthropic_stream_stop_event(stop_reason: &str, usage: &Value) -> Value {
    serde_json::json!({
        "type": "message_delta",
        "delta": { "stop_reason": stop_reason, "stop_sequence": null },
        "usage": {
            "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
        }
    })
}

async fn write_sse_event(
    stream: &mut tokio::net::TcpStream,
    event: &str,
    data: &Value,
) -> Result<()> {
    let body = format!("event: {event}\ndata: {data}\n\n");
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedClaudeOutput {
    pub(crate) text: String,
    pub(crate) status: ClaudePaneTurnStatus,
    pub(crate) session_id: Option<String>,
    pub(crate) usage_summary: Option<String>,
    pub(crate) terminal_reason: Option<String>,
    pub(crate) error_summary: Option<String>,
    pub(crate) tool_names: Vec<String>,
    pub(crate) tool_events: Vec<ClaudePaneToolEvent>,
    pub(crate) reasoning_events: Vec<ClaudePaneReasoningEvent>,
}

pub(crate) fn parse_claude_output(stdout: &str) -> Result<ParsedClaudeOutput> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Claude returned empty output"));
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return parsed_from_value(&value);
    }

    let mut assistant_chunks = Vec::new();
    let mut final_result = None;
    let mut session_id = None;
    let mut usage_summary = None;
    let mut error_value = None;
    let mut saw_result_event = false;
    let mut tool_names = Vec::new();
    let mut tool_events = Vec::new();
    let mut reasoning_events = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("Claude stream-json line was not valid JSON: {line}"))?;
        if value.get("is_error").and_then(Value::as_bool) == Some(true) {
            error_value = Some(value.clone());
        }
        collect_text_chunks(&value, &mut assistant_chunks);
        collect_reasoning_events(&value, &mut reasoning_events);
        collect_tool_names(&value, &mut tool_names);
        collect_tool_events(&value, &mut tool_events);
        if let Some(result) = value.get("result").and_then(Value::as_str) {
            saw_result_event = true;
            final_result = Some(result.to_string());
        }
        if session_id.is_none() {
            session_id = value
                .get("session_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
        if usage_summary.is_none() {
            usage_summary = usage_summary_from_value(&value);
        }
    }

    if let Some(error_value) = error_value {
        let text = assistant_chunks.join("");
        let status = claude_error_status(&error_value);
        return Ok(ParsedClaudeOutput {
            text,
            status,
            session_id,
            usage_summary,
            terminal_reason: error_value
                .get("terminal_reason")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            error_summary: Some(claude_error_summary(&error_value)),
            tool_names: dedupe_tool_names(tool_names),
            tool_events,
            reasoning_events,
        });
    }

    if !saw_result_event {
        return Err(anyhow!(
            "Claude stream ended before a final result event; the turn is incomplete"
        ));
    }

    let text = final_result
        .filter(|result| !result.trim().is_empty())
        .unwrap_or_else(|| assistant_chunks.join(""));
    if text.trim().is_empty() {
        return Err(anyhow!("Claude returned no assistant text"));
    }
    Ok(ParsedClaudeOutput {
        text,
        status: ClaudePaneTurnStatus::Success,
        session_id,
        usage_summary,
        terminal_reason: None,
        error_summary: None,
        tool_names: dedupe_tool_names(tool_names),
        tool_events,
        reasoning_events,
    })
}

fn parsed_from_value(value: &Value) -> Result<ParsedClaudeOutput> {
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        let mut tool_names = Vec::new();
        let mut tool_events = Vec::new();
        let mut reasoning_events = Vec::new();
        collect_reasoning_events(value, &mut reasoning_events);
        collect_tool_names(value, &mut tool_names);
        collect_tool_events(value, &mut tool_events);
        return Ok(ParsedClaudeOutput {
            text: String::new(),
            status: claude_error_status(value),
            session_id: value
                .get("session_id")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            usage_summary: usage_summary_from_value(value),
            terminal_reason: value
                .get("terminal_reason")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            error_summary: Some(claude_error_summary(value)),
            tool_names: dedupe_tool_names(tool_names),
            tool_events,
            reasoning_events,
        });
    }
    let mut assistant_chunks = Vec::new();
    collect_text_chunks(value, &mut assistant_chunks);
    let mut tool_names = Vec::new();
    let mut tool_events = Vec::new();
    let mut reasoning_events = Vec::new();
    collect_reasoning_events(value, &mut reasoning_events);
    collect_tool_names(value, &mut tool_names);
    collect_tool_events(value, &mut tool_events);
    let text = value
        .get("result")
        .and_then(Value::as_str)
        .filter(|result| !result.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| assistant_chunks.join(""));
    if text.trim().is_empty() {
        return Err(anyhow!("Claude returned no assistant text"));
    }
    Ok(ParsedClaudeOutput {
        text,
        status: ClaudePaneTurnStatus::Success,
        session_id: value
            .get("session_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        usage_summary: usage_summary_from_value(value),
        terminal_reason: None,
        error_summary: None,
        tool_names: dedupe_tool_names(tool_names),
        tool_events,
        reasoning_events,
    })
}

fn claude_error_status(value: &Value) -> ClaudePaneTurnStatus {
    let subtype = value.get("subtype").and_then(Value::as_str);
    let terminal_reason = value.get("terminal_reason").and_then(Value::as_str);
    if subtype == Some("error_max_turns") || terminal_reason == Some("max_turns") {
        ClaudePaneTurnStatus::MaxTurnsPause
    } else {
        ClaudePaneTurnStatus::ProviderError
    }
}

fn claude_error_summary(value: &Value) -> String {
    let subtype = value
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let terminal_reason = value
        .get("terminal_reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let errors = value
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .take(3)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|errors| !errors.is_empty())
        .or_else(|| {
            value
                .get("result")
                .and_then(Value::as_str)
                .filter(|result| !result.trim().is_empty())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "no error details".to_string());
    format!("{subtype}; terminal_reason={terminal_reason}; {errors}")
}

fn collect_text_chunks(value: &Value, chunks: &mut Vec<String>) {
    if let Some(text) = value.get("text").and_then(Value::as_str)
        && value.get("type").and_then(Value::as_str) == Some("text")
    {
        chunks.push(text.to_string());
    }
    if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
        for item in content {
            if item.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = item.get("text").and_then(Value::as_str)
            {
                chunks.push(text.to_string());
            }
        }
    }
    if let Some(delta_text) = value.pointer("/delta/text").and_then(Value::as_str) {
        chunks.push(delta_text.to_string());
    }
}

fn collect_reasoning_events(value: &Value, events: &mut Vec<ClaudePaneReasoningEvent>) {
    if value.get("type").and_then(Value::as_str) == Some("thinking")
        && let Some(preview) = reasoning_preview_from_value(value)
    {
        events.push(ClaudePaneReasoningEvent { preview });
    }
    if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
        for item in content {
            if item.get("type").and_then(Value::as_str) == Some("thinking")
                && let Some(preview) = reasoning_preview_from_value(item)
            {
                events.push(ClaudePaneReasoningEvent { preview });
            }
        }
    }
    if let Some(delta) = value.pointer("/delta/thinking").and_then(Value::as_str) {
        let preview = summarize_reasoning_text(delta);
        if !preview.is_empty() {
            events.push(ClaudePaneReasoningEvent { preview });
        }
    }
}

fn reasoning_preview_from_value(value: &Value) -> Option<String> {
    let text = string_field(value, &["thinking", "text", "content"])?;
    let preview = summarize_reasoning_text(text);
    (!preview.is_empty()).then_some(preview)
}

fn collect_tool_names(value: &Value, tool_names: &mut Vec<String>) {
    if let Some(name) = value.get("name").and_then(Value::as_str)
        && value.get("type").and_then(Value::as_str) == Some("tool_use")
    {
        tool_names.push(name.to_string());
    }
    if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
        for item in content {
            if item.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(name) = item.get("name").and_then(Value::as_str)
            {
                tool_names.push(name.to_string());
            }
        }
    }
}

fn collect_tool_events(value: &Value, tool_events: &mut Vec<ClaudePaneToolEvent>) {
    if value.get("type").and_then(Value::as_str) == Some("tool_use")
        && let Some(name) = value.get("name").and_then(Value::as_str)
    {
        let preview = value
            .get("input")
            .map(|input| summarize_tool_call_input(name, input))
            .unwrap_or_default();
        tool_events.push(ClaudePaneToolEvent {
            name: name.to_string(),
            preview,
        });
    }
    if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
        for item in content {
            if item.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(name) = item.get("name").and_then(Value::as_str)
            {
                let preview = item
                    .get("input")
                    .map(|input| summarize_tool_call_input(name, input))
                    .unwrap_or_default();
                tool_events.push(ClaudePaneToolEvent {
                    name: name.to_string(),
                    preview,
                });
            }
        }
    }
}

const TOOL_PREVIEW_MAX_CHARS: usize = 120;
const REASONING_PREVIEW_MAX_CHARS: usize = 240;
const ASSISTANT_UPDATE_MAX_CHARS: usize = 220;
const CLAUDE_TOOL_CALL_PREFIX: &str = "Claude tool call: ";
const CLAUDE_REASONING_PREFIX: &str = "Claude reasoning: ";
const SEND_TASK_FENCE_OPEN_MARKER: &str = "```pfterminal-send-task";
const SEND_TASK_FENCE_CLOSE_MARKER: &str = "```";
const SEND_TASK_XML_OPEN_MARKER: &str = "<pfterminal_send_task";
const SEND_TASK_XML_CLOSE_MARKER: &str = "</pfterminal_send_task>";

fn summarize_reasoning_text(text: &str) -> String {
    truncate_for_display(
        &collapse_whitespace(text.trim()),
        REASONING_PREVIEW_MAX_CHARS,
    )
}

fn assistant_update_blurbs_from_buffer(buffer: &str) -> Vec<String> {
    let stable_text = strip_incomplete_spawn_dispatch_tail(buffer);
    let (visible, _) = crate::spawn_orchestration::extract_spawn_task_dispatches(stable_text);
    let mut blurbs = Vec::new();
    let mut paragraph = String::new();

    for raw_line in visible.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            push_assistant_update_blurb(&mut blurbs, &mut paragraph);
            continue;
        }
        if line.starts_with("```") {
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(line);
    }
    push_assistant_update_blurb(&mut blurbs, &mut paragraph);
    blurbs
}

fn push_assistant_update_blurb(blurbs: &mut Vec<String>, paragraph: &mut String) {
    let compact = collapse_whitespace(paragraph.trim());
    paragraph.clear();
    if compact.is_empty() {
        return;
    }
    let chars = compact.chars().collect::<Vec<_>>();
    for chunk in chars.chunks(ASSISTANT_UPDATE_MAX_CHARS) {
        let blurb = chunk.iter().collect::<String>();
        if blurbs.last() != Some(&blurb) {
            blurbs.push(blurb);
        }
    }
}

fn strip_incomplete_spawn_dispatch_tail(text: &str) -> &str {
    let mut end = text.len();
    if let Some(index) = text.rfind(SEND_TASK_FENCE_OPEN_MARKER) {
        let tail = &text[index..];
        if !tail
            .get(SEND_TASK_FENCE_OPEN_MARKER.len()..)
            .is_some_and(|rest| rest.contains(SEND_TASK_FENCE_CLOSE_MARKER))
        {
            end = end.min(index);
        }
    }
    if let Some(index) = text.rfind(SEND_TASK_XML_OPEN_MARKER) {
        let tail = &text[index..];
        if !tail.contains(SEND_TASK_XML_CLOSE_MARKER) {
            end = end.min(index);
        }
    }
    &text[..end]
}

fn summarize_tool_call_input(name: &str, input: &Value) -> String {
    if let Some(description) = string_field(input, &["description"]) {
        let description = collapse_whitespace(description);
        if !description.is_empty() {
            return truncate_for_display(&description, TOOL_PREVIEW_MAX_CHARS);
        }
    }

    let lower_name = name.to_ascii_lowercase();
    let summary = match lower_name.as_str() {
        "bash" | "shell" => summarize_bash_input(input),
        "read" => summarize_path_tool("reading", input),
        "write" => summarize_path_tool("writing", input),
        "edit" | "multiedit" => summarize_path_tool("editing", input),
        "ls" | "list" => summarize_path_tool("listing", input),
        "grep" => summarize_grep_input(input),
        "glob" => string_field(input, &["pattern"]).map(|pattern| {
            format!(
                "matching {}",
                truncate_for_display(&collapse_whitespace(pattern), 90)
            )
        }),
        "webfetch" => summarize_path_tool("fetching", input),
        "websearch" => string_field(input, &["query"]).map(|query| {
            format!(
                "searching {}",
                truncate_for_display(&collapse_whitespace(query), 90)
            )
        }),
        "todowrite" => Some("updating todo list".to_string()),
        _ => summarize_generic_tool_input(input),
    };

    summary
        .map(|value| truncate_for_display(&value, TOOL_PREVIEW_MAX_CHARS))
        .unwrap_or_else(|| "running tool".to_string())
}

fn summarize_bash_input(input: &Value) -> Option<String> {
    let command = string_field(input, &["command", "cmd", "script"])?;
    summarize_bash_command(command)
}

fn summarize_bash_command(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    if let Some(target) = shell_write_target(command) {
        return Some(format!("writing {}", compact_shell_target(&target)));
    }

    if let Some(target) = shell_mkdir_target(command) {
        return Some(format!("creating directory {}", compact_tool_path(&target)));
    }

    first_meaningful_shell_fragment(command).map(|fragment| {
        truncate_for_display(&collapse_whitespace(&fragment), TOOL_PREVIEW_MAX_CHARS)
    })
}

fn summarize_path_tool(verb: &str, input: &Value) -> Option<String> {
    let path = string_field(
        input,
        &["file_path", "path", "notebook_path", "url", "directory"],
    )?;
    Some(format!("{verb} {}", compact_tool_path(path)))
}

fn summarize_grep_input(input: &Value) -> Option<String> {
    let pattern = string_field(input, &["pattern", "query"])?;
    if let Some(path) = string_field(input, &["path", "directory"]) {
        return Some(format!(
            "searching {} in {}",
            truncate_for_display(&collapse_whitespace(pattern), 60),
            compact_tool_path(path)
        ));
    }
    Some(format!(
        "searching {}",
        truncate_for_display(&collapse_whitespace(pattern), 90)
    ))
}

fn summarize_generic_tool_input(input: &Value) -> Option<String> {
    if let Some(path_summary) = summarize_path_tool("using", input) {
        return Some(path_summary);
    }
    if let Some(value) = input.as_str() {
        let value = collapse_whitespace(value);
        if !value.is_empty() {
            return Some(value);
        }
    }
    let object = input.as_object()?;
    let fields = object
        .keys()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if fields.is_empty() {
        None
    } else {
        Some(format!("input fields: {fields}"))
    }
}

fn string_field<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(value) = input.get(*key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn shell_write_target(command: &str) -> Option<String> {
    for line in command
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(target) = extract_cat_write_target(line) {
            return Some(target);
        }
        if let Some(target) = extract_tee_write_target(line) {
            return Some(target);
        }
        if let Some(target) = extract_redirection_target(line) {
            return Some(target);
        }
        if line.contains("<<") {
            break;
        }
    }
    None
}

fn extract_cat_write_target(line: &str) -> Option<String> {
    if let Some(index) = line.find("cat >") {
        return first_shell_token(&line[index + "cat >".len()..])
            .filter(|target| is_useful_shell_target(target));
    }
    if line.starts_with("cat <<")
        && let Some(index) = line.rfind('>')
    {
        return first_shell_token(&line[index + 1..])
            .filter(|target| is_useful_shell_target(target));
    }
    None
}

fn extract_tee_write_target(line: &str) -> Option<String> {
    let index = line.find("tee ")?;
    let after = &line[index + "tee ".len()..];
    for token in after.split_whitespace() {
        if token.starts_with('-') {
            continue;
        }
        let token = clean_shell_token(token);
        if is_useful_shell_target(&token) {
            return Some(token);
        }
    }
    None
}

fn extract_redirection_target(line: &str) -> Option<String> {
    let mut target = None;
    for (index, _) in line.match_indices('>') {
        if let Some(token) = first_shell_token(&line[index + 1..])
            && is_useful_shell_target(&token)
        {
            target = Some(token);
        }
    }
    target
}

fn shell_mkdir_target(command: &str) -> Option<String> {
    for line in command
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(index) = line.find("mkdir ") {
            let after = &line[index + "mkdir ".len()..];
            for token in after.split_whitespace() {
                if token.starts_with('-') {
                    continue;
                }
                let token = clean_shell_token(token);
                if is_useful_shell_target(&token) {
                    return Some(token);
                }
            }
        }
    }
    None
}

fn first_meaningful_shell_fragment(command: &str) -> Option<String> {
    let line = command
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let fragment = line
        .split("&&")
        .next()
        .unwrap_or(line)
        .split(';')
        .next()
        .unwrap_or(line)
        .trim();
    if fragment.is_empty() {
        None
    } else {
        Some(fragment.to_string())
    }
}

fn first_shell_token(value: &str) -> Option<String> {
    let value = value.trim_start();
    let mut chars = value.chars();
    let quote = match chars.next()? {
        '"' => Some('"'),
        '\'' => Some('\''),
        _ => None,
    };
    let mut token = String::new();
    if let Some(quote) = quote {
        for ch in chars {
            if ch == quote {
                break;
            }
            token.push(ch);
        }
    } else {
        for ch in value.chars() {
            if ch.is_whitespace() || matches!(ch, ';' | '|' | '<' | '>') {
                break;
            }
            token.push(ch);
        }
    }
    let token = clean_shell_token(&token);
    if token.is_empty() { None } else { Some(token) }
}

fn clean_shell_token(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches(';')
        .to_string()
}

fn is_useful_shell_target(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value != "/dev/null"
        && value != "&1"
        && value != "&2"
        && value != "1"
        && value != "2"
        && !value.starts_with('$')
}

fn compact_tool_path(path: &str) -> String {
    let path = collapse_whitespace(path);
    if path.chars().count() <= 90 {
        return path;
    }
    Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| truncate_for_display(&path, 90))
}

fn compact_claude_pane_metadata(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = compact.chars().take(max_chars).collect::<String>();
    if compact.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn compact_shell_target(path: &str) -> String {
    let path = collapse_whitespace(path);
    Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| compact_tool_path(&path))
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tool_events_from_stdout(stdout: &str) -> Vec<ClaudePaneToolEvent> {
    let mut tool_events = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            collect_tool_events(&value, &mut tool_events);
        }
    }
    tool_events
}

fn reasoning_events_from_stdout(stdout: &str) -> Vec<ClaudePaneReasoningEvent> {
    let mut reasoning_events = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            collect_reasoning_events(&value, &mut reasoning_events);
        }
    }
    reasoning_events
}

fn dedupe_tool_names(tool_names: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for name in tool_names {
        if !deduped.iter().any(|existing| existing == &name) {
            deduped.push(name);
        }
    }
    deduped
}

fn usage_summary_from_value(value: &Value) -> Option<String> {
    let usage = value
        .get("usage")
        .or_else(|| value.pointer("/message/usage"))?;
    if !usage.is_object() {
        return None;
    }
    Some(usage.to_string())
}

fn usage_status_from_summary(summary: Option<&str>) -> ClaudePaneUsageStatus {
    let Some(summary) = summary else {
        return ClaudePaneUsageStatus::Missing;
    };
    let Ok(value) = serde_json::from_str::<Value>(summary) else {
        return ClaudePaneUsageStatus::Unknown;
    };
    let Some(object) = value.as_object() else {
        return ClaudePaneUsageStatus::Unknown;
    };
    let mut saw_numeric = false;
    let mut saw_output_metric = false;
    let mut saw_positive_output_metric = false;
    for (key, value) in object {
        if let Some(number) = value.as_u64() {
            saw_numeric = true;
            if matches!(
                key.as_str(),
                "output_tokens" | "completion_tokens" | "completion"
            ) {
                saw_output_metric = true;
                if number > 0 {
                    saw_positive_output_metric = true;
                }
            }
        }
    }
    if saw_positive_output_metric {
        ClaudePaneUsageStatus::Reported
    } else if saw_numeric || saw_output_metric {
        ClaudePaneUsageStatus::Untrusted
    } else {
        ClaudePaneUsageStatus::Unknown
    }
}

fn truncate_for_display(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn elapsed_ms(started_at: &Instant) -> i64 {
    i64::try_from(started_at.elapsed().as_millis()).unwrap_or(i64::MAX)
}

fn format_elapsed_ms(elapsed_ms: i64) -> String {
    let total_seconds = (elapsed_ms.max(0) / 1_000).max(0);
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn tool_blurb_from_progress(progress: &ClaudePaneTurnProgress) -> String {
    progress
        .summary
        .strip_prefix(CLAUDE_TOOL_CALL_PREFIX)
        .unwrap_or(progress.summary.as_str())
        .trim()
        .to_string()
}

fn reasoning_blurb_from_progress(progress: &ClaudePaneTurnProgress) -> String {
    progress
        .summary
        .strip_prefix(CLAUDE_REASONING_PREFIX)
        .unwrap_or(progress.summary.as_str())
        .trim()
        .to_string()
}

fn progress_status_text(progress: &ClaudePaneTurnProgress) -> String {
    match progress.phase.as_str() {
        "system" => "session initialized".to_string(),
        "assistant-result" => "finalizing result".to_string(),
        "waiting" => "waiting for Claude".to_string(),
        "error" => progress.summary.trim().to_string(),
        "tool-call" => tool_blurb_from_progress(progress),
        "reasoning" | "reasoning-tokens" => reasoning_blurb_from_progress(progress),
        _ => progress.summary.trim().to_string(),
    }
}

fn thinking_tokens_progress(
    plan: &ClaudeCommandPlan,
    started_at: &Instant,
    value: &Value,
) -> Option<ClaudePaneTurnProgress> {
    let estimated_tokens = value.get("estimated_tokens").and_then(Value::as_u64)?;
    let bucket = thinking_tokens_progress_bucket(estimated_tokens);
    Some(ClaudePaneTurnProgress {
        pane_id: plan.pane_id.clone(),
        phase: "reasoning-tokens".to_string(),
        summary: format!(
            "{}thinking: {} reasoning tokens",
            CLAUDE_REASONING_PREFIX,
            format_reasoning_token_count(estimated_tokens)
        ),
        assistant_text_delta: None,
        hint: Some(format!("thinking-token-bucket:{bucket}")),
        elapsed_ms: elapsed_ms(started_at),
        artifact_path: plan.artifact_path.clone(),
        audit_path: plan.audit_path.clone(),
    })
}

fn thinking_tokens_progress_bucket(tokens: u64) -> u64 {
    if tokens < 100 {
        tokens / 10
    } else {
        tokens / 100
    }
}

fn format_reasoning_token_count(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else {
        let tenths = tokens / 100;
        format!("{}.{}K", tenths / 10, tenths % 10)
    }
}

fn unix_epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn emit_claude_progress(
    progress_tx: &Option<AppEventSender>,
    plan: &ClaudeCommandPlan,
    started_at: &Instant,
    phase: &str,
    summary: &str,
    hint: Option<String>,
) {
    if let Some(tx) = progress_tx.as_ref() {
        tx.send(AppEvent::ClaudePaneTurnProgress {
            progress: ClaudePaneTurnProgress {
                pane_id: plan.pane_id.clone(),
                phase: phase.to_string(),
                summary: summary.to_string(),
                assistant_text_delta: None,
                hint,
                elapsed_ms: elapsed_ms(started_at),
                artifact_path: plan.artifact_path.clone(),
                audit_path: plan.audit_path.clone(),
            },
        });
    }
}

#[cfg(test)]
fn progress_from_claude_value(
    plan: &ClaudeCommandPlan,
    started_at: &Instant,
    value: &Value,
) -> Option<ClaudePaneTurnProgress> {
    progresses_from_claude_value(plan, started_at, value)
        .into_iter()
        .next()
}

fn progresses_from_claude_value(
    plan: &ClaudeCommandPlan,
    started_at: &Instant,
    value: &Value,
) -> Vec<ClaudePaneTurnProgress> {
    let mut progresses = Vec::new();
    let value_type = value.get("type").and_then(Value::as_str).unwrap_or("event");
    if value_type == "system"
        && value.get("subtype").and_then(Value::as_str) == Some("thinking_tokens")
    {
        if let Some(progress) = thinking_tokens_progress(plan, started_at, value) {
            progresses.push(progress);
        }
        return progresses;
    }
    let mut text_chunks = Vec::new();
    collect_text_chunks(value, &mut text_chunks);
    for chunk in text_chunks {
        progresses.push(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "assistant-text".to_string(),
            summary: "Claude assistant text.".to_string(),
            assistant_text_delta: Some(chunk),
            hint: None,
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
    }
    let mut reasoning_events = Vec::new();
    collect_reasoning_events(value, &mut reasoning_events);
    for event in reasoning_events {
        progresses.push(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "reasoning".to_string(),
            summary: format!("{}{}", CLAUDE_REASONING_PREFIX, event.preview.trim()),
            assistant_text_delta: None,
            hint: None,
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
    }
    let mut tool_events = Vec::new();
    collect_tool_events(value, &mut tool_events);
    for event in tool_events {
        let summary = if event.preview.trim().is_empty() {
            format!("{}{}", CLAUDE_TOOL_CALL_PREFIX, event.name)
        } else {
            format!(
                "{}{}: {}",
                CLAUDE_TOOL_CALL_PREFIX,
                event.name,
                event.preview.trim()
            )
        };
        progresses.push(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "tool-call".to_string(),
            summary,
            assistant_text_delta: None,
            hint: None,
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
    }
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        progresses.push(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "error".to_string(),
            summary: "Claude reported an error result.".to_string(),
            assistant_text_delta: None,
            hint: Some(claude_error_summary(value)),
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
        return progresses;
    }
    if value.get("result").and_then(Value::as_str).is_some() {
        progresses.push(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "assistant-result".to_string(),
            summary: "Claude returned a result.".to_string(),
            assistant_text_delta: None,
            hint: None,
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
        return progresses;
    }
    if value_type == "system" && value.get("subtype").and_then(Value::as_str) == Some("init") {
        progresses.push(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "system".to_string(),
            summary: "Claude session initialized.".to_string(),
            assistant_text_delta: None,
            hint: value
                .get("session_id")
                .and_then(Value::as_str)
                .map(|session_id| format!("session_id: {session_id}")),
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
    }
    progresses
}

fn progress_key(progress: &ClaudePaneTurnProgress) -> String {
    if progress.phase == "reasoning-tokens" {
        return format!(
            "{}\n{}",
            progress.phase,
            progress
                .hint
                .as_deref()
                .unwrap_or(progress.summary.as_str())
        );
    }
    format!(
        "{}\n{}\n{}",
        progress.phase,
        progress.summary,
        progress.hint.as_deref().unwrap_or_default()
    )
}

impl App {
    pub(super) fn open_pane_picker(&mut self) {
        let mut items = Vec::new();
        items.push(section_item("User Panes"));
        items.extend(self.user_pane_items());
        items.push(section_item("New Pane"));
        items.extend(new_pane_items());
        items.push(section_item("Agent Panes"));
        items.extend(self.spawn_tree_items(/*show_task_actions*/ false));

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

    pub(super) fn open_claude_pane_profile_picker(&mut self) {
        let mut items = Vec::new();
        for profile in ClaudeProviderProfileKind::creation_options() {
            let profile_config = profile.profile();
            let kind = *profile;
            items.push(SelectionItem {
                name: format!("+ {}", profile_config.title),
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

    pub(super) fn open_codex_pane_model_picker(&mut self) {
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

    pub(super) async fn select_user_pane(&mut self, tui: &mut tui::Tui, pane_id: String) {
        self.save_active_claude_pane_transcript();
        match self.claude_panes.set_active_user_pane(&pane_id) {
            Ok(()) if pane_id == CODEX_MAIN_PANE_ID => {
                self.sync_external_pane_turn_display(&pane_id);
                self.sync_active_agent_label();
            }
            Ok(()) => {
                self.detach_active_thread_for_external_pane().await;
                if let Err(err) = self.restore_claude_pane_transcript(tui, &pane_id) {
                    self.chat_widget
                        .add_error_message(format!("Failed to switch Claude pane display: {err}"));
                }
                self.sync_external_pane_turn_display(&pane_id);
                self.sync_active_agent_label();
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

    pub(super) async fn create_claude_pane(
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
                self.chat_widget.add_info_message(
                    format!("Created and switched to {title}."),
                    Some("Type normally; turns will run through Claude Code headless.".to_string()),
                );
                tracing::info!(pane_id = %id, profile = ?profile, "created Claude headless pane");
            }
            Err(err) => self.chat_widget.add_error_message(err.to_string()),
        }
    }

    pub(super) async fn create_spawn_claude_pane(
        &mut self,
        tui: &mut tui::Tui,
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
        self.save_active_claude_pane_transcript();
        let spawn_nickname = self.next_spawn_agent_nickname(role);
        match self.claude_panes.create_pane_with_role(
            profile,
            self.config.cwd.to_path_buf(),
            self.config.codex_home.as_ref(),
            Some(role),
            spawn_nickname.clone(),
        ) {
            Ok(id) => {
                self.detach_active_thread_for_external_pane().await;
                self.claude_pane_transcript_cells
                    .entry(id.clone())
                    .or_default();
                if let Err(err) = self.restore_claude_pane_transcript(tui, &id) {
                    self.chat_widget.add_error_message(format!(
                        "Failed to initialize Claude spawn pane display: {err}"
                    ));
                }
                let title = claude_pane_title(profile, Some(role), spawn_nickname.as_deref());
                let logical_parent_node_id =
                    self.logical_parent_node_for_spawn(role, parent_node_id.as_deref());
                self.spawn_parent_by_node.insert(
                    crate::spawn_orchestration::pane_node_id(&id),
                    logical_parent_node_id,
                );
                self.sync_active_agent_label();
                self.chat_widget.add_info_message(
                    format!("Created and switched to {title}."),
                    Some(format!(
                        "Harness: Claude Code; role: {}; no task was started.",
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

    pub(super) fn try_submit_active_claude_pane_op(&mut self, op: &AppCommand) -> bool {
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

    pub(super) fn submit_claude_pane_task(&mut self, pane_id: String, task: String) {
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
        let prompt = compose_claude_pane_prompt(task, prompt_context.as_deref());
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

    pub(super) fn on_claude_pane_turn_progress(&mut self, progress: ClaudePaneTurnProgress) {
        if let Some(delta) = progress.assistant_text_delta.as_deref() {
            let dispatches = self
                .claude_panes
                .collect_spawn_dispatches_from_assistant_delta(&progress.pane_id, delta);
            if !dispatches.is_empty() {
                self.dispatch_spawn_task_blocks(&progress.pane_id, dispatches);
            }
        }
        if self.claude_panes.active_user_pane_id() != progress.pane_id {
            return;
        }
        if let Some(status) = self.claude_panes.update_live_progress(&progress) {
            self.chat_widget
                .update_external_pane_live_status(status.header, status.details);
        }
    }

    pub(super) fn on_claude_pane_turn_finished(
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
                let dispatches = self
                    .claude_panes
                    .filter_new_spawn_dispatches(&pane_id, dispatches);
                self.claude_panes.finish_turn(&pane_id, &Ok(output.clone()));
                if !dispatches.is_empty() {
                    self.dispatch_spawn_task_blocks(&pane_id, dispatches);
                }
                if !output.text.trim().is_empty() {
                    if is_active {
                        self.chat_widget
                            .append_external_pane_response(output.text.clone());
                    } else {
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

    pub(super) fn next_codex_pane_nickname(&self) -> String {
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

fn new_pane_items() -> Vec<SelectionItem> {
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

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

        let progress =
            progress_from_claude_value(&plan, &started_at, &value).expect("tool progress");
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
        let plan =
            build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
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
        let plan =
            build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
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
        let plan =
            build_claude_command_plan(&pane, "dispatch".to_string(), dir.path()).expect("plan");
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
            "Current: Claude: Let me trace the allow flags and wrap_owned relationship."
        ));
        assert!(details.contains("Updates:"));
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
            details.contains(
                "Current: Bash: Create directory for the mock donkey riding course website"
            )
        );
        assert!(
            details.contains(
                "running Bash: Create directory for the mock donkey riding course website"
            )
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
            details.contains(
                "Current: Bash: Create directory for the mock donkey riding course website"
            )
        );
        assert!(!details.contains("Claude pane still running"));

        let second_tool = ClaudePaneTurnProgress {
            pane_id: pane_id.clone(),
            phase: "tool-call".to_string(),
            summary:
                "Claude tool call: Bash: Write the donkey riding course mock website HTML file"
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
            details.contains(
                "done    Bash: Create directory for the mock donkey riding course website"
            )
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
            details.contains(
                "Current: reasoning: Inspect the hierarchy before asking Orcs to execute."
            )
        );
        assert!(details.contains("Reasoning:"));
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
        assert!(details.contains("Reasoning:"));
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
            settings
                .pointer("/env/ANTHROPIC_BASE_URL")
                .and_then(Value::as_str)
                .is_some_and(|base_url| base_url.starts_with("http://127.0.0.1:"))
        );
        assert!(!plan.args.iter().any(|arg| arg.contains("vercel-test-key")));
        assert!(!settings.to_string().contains("vercel-test-key"));
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
    fn ambient_profile_is_first_creation_option() {
        assert_eq!(
            ClaudeProviderProfileKind::creation_options()
                .first()
                .copied(),
            Some(ClaudeProviderProfileKind::AmbientGlm52)
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
            cwd: dir.path().to_path_buf(),
            pane_id: "claude-test".to_string(),
            pane_title: "Claude Test".to_string(),
            profile_title: "Claude Test".to_string(),
            provider_model: "test-model".to_string(),
            turn_index: 1,
            command_mode: ClaudeCommandMode::NewSession,
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
        assert!(artifact_path.exists());
        assert!(audit_path.exists());
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
            Some(
                "<pfterminal_spawn_context>\nTrolls: none spawned yet.\n</pfterminal_spawn_context>",
            ),
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
            "Claude Code - Claude Plan"
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
        assert_eq!(pane.title, "Claude Code Burzum [troll] - Claude Plan");
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
        let plan =
            build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
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
        let plan =
            build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
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
        let plan =
            build_claude_command_plan(&pane, "review".to_string(), dir.path()).expect("plan");
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
            "zai-org/GLM-5.2-FP8",
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
            "zai-org/GLM-5.2-FP8",
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
}
