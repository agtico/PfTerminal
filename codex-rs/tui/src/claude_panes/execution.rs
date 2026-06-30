//! Running Claude Code command plans and producing turn outputs.

use std::io::Write as _;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::Command;
use tokio::time::MissedTickBehavior;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

use super::bridge::run_claude_bridge;
use super::output_parse::ParsedClaudeOutput;
use super::output_parse::parse_claude_output;
use super::pane::ClaudePaneTurnStatus;
use super::pane::ClaudePaneUsageStatus;
use super::progress::dedupe_tool_names;
use super::progress::elapsed_ms;
use super::progress::emit_claude_progress;
use super::progress::progress_key;
use super::progress::progresses_from_claude_value;
use super::progress::reasoning_events_from_stdout;
use super::progress::tool_events_from_stdout;
use super::progress::truncate_for_display;
use super::progress::unix_epoch_ms;
use super::progress::usage_status_from_summary;
use super::turn_types::ClaudeCommandPlan;
use super::turn_types::ClaudePaneTurnAudit;
use super::turn_types::ClaudePaneTurnOutput;
use super::turn_types::PreparedClaudePaneTurn;

const CLAUDE_PANE_PROGRESS_HEARTBEAT: Duration = Duration::from_secs(30);
const CLAUDE_SECRET_REDACTION: &str = "[REDACTED_SECRET]";
const MIN_REDACTED_SECRET_LEN: usize = 8;
pub(crate) async fn run_prepared_claude_turn(
    prepared: PreparedClaudePaneTurn,
    progress_tx: Option<AppEventSender>,
) -> Result<ClaudePaneTurnOutput, String> {
    run_claude_command_plan(prepared.plan, prepared.cancel_token, progress_tx)
        .await
        .map_err(|err| format!("{err:#}"))
}

pub(crate) async fn run_claude_command_plan(
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
    let redactor = ClaudeSecretRedactor::from_plan(&plan);
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
    for key in &plan.env_remove {
        command.env_remove(key);
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
                let line = redactor.redact(&line);
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
        redactor.redact(&stderr_task.await.unwrap_or_default())
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

#[derive(Debug, Clone, Default)]
pub(crate) struct ClaudeSecretRedactor {
    secrets: Vec<String>,
}

impl ClaudeSecretRedactor {
    pub(crate) fn from_plan(plan: &ClaudeCommandPlan) -> Self {
        let secrets = plan
            .bridge
            .as_ref()
            .map(|bridge| bridge.upstream_api_key.as_str())
            .into_iter()
            .filter(|secret| secret.len() >= MIN_REDACTED_SECRET_LEN)
            .map(ToString::to_string)
            .collect();
        Self { secrets }
    }

    pub(crate) fn redact(&self, text: &str) -> String {
        let mut redacted = text.to_string();
        for secret in &self.secrets {
            redacted = redacted.replace(secret, CLAUDE_SECRET_REDACTION);
        }
        redacted
    }
}

pub(crate) async fn stop_claude_child(child: &mut Child) -> Result<()> {
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
        session_id: parsed
            .session_id
            .or_else(|| Some(plan.command_session_id.clone())),
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

pub(crate) fn failed_turn_output(
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

pub(crate) fn partial_failed_turn_output(
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
        output.session_id = parsed
            .session_id
            .or_else(|| Some(plan.command_session_id.clone()));
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
        if matches!(
            status,
            ClaudePaneTurnStatus::Interrupted | ClaudePaneTurnStatus::TimeoutPause
        ) {
            output.session_id =
                session_id_from_stdout(stdout).or_else(|| Some(plan.command_session_id.clone()));
        }
    }
    output
}

pub(crate) fn session_id_from_stdout(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|value| {
            value
                .get("session_id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

pub(crate) fn write_turn_audit(
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
