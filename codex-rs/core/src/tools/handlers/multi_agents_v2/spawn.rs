use super::*;
use crate::agent::control::SpawnAgentForkMode;
use crate::agent::control::SpawnAgentOptions;
use crate::agent::next_thread_spawn_depth;
use crate::agent::role::DEFAULT_ROLE_NAME;
use crate::agent_communication::AgentCommunicationContext;
use crate::agent_communication::AgentCommunicationKind;
use crate::tools::handlers::multi_agents_spec::SpawnAgentToolOptions;
use crate::tools::handlers::multi_agents_spec::create_spawn_agent_tool_v2;
use crate::tools::handlers::multi_agents_v2::message_tool::message_content;
use codex_protocol::AgentPath;
use codex_tools::ToolSpec;

#[derive(Default)]
pub(crate) struct Handler {
    options: SpawnAgentToolOptions,
}

impl Handler {
    pub(crate) fn new(options: SpawnAgentToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("spawn_agent")
    }

    fn spec(&self) -> ToolSpec {
        create_spawn_agent_tool_v2(self.options.clone())
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            handle_spawn_agent(invocation, CollaborationMessageEncoding::ProviderNative)
                .await
                .map(boxed_tool_output)
        })
    }
}

#[derive(Default)]
pub(crate) struct PlaintextHandler {
    options: SpawnAgentToolOptions,
}

impl PlaintextHandler {
    pub(crate) fn new(options: SpawnAgentToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for PlaintextHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(PLAINTEXT_SPAWN_AGENT_TOOL)
    }

    fn spec(&self) -> ToolSpec {
        plaintext_adapter_spec(
            create_spawn_agent_tool_v2(self.options.clone()),
            PLAINTEXT_SPAWN_AGENT_TOOL,
            "spawn_agent",
        )
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            handle_spawn_agent(invocation, CollaborationMessageEncoding::PlaintextAdapter)
                .await
                .map(boxed_tool_output)
        })
    }
}

async fn handle_spawn_agent(
    invocation: ToolInvocation,
    encoding: CollaborationMessageEncoding,
) -> Result<SpawnAgentResult, FunctionCallError> {
    let ToolInvocation {
        session,
        step_context,
        payload,
        call_id,
        source,
        ..
    } = invocation;
    let turn = &step_context.turn;
    ensure_manager_tool_allowed(turn, "spawn_agent")?;
    let arguments = function_arguments(payload)?;
    let args: SpawnAgentArgs = parse_arguments(&arguments)?;
    let fork_mode = args.fork_mode()?;
    let message = message_content(args.message)?;
    let role_name = args
        .agent_type
        .as_deref()
        .map(str::trim)
        .filter(|role| !role.is_empty());

    let session_source = turn.session_source.clone();
    let child_depth = next_thread_spawn_depth(&session_source);
    // Resolve the canonical path before runtime policy so malformed task names produce the
    // actionable routing error regardless of the parent runtime's current catalogue lifecycle.
    let spawn_source = thread_spawn_source(
        session.thread_id,
        &turn.session_source,
        child_depth,
        role_name,
        Some(args.task_name.clone()),
        Some(call_id.clone()),
    )?;
    let new_agent_path = spawn_source.get_agent_path().ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "spawned agent is missing a canonical task name".to_string(),
        )
    })?;
    let child_runtime_was_selected = args.model_provider.is_some()
        || args.model.is_some()
        || args.reasoning_effort.is_some()
        || role_name.is_some()
        || turn.config.agent_default_subagent_model.is_some()
        || turn
            .config
            .agent_default_subagent_reasoning_effort
            .is_some();
    let mut config = build_agent_spawn_config(
        &session.get_base_instructions().await,
        turn.as_ref(),
        step_context.environments.primary(),
    )?;
    if let Some(service_tier) = args.service_tier.as_ref() {
        config.service_tier = Some(service_tier.clone());
    }
    let is_full_history_fork = matches!(fork_mode, Some(SpawnAgentForkMode::FullHistory));
    if is_full_history_fork {
        reject_full_fork_agent_type_override(role_name)?;
    }
    if is_full_history_fork || args.model_provider.is_none() {
        apply_requested_spawn_agent_model_overrides(
            &session,
            turn.as_ref(),
            &mut config,
            args.model.as_deref(),
            args.reasoning_effort.clone(),
        )
        .await?;
        if !is_full_history_fork {
            // Legacy same-provider model selection retains the established role precedence.
            apply_spawn_agent_role(&session, &mut config, role_name).await?;
        }
    } else {
        // An explicit provider/model pair is a complete runtime selection. Apply the role first
        // so its instruction and sandbox layers survive while the requested runtime wins.
        apply_spawn_agent_role(&session, &mut config, role_name).await?;
        apply_requested_spawn_agent_runtime_overrides(
            &session,
            turn.as_ref(),
            &mut config,
            args.model_provider.as_deref(),
            args.model.as_deref(),
            args.reasoning_effort.clone(),
        )
        .await?;
    }
    apply_spawn_agent_service_tier(
        &session,
        &mut config,
        turn.config.service_tier.as_deref(),
        args.service_tier.as_deref(),
    )
    .await?;
    apply_spawn_agent_runtime_overrides(
        &mut config,
        turn.as_ref(),
        step_context.environments.primary(),
    )?;
    ensure_spawn_provider_authorized(&config, &config.model_provider_id)?;
    // Existing sessions may legitimately run a model that has since been hidden or disabled in
    // the refreshed catalogue. Inheriting that already-authorized runtime preserves the active
    // session contract; selections made specifically for the child still fail closed.
    if child_runtime_was_selected {
        ensure_spawn_runtime_eligible(&session, &config).await?;
    }
    let resolved_model_provider = config.model_provider_id.clone();
    let resolved_model = config
        .model
        .clone()
        .unwrap_or_else(|| turn.model_info.slug.clone());
    let resolved_reasoning_effort = config.model_reasoning_effort.clone();
    let resolved_service_tier = config.service_tier.clone();

    ensure_message_encoding_matches_target(
        &turn.config.model_provider_id,
        &source,
        &resolved_model_provider,
        encoding,
        "spawn_agent",
        PLAINTEXT_SPAWN_AGENT_TOOL,
    )?;
    let source = match encoding {
        CollaborationMessageEncoding::ProviderNative => source,
        CollaborationMessageEncoding::PlaintextAdapter => {
            crate::tools::context::ToolCallSource::DirectPlaintextMessage
        }
    };

    let author = turn
        .session_source
        .get_agent_path()
        .unwrap_or_else(AgentPath::root);
    let communication = communication_from_tool_message(
        author,
        new_agent_path.clone(),
        message,
        &source,
        &turn.config.model_provider_id,
        /*trigger_turn*/ true,
    );
    let context = AgentCommunicationContext::new(AgentCommunicationKind::Spawn, session.thread_id);
    let spawned_agent = Box::pin(
        session
            .services
            .agent_control
            .spawn_agent_with_communication(
                config,
                communication,
                context,
                Some(spawn_source),
                SpawnAgentOptions {
                    fork_parent_spawn_call_id: fork_mode.as_ref().map(|_| call_id.clone()),
                    fork_mode,
                    parent_thread_id: Some(session.thread_id),
                    parent_turn_id: Some(turn.sub_id.clone()),
                    environments: Some(step_context.environments.to_selections()),
                },
            ),
    )
    .await
    .map_err(collab_spawn_error)?;
    let new_thread_id = spawned_agent.thread_id;
    session
        .services
        .agent_control
        .note_native_agent_dispatch(session.thread_id);
    let agent_snapshot = session
        .services
        .agent_control
        .get_agent_config_snapshot(new_thread_id)
        .await;
    let nickname = agent_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.session_source.get_nickname())
        .or(spawned_agent.metadata.agent_nickname);
    emit_sub_agent_activity(
        &session,
        turn,
        SubAgentActivityItem {
            id: call_id,
            agent_thread_id: new_thread_id,
            agent_path: new_agent_path.clone(),
            kind: SubAgentActivityKind::Started,
        },
    )
    .await;
    let role_tag = role_name.unwrap_or(DEFAULT_ROLE_NAME);
    turn.session_telemetry.counter(
        "codex.multi_agent.spawn",
        /*inc*/ 1,
        &[("role", role_tag), ("version", "v2")],
    );
    let task_name = String::from(new_agent_path);

    let hide_agent_metadata = turn.config.multi_agent_v2.hide_spawn_agent_metadata;
    if hide_agent_metadata {
        Ok(SpawnAgentResult::HiddenMetadata { task_name })
    } else {
        Ok(SpawnAgentResult::WithNickname {
            task_name,
            nickname,
            model_provider: resolved_model_provider,
            model: resolved_model,
            reasoning_effort: resolved_reasoning_effort,
            service_tier: resolved_service_tier,
        })
    }
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

impl CoreToolRuntime for PlaintextHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnAgentArgs {
    message: String,
    task_name: String,
    agent_type: Option<String>,
    model_provider: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    service_tier: Option<String>,
    fork_turns: Option<String>,
    fork_context: Option<bool>,
}

impl SpawnAgentArgs {
    fn fork_mode(&self) -> Result<Option<SpawnAgentForkMode>, FunctionCallError> {
        if self.fork_context.is_some() {
            return Err(FunctionCallError::RespondToModel(
                "fork_context is not supported in MultiAgentV2; use fork_turns instead".to_string(),
            ));
        }

        let fork_turns = self
            .fork_turns
            .as_deref()
            .map(str::trim)
            .filter(|fork_turns| !fork_turns.is_empty())
            .unwrap_or("all");

        if fork_turns.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        if fork_turns.eq_ignore_ascii_case("all") {
            return Ok(Some(SpawnAgentForkMode::FullHistory));
        }

        let last_n_turns = fork_turns.parse::<usize>().map_err(|_| {
            FunctionCallError::RespondToModel(
                "fork_turns must be `none`, `all`, or a positive integer string".to_string(),
            )
        })?;
        if last_n_turns == 0 {
            return Err(FunctionCallError::RespondToModel(
                "fork_turns must be `none`, `all`, or a positive integer string".to_string(),
            ));
        }

        Ok(Some(SpawnAgentForkMode::LastNTurns(last_n_turns)))
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum SpawnAgentResult {
    WithNickname {
        task_name: String,
        nickname: Option<String>,
        model_provider: String,
        model: String,
        reasoning_effort: Option<ReasoningEffort>,
        service_tier: Option<String>,
    },
    HiddenMetadata {
        task_name: String,
    },
}

impl ToolOutput for SpawnAgentResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "spawn_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "spawn_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "spawn_agent")
    }
}
