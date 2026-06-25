use std::collections::BTreeMap;
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
use codex_protocol::ThreadId;
use codex_vault::Vault;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::OwnedMutexGuard;
use tokio::time::MissedTickBehavior;
use tokio::time::interval;
use uuid::Uuid;

use crate::app::App;
use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::multi_agents::AgentPickerThreadEntry;
use crate::multi_agents::agent_picker_status_dot_spans;
use crate::multi_agents::format_agent_picker_item_name;

pub(crate) const CODEX_MAIN_PANE_ID: &str = "codex-main";
const CLAUDE_PANE_TURN_TIMEOUT: Duration = Duration::from_secs(600);
const CLAUDE_PANE_MAX_TURNS: &str = "80";
const AMBIENT_BRIDGE_TOOL_CALL_BUDGET: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ClaudeProviderProfileKind {
    ClaudePlan,
    AmbientGlm52,
    ZaiGlm52,
    BasetenGlm52,
    OpenRouterGlm52,
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
        }
    }

    pub(crate) fn creation_options() -> &'static [Self] {
        &[
            Self::AmbientGlm52,
            Self::ZaiGlm52,
            Self::BasetenGlm52,
            Self::OpenRouterGlm52,
            Self::ClaudePlan,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeProviderTransport {
    DirectAnthropic,
    AmbientChatBridge,
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
    ProviderError,
    ParseFailure,
}

impl ClaudePaneTurnStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::MaxTurnsPause => "max-turn-pause",
            Self::TimeoutPause => "timeout-pause",
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
    pub(crate) cwd: PathBuf,
    pub(crate) claude_session_id: Option<String>,
    pub(crate) status: ClaudePaneStatus,
    pub(crate) latest_usage_summary: Option<String>,
    pub(crate) latest_usage_status: Option<ClaudePaneUsageStatus>,
    pub(crate) latest_turn_status: Option<ClaudePaneTurnStatus>,
    pub(crate) latest_audit_path: Option<PathBuf>,
    pub(crate) artifact_dir: PathBuf,
    lock: Arc<Mutex<()>>,
    next_turn_index: u64,
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
        let profile_config = profile.profile();
        if let Some(label) = profile_config.vault_label {
            ensure_vault_label_exists(codex_home, label)?;
        }

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
            title: profile_config.title.to_string(),
            profile,
            cwd,
            claude_session_id: None,
            status: ClaudePaneStatus::Idle,
            latest_usage_summary: None,
            latest_usage_status: None,
            latest_turn_status: None,
            latest_audit_path: None,
            artifact_dir,
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
        pane.status = ClaudePaneStatus::Running;
        Ok(PreparedClaudePaneTurn {
            pane_id: pane.id.clone(),
            plan,
            _lock: lock,
        })
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
        if let Ok(output) = result {
            if let Some(session_id) = &output.session_id {
                pane.claude_session_id = Some(session_id.clone());
            }
            pane.latest_usage_summary = output.usage_summary.clone();
            pane.latest_usage_status = Some(output.usage_status);
            pane.latest_turn_status = Some(output.status);
            pane.latest_audit_path = Some(output.audit_path.clone());
            pane.next_turn_index = pane.next_turn_index.saturating_add(1);
        }
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
    pub(crate) command_mode: ClaudeCommandMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClaudePaneToolEvent {
    pub(crate) name: String,
    pub(crate) preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClaudePaneTurnProgress {
    pub(crate) pane_id: String,
    pub(crate) phase: String,
    pub(crate) summary: String,
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
    max_turns: String,
    artifact_path: PathBuf,
    audit_path: PathBuf,
    timeout_ms: u64,
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
    max_turns: String,
    artifact_path: PathBuf,
    audit_path: PathBuf,
    timeout_ms: u64,
    started_at_unix_ms: u128,
    ended_at_unix_ms: u128,
    last_progress_elapsed_ms: Option<i64>,
    duration_ms: i64,
    usage: Option<Value>,
    usage_status: ClaudePaneUsageStatus,
    terminal_reason: Option<String>,
    status: ClaudePaneTurnStatus,
    error_summary: Option<String>,
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
            "status: {}; mode: {}; {tools}{terminal}{usage}; artifact: {}; audit: {}",
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
    listener: StdTcpListener,
    bind_addr: SocketAddr,
    upstream_api_key: String,
    upstream_model: String,
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
    let timeout_ms = u64::try_from(CLAUDE_PANE_TURN_TIMEOUT.as_millis()).unwrap_or(u64::MAX);
    let mut bridge = None;
    let mut base_url_override = None;
    if profile.transport == ClaudeProviderTransport::AmbientChatBridge {
        let Some(label) = profile.vault_label else {
            return Err(anyhow!(
                "Ambient Claude bridge requires a provider vault label"
            ));
        };
        let secret = reveal_provider_secret(codex_home, label)?;
        let listener = StdTcpListener::bind("127.0.0.1:0")
            .context("failed to bind Ambient Claude bridge loopback listener")?;
        listener
            .set_nonblocking(true)
            .context("failed to set Ambient Claude bridge listener nonblocking")?;
        let bind_addr = listener
            .local_addr()
            .context("failed to read Ambient Claude bridge listener address")?;
        base_url_override = Some(format!("http://{bind_addr}"));
        bridge = Some(ClaudeBridgePlan {
            listener,
            bind_addr,
            upstream_api_key: secret,
            upstream_model: "zai-org/GLM-5.2-FP8".to_string(),
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
        "--max-turns".to_string(),
        CLAUDE_PANE_MAX_TURNS.to_string(),
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
        args.push(pane.id.trim_start_matches("claude-").to_string());
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
        max_turns: CLAUDE_PANE_MAX_TURNS.to_string(),
        artifact_path,
        audit_path,
        timeout_ms,
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

pub(crate) async fn run_prepared_claude_turn(
    prepared: PreparedClaudePaneTurn,
    progress_tx: Option<AppEventSender>,
) -> Result<ClaudePaneTurnOutput, String> {
    run_claude_command_plan(prepared.plan, progress_tx)
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
    let provider_names = if options.providers.is_empty() {
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

    let passed = entries
        .iter()
        .any(|entry| entry.status == "passed" && entry.provider == "ambient");
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
        "Perform a read-only code review of codex-rs/tui/src/app_event_sender.rs in this repo. ",
        "Use filesystem tools to inspect that file and, if needed, one nearby caller. ",
        "Keep the review bounded and stop after you have enough evidence. ",
        "Return marker PFT_CODE_REVIEW_DONE and either concrete findings with file references ",
        "or say no findings with a short rationale. Do not edit files."
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
        && first_output
            .text
            .contains("codex-rs/tui/src/app_event_sender.rs");
    if !(first_output.status.is_success() && has_review && !first_output.tool_names.is_empty()) {
        return workflow_entry_from_output(
            provider_name,
            profile,
            workflow,
            first_output,
            None,
            "fresh code review verification failed".to_string(),
        );
    }

    let resume_prompt = concat!(
        "Continue the same read-only code review. Use the context already gathered. ",
        "Use at most one more filesystem tool only if necessary. Return marker ",
        "PFT_CODE_REVIEW_RESUME_DONE and either one additional concrete finding ",
        "with a file reference or say no additional findings with a short rationale. ",
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
    let has_resume_review = resume_output.text.contains("PFT_CODE_REVIEW_RESUME_DONE");
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
        .map(|bridge| tokio::spawn(run_ambient_chat_bridge(bridge)));
    let mut child = Command::new(&plan.executable)
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
    let mut heartbeat = interval(Duration::from_secs(30));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let timeout_sleep = tokio::time::sleep(CLAUDE_PANE_TURN_TIMEOUT);
    tokio::pin!(timeout_sleep);
    let mut timed_out = false;
    let mut last_progress_key: Option<String> = None;
    loop {
        tokio::select! {
            _ = &mut timeout_sleep => {
                timed_out = true;
                let _ = child.kill().await;
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
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && let Some(progress) = progress_from_claude_value(&plan, &started_at, &value)
                {
                    last_progress_elapsed_ms = Some(progress.elapsed_ms);
                    let key = progress_key(&progress);
                    if last_progress_key.as_deref() != Some(key.as_str())
                        && let Some(tx) = progress_tx.as_ref()
                    {
                        tx.send(AppEvent::ClaudePaneTurnProgress { progress });
                    }
                    last_progress_key = Some(key);
                }
            }
            _ = heartbeat.tick() => {
                last_progress_elapsed_ms = Some(elapsed_ms(&started_at));
                emit_claude_progress(
                    &progress_tx,
                    &plan,
                    &started_at,
                    "waiting",
                    "Claude pane still running.",
                    Some(format!(
                        "elapsed={}ms; mode: {}; artifact: {}",
                        elapsed_ms(&started_at),
                        plan.command_mode.label(),
                        plan.artifact_path.display()
                    )),
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

    let wait_result = if timed_out {
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
    if timed_out {
        let _ = child.kill().await;
    }
    let stderr = stderr_task.await.unwrap_or_default();
    if let Some(handle) = bridge_handle {
        handle.abort();
    }
    let duration_ms = elapsed_ms(&started_at);
    let ended_at_unix_ms = unix_epoch_ms();

    if timed_out {
        let output = partial_failed_turn_output(
            &plan,
            duration_ms,
            ClaudePaneTurnStatus::TimeoutPause,
            Some("timeout".to_string()),
            format!(
                "Claude pane turn hit PFTerminal's wall-clock timeout after {CLAUDE_PANE_TURN_TIMEOUT:?}. Type `continue` in this pane to resume if a Claude session id was captured."
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
    } else {
        output.tool_events = tool_events_from_stdout(stdout);
        output.tool_names = dedupe_tool_names(
            output
                .tool_events
                .iter()
                .map(|event| event.name.clone())
                .collect(),
        );
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

async fn run_ambient_chat_bridge(plan: ClaudeBridgePlan) -> Result<()> {
    let listener = TcpListener::from_std(plan.listener)
        .context("failed to create async Ambient Claude bridge listener")?;
    let api_key = Arc::new(plan.upstream_api_key);
    let upstream_model = Arc::new(plan.upstream_model);
    let http = reqwest::Client::new();
    loop {
        let (stream, _) = listener.accept().await?;
        let api_key = api_key.clone();
        let upstream_model = upstream_model.clone();
        let http = http.clone();
        tokio::spawn(async move {
            if let Err(err) =
                handle_ambient_bridge_connection(stream, api_key, upstream_model, http).await
            {
                tracing::debug!(error = %err, "Ambient Claude bridge connection failed");
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

    let request: Value = serde_json::from_slice(body).context("invalid Claude Messages request")?;
    let wants_stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_tokens = request
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(1024)
        .clamp(64, 4096);
    let mut chat_messages = ambient_chat_messages_from_claude_request(&request)?;
    let prior_tool_calls = claude_tool_use_count(&request);
    let tool_budget_exhausted = prior_tool_calls >= AMBIENT_BRIDGE_TOOL_CALL_BUDGET;
    if tool_budget_exhausted {
        chat_messages.push(serde_json::json!({
            "role": "user",
            "content": "PFTerminal has reached the local Claude pane tool-call budget for this turn. Do not call more tools. Produce the final answer now using the evidence already gathered. Preserve any required completion marker from the original user request."
        }));
    }
    let chat_tools = if tool_budget_exhausted {
        Vec::new()
    } else {
        ambient_chat_tools_from_claude_request(&request)
    };
    let mut upstream_body = serde_json::json!({
        "model": upstream_model.as_str(),
        "messages": chat_messages,
        "max_tokens": max_tokens,
    });
    if !chat_tools.is_empty() {
        upstream_body["tools"] = Value::Array(chat_tools);
        upstream_body["tool_choice"] = Value::String("auto".to_string());
    }
    let response = http
        .post("https://api.ambient.xyz/v1/chat/completions")
        .bearer_auth(api_key.as_str())
        .json(&upstream_body)
        .send()
        .await
        .context("Ambient Chat bridge upstream request failed")?;
    let status = response.status();
    let response_text = response.text().await.unwrap_or_default();
    if !status.is_success() {
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
        return Ok(());
    }

    let upstream: Value =
        serde_json::from_str(&response_text).context("Ambient Chat response was not JSON")?;
    let usage = upstream.get("usage").cloned().unwrap_or_else(|| {
        serde_json::json!({
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0
        })
    });
    let tool_calls = bridge_tool_calls_from_ambient_response(&upstream);
    if let Some(tool_call) = tool_calls.first() {
        if wants_stream {
            write_anthropic_stream_tool_use_response(
                &mut stream,
                upstream_model.as_str(),
                tool_call,
                &usage,
            )
            .await?;
        } else {
            write_json_response(
                &mut stream,
                anthropic_tool_use_response(upstream_model.as_str(), tool_call, &usage),
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
        write_anthropic_stream_response(&mut stream, upstream_model.as_str(), &text, &usage)
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

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
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

fn claude_tool_use_count(request: &Value) -> usize {
    request
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        .count()
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
        "usage": {
            "input_tokens": usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
            "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
        }
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

fn anthropic_tool_use_response(model: &str, tool_call: &BridgeToolCall, usage: &Value) -> Value {
    serde_json::json!({
        "id": format!("msg_pfterminal_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{
            "type": "tool_use",
            "id": tool_call.id,
            "name": tool_call.name,
            "input": tool_call.input
        }],
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {
            "input_tokens": usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
            "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
        }
    })
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

async fn write_anthropic_stream_response(
    stream: &mut tokio::net::TcpStream,
    model: &str,
    text: &str,
    usage: &Value,
) -> Result<()> {
    let message_id = format!("msg_pfterminal_{}", Uuid::new_v4().simple());
    let events = vec![
        (
            "message_start",
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {
                        "input_tokens": usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                        "output_tokens": 0
                    }
                }
            }),
        ),
        (
            "content_block_start",
            serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            }),
        ),
        (
            "content_block_delta",
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": text }
            }),
        ),
        (
            "content_block_stop",
            serde_json::json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "message_delta",
            serde_json::json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                "usage": {
                    "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
                }
            }),
        ),
        (
            "message_stop",
            serde_json::json!({ "type": "message_stop" }),
        ),
    ];
    let mut body = String::new();
    for (event, data) in events {
        body.push_str("event: ");
        body.push_str(event);
        body.push_str("\ndata: ");
        body.push_str(&data.to_string());
        body.push_str("\n\n");
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n{body}"
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

async fn write_anthropic_stream_tool_use_response(
    stream: &mut tokio::net::TcpStream,
    model: &str,
    tool_call: &BridgeToolCall,
    usage: &Value,
) -> Result<()> {
    let message_id = format!("msg_pfterminal_{}", Uuid::new_v4().simple());
    let partial_json = tool_call.input.to_string();
    let events = vec![
        (
            "message_start",
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {
                        "input_tokens": usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                        "output_tokens": 0
                    }
                }
            }),
        ),
        (
            "content_block_start",
            serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": tool_call.id,
                    "name": tool_call.name,
                    "input": {}
                }
            }),
        ),
        (
            "content_block_delta",
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "input_json_delta", "partial_json": partial_json }
            }),
        ),
        (
            "content_block_stop",
            serde_json::json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "message_delta",
            serde_json::json!({
                "type": "message_delta",
                "delta": { "stop_reason": "tool_use", "stop_sequence": null },
                "usage": {
                    "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
                }
            }),
        ),
        (
            "message_stop",
            serde_json::json!({ "type": "message_stop" }),
        ),
    ];
    let mut body = String::new();
    for (event, data) in events {
        body.push_str("event: ");
        body.push_str(event);
        body.push_str("\ndata: ");
        body.push_str(&data.to_string());
        body.push_str("\n\n");
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n{body}"
    );
    stream.write_all(response.as_bytes()).await?;
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
    let mut tool_names = Vec::new();
    let mut tool_events = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("Claude stream-json line was not valid JSON: {line}"))?;
        if value.get("is_error").and_then(Value::as_bool) == Some(true) {
            error_value = Some(value.clone());
        }
        collect_text_chunks(&value, &mut assistant_chunks);
        collect_tool_names(&value, &mut tool_names);
        collect_tool_events(&value, &mut tool_events);
        if let Some(result) = value.get("result").and_then(Value::as_str) {
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
        });
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
    })
}

fn parsed_from_value(value: &Value) -> Result<ParsedClaudeOutput> {
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        let mut tool_names = Vec::new();
        let mut tool_events = Vec::new();
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
        });
    }
    let mut assistant_chunks = Vec::new();
    collect_text_chunks(value, &mut assistant_chunks);
    let mut tool_names = Vec::new();
    let mut tool_events = Vec::new();
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
            .map(|input| truncate_for_display(&input.to_string(), 240))
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
                    .map(|input| truncate_for_display(&input.to_string(), 240))
                    .unwrap_or_default();
                tool_events.push(ClaudePaneToolEvent {
                    name: name.to_string(),
                    preview,
                });
            }
        }
    }
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
                hint,
                elapsed_ms: elapsed_ms(started_at),
                artifact_path: plan.artifact_path.clone(),
                audit_path: plan.audit_path.clone(),
            },
        });
    }
}

fn progress_from_claude_value(
    plan: &ClaudeCommandPlan,
    started_at: &Instant,
    value: &Value,
) -> Option<ClaudePaneTurnProgress> {
    let value_type = value.get("type").and_then(Value::as_str).unwrap_or("event");
    let mut tool_events = Vec::new();
    collect_tool_events(value, &mut tool_events);
    if let Some(event) = tool_events.last() {
        return Some(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "tool-call".to_string(),
            summary: format!("Claude tool call: {}", event.name),
            hint: Some(format!(
                "preview: {}; artifact: {}",
                event.preview,
                plan.artifact_path.display()
            )),
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
    }
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Some(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "error".to_string(),
            summary: "Claude reported an error result.".to_string(),
            hint: Some(claude_error_summary(value)),
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
    }
    if value.get("result").and_then(Value::as_str).is_some() {
        return Some(ClaudePaneTurnProgress {
            pane_id: plan.pane_id.clone(),
            phase: "assistant-result".to_string(),
            summary: "Claude returned a result.".to_string(),
            hint: Some(format!("artifact: {}", plan.artifact_path.display())),
            elapsed_ms: elapsed_ms(started_at),
            artifact_path: plan.artifact_path.clone(),
            audit_path: plan.audit_path.clone(),
        });
    }
    (value_type == "system").then(|| ClaudePaneTurnProgress {
        pane_id: plan.pane_id.clone(),
        phase: "system".to_string(),
        summary: "Claude session initialized.".to_string(),
        hint: value
            .get("session_id")
            .and_then(Value::as_str)
            .map(|session_id| format!("session_id: {session_id}")),
        elapsed_ms: elapsed_ms(started_at),
        artifact_path: plan.artifact_path.clone(),
        audit_path: plan.audit_path.clone(),
    })
}

fn progress_key(progress: &ClaudePaneTurnProgress) -> String {
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
        items.push(section_item("New Claude Pane"));
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
        items.push(section_item("Agent Panes"));
        items.extend(self.agent_pane_items());

        self.chat_widget.show_selection_view(SelectionViewParams {
            title: Some("Panes".to_string()),
            subtitle: Some("Switch user panes or create Claude Code headless panes.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search panes".to_string()),
            ..Default::default()
        });
    }

    pub(super) fn select_user_pane(&mut self, pane_id: String) {
        match self.claude_panes.set_active_user_pane(&pane_id) {
            Ok(()) if pane_id == CODEX_MAIN_PANE_ID => {
                self.sync_active_agent_label();
                self.chat_widget.add_info_message(
                    "Switched to Codex main pane.".to_string(),
                    /*hint*/ None,
                );
            }
            Ok(()) => {
                self.sync_active_agent_label();
                let title = self
                    .claude_panes
                    .panes()
                    .iter()
                    .find(|pane| pane.id == pane_id)
                    .map(|pane| pane.title.clone())
                    .unwrap_or_else(|| pane_id.clone());
                self.chat_widget
                    .add_info_message(format!("Switched to {title}."), /*hint*/ None);
            }
            Err(err) => self.chat_widget.add_error_message(err.to_string()),
        }
    }

    pub(super) fn create_claude_pane(&mut self, profile: ClaudeProviderProfileKind) {
        match self.claude_panes.create_pane(
            profile,
            self.config.cwd.to_path_buf(),
            self.config.codex_home.as_ref(),
        ) {
            Ok(id) => {
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

    pub(super) fn try_submit_active_claude_pane_op(&mut self, op: &AppCommand) -> bool {
        let Some(pane_id) = self
            .claude_panes
            .active_claude_pane_id()
            .map(ToString::to_string)
        else {
            return false;
        };
        let prompt = match prompt_from_user_turn(op) {
            Ok(Some(prompt)) => prompt,
            Ok(None) => return false,
            Err(err) => {
                self.chat_widget.fail_external_pane_turn(err.to_string());
                return true;
            }
        };
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

    pub(super) fn on_claude_pane_turn_progress(&mut self, progress: ClaudePaneTurnProgress) {
        if self.claude_panes.active_user_pane_id() != progress.pane_id {
            return;
        }
        self.chat_widget.add_info_message(
            progress.summary,
            progress.hint.or_else(|| {
                Some(format!(
                    "elapsed={}ms; artifact: {}; audit: {}",
                    progress.elapsed_ms,
                    progress.artifact_path.display(),
                    progress.audit_path.display()
                ))
            }),
        );
    }

    pub(super) fn on_claude_pane_turn_finished(
        &mut self,
        pane_id: String,
        result: Result<ClaudePaneTurnOutput, String>,
    ) {
        self.claude_panes.finish_turn(&pane_id, &result);
        match result {
            Ok(output) => {
                if !output.text.trim().is_empty() {
                    self.chat_widget
                        .append_external_pane_response(output.text.clone());
                }
                let hint = output.audit_hint();
                if output.status.is_success() {
                    self.chat_widget
                        .complete_external_pane_turn(Some(output.text), Some(output.duration_ms));
                    self.chat_widget
                        .add_info_message("Claude pane turn complete.".to_string(), Some(hint));
                } else {
                    self.chat_widget
                        .fail_external_pane_turn(output.failure_message());
                    self.chat_widget.add_info_message(
                        "Claude pane turn audit recorded.".to_string(),
                        Some(hint),
                    );
                }
            }
            Err(error) => self.chat_widget.fail_external_pane_turn(error),
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

    fn agent_pane_items(&self) -> Vec<SelectionItem> {
        self.agent_navigation
            .ordered_threads()
            .into_iter()
            .map(|(thread_id, entry)| {
                agent_thread_item(
                    self.active_thread_id,
                    self.primary_thread_id,
                    thread_id,
                    entry,
                )
            })
            .collect()
    }
}

fn section_item(name: &str) -> SelectionItem {
    SelectionItem {
        name: name.to_string(),
        is_disabled: true,
        ..Default::default()
    }
}

fn agent_thread_item(
    active_thread_id: Option<ThreadId>,
    primary_thread_id: Option<ThreadId>,
    thread_id: ThreadId,
    entry: &AgentPickerThreadEntry,
) -> SelectionItem {
    let is_primary = primary_thread_id == Some(thread_id);
    let name = format_agent_picker_item_name(
        entry.agent_nickname.as_deref(),
        entry.agent_role.as_deref(),
        is_primary,
    );
    let uuid = thread_id.to_string();
    SelectionItem {
        name: name.clone(),
        name_prefix_spans: agent_picker_status_dot_spans(entry.is_closed),
        description: Some(uuid.clone()),
        is_current: active_thread_id == Some(thread_id),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::SelectAgentThread(thread_id));
        })],
        dismiss_on_select: true,
        search_value: Some(format!("{name} {uuid}")),
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
                cwd: std::env::current_dir().expect("cwd"),
                claude_session_id: None,
                status: ClaudePaneStatus::Idle,
                latest_usage_summary: None,
                latest_usage_status: None,
                latest_turn_status: None,
                latest_audit_path: None,
                artifact_dir,
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
    fn command_plan_uses_session_id_then_resume_without_secret_in_args() {
        let (dir, mut pane) = pane(ClaudeProviderProfileKind::ClaudePlan);
        let first =
            build_claude_command_plan(&pane, "hello".to_string(), dir.path()).expect("first plan");
        assert!(
            first.args.windows(2).any(|w| {
                w[0] == "--session-id" && w[1] == pane.id.trim_start_matches("claude-")
            })
        );
        assert!(
            first
                .args
                .windows(2)
                .any(|w| w[0] == "--output-format" && w[1] == "stream-json")
        );
        assert!(first.args.iter().any(|arg| arg == "--verbose"));
        assert!(
            first
                .args
                .windows(2)
                .any(|w| w[0] == "--max-turns" && w[1] == CLAUDE_PANE_MAX_TURNS)
        );
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
            command_mode: ClaudeCommandMode::NewSession,
        };

        write_turn_audit(&plan, &output, 1, 2, Some(1)).expect("write audit");
        let audit = std::fs::read_to_string(&plan.audit_path).expect("read audit");
        let audit: Value = serde_json::from_str(&audit).expect("audit json");
        assert_eq!(audit.get("tool_use_count").and_then(Value::as_u64), Some(3));
    }

    #[test]
    fn allowed_auth_helper_labels_are_provider_scoped() {
        assert!(allowed_provider_vault_label("provider/zai_api_key"));
        assert!(allowed_provider_vault_label("provider/ambient_api_key"));
        assert!(allowed_provider_vault_label("provider/baseten_api_key"));
        assert!(allowed_provider_vault_label("provider/openrouter_api_key"));
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
    fn claude_tool_use_count_counts_prior_assistant_tool_calls() {
        let request = json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {"type": "tool_use", "id": "toolu_1", "name": "Read"},
                        {"type": "text", "text": "checking"}
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        {"type": "tool_use", "id": "toolu_2", "name": "Bash"}
                    ]
                },
                {
                    "role": "user",
                    "content": "continue"
                }
            ]
        });

        assert_eq!(claude_tool_use_count(&request), 2);
    }

    #[test]
    fn ambient_tool_call_is_translated_to_anthropic_tool_use() {
        let upstream = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "chatcmpl-tool-1",
                        "type": "function",
                        "function": {
                            "name": "Read",
                            "arguments": "{\"path\":\"README.md\"}"
                        }
                    }]
                }
            }]
        });
        let calls = bridge_tool_calls_from_ambient_response(&upstream);
        let response = anthropic_tool_use_response(
            "zai-org/GLM-5.2-FP8",
            &calls[0],
            &json!({"prompt_tokens": 5, "completion_tokens": 3}),
        );

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "Read");
        assert_eq!(
            response.pointer("/content/0/type").and_then(Value::as_str),
            Some("tool_use")
        );
        assert_eq!(
            response.pointer("/stop_reason").and_then(Value::as_str),
            Some("tool_use")
        );
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
        let first = run_claude_command_plan(first_plan, None)
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
        let second = run_claude_command_plan(second_plan, None)
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
        let output = run_claude_command_plan(plan, None)
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
        let output = run_claude_command_plan(plan, None)
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
        let output = run_claude_command_plan(plan, None)
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
