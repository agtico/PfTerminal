//! Building Claude Code command plans, settings, and prompts.

use std::collections::BTreeMap;
use std::net::TcpListener as StdTcpListener;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use codex_app_server_protocol::UserInput;
use codex_vault::Vault;
use serde_json::Value;
use uuid::Uuid;

use crate::app_command::AppCommand;
use crate::spawn_orchestration::SpawnRole;

use super::pane::ClaudeCommandMode;
use super::pane::ClaudePane;
use super::provider::ClaudeProviderProfile;
use super::provider::ClaudeProviderProfileKind;
use super::provider::ClaudeProviderTransport;
use super::turn_types::ClaudeBridgeKind;
use super::turn_types::ClaudeBridgePlan;
use super::turn_types::ClaudeCommandPlan;
pub(crate) fn ensure_vault_label_exists(codex_home: &Path, label: &str) -> Result<()> {
    let vault = Vault::new(codex_home.to_path_buf());
    match vault.exists(label) {
        Ok(true) => Ok(()),
        Ok(false) => Err(anyhow!(
            "Missing vault credential `{label}`. Add it from /providers before creating this Claude pane."
        )),
        Err(err) => Err(anyhow!("Could not read vault credential `{label}`: {err}")),
    }
}

pub(crate) fn reveal_provider_secret(codex_home: &Path, label: &str) -> Result<String> {
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
            | "provider/anthropic_api_key"
            | "provider/ambient_api_key"
            | "provider/baseten_api_key"
            | "provider/openrouter_api_key"
            | "provider/ai_gateway_api_key"
    )
}

pub(crate) fn build_claude_command_plan(
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
                profile.provider_model.to_string(),
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
    if matches!(profile.kind, ClaudeProviderProfileKind::ClaudePlan) {
        args.extend(["--effort".to_string(), "high".to_string()]);
    }
    args.extend(["--setting-sources".to_string(), "project".to_string()]);
    let (command_mode, command_session_id) = if let Some(session_id) = &pane.claude_session_id {
        args.push("--resume".to_string());
        args.push(session_id.clone());
        (ClaudeCommandMode::Resume, session_id.clone())
    } else {
        let session_id = Uuid::new_v4().to_string();
        args.push("--session-id".to_string());
        args.push(session_id.clone());
        (ClaudeCommandMode::NewSession, session_id)
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
        command_session_id,
        max_turns: None,
        artifact_path,
        audit_path,
        timeout_ms: None,
        bridge,
    })
}

pub(crate) fn settings_json_with_base_url(
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

pub(crate) fn claude_pane_title(
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
