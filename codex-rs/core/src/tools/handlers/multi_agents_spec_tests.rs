use super::*;
use codex_protocol::openai_models::ModelBilling;
use codex_protocol::openai_models::ModelCapabilityTier;
use codex_protocol::openai_models::ModelOrchestrationMetadata;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelServiceTier;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_tools::JsonSchemaPrimitiveType;
use codex_tools::JsonSchemaType;
use pretty_assertions::assert_eq;
use serde_json::json;

fn model_preset(id: &str, show_in_picker: bool) -> ModelPreset {
    ModelPreset {
        id: id.to_string(),
        model: format!("{id}-model"),
        provider_id: None,
        orchestration: None,
        display_name: format!("{id} display"),
        description: format!("{id} description"),
        model_specialty: None,
        default_reasoning_effort: ReasoningEffort::XHigh,
        supported_reasoning_efforts: vec![ReasoningEffortPreset {
            effort: ReasoningEffort::XHigh,
            description: "Extra high".to_string(),
        }],
        supports_personality: false,
        additional_speed_tiers: Vec::new(),
        service_tiers: vec![ModelServiceTier {
            id: "priority".to_string(),
            name: "Fast".to_string(),
            description: "1.5x speed, increased usage".to_string(),
        }],
        default_service_tier: None,
        is_default: false,
        upgrade: None,
        show_in_picker,
        multi_agent_version: Some(MultiAgentVersion::V2),
        availability_nux: None,
        supported_in_api: true,
        input_modalities: Vec::new(),
    }
}

#[test]
fn spawn_agent_tool_v2_requires_task_name_and_lists_visible_models() {
    let mut incompatible = model_preset("incompatible", /*show_in_picker*/ true);
    incompatible.multi_agent_version = Some(MultiAgentVersion::V1);
    let mut visible = model_preset("visible", /*show_in_picker*/ true);
    visible.provider_id = Some("example-provider".to_string());
    let mut legacy = model_preset("legacy", /*show_in_picker*/ true);
    legacy.multi_agent_version = Some(MultiAgentVersion::V1);
    let mut disabled = model_preset("disabled", /*show_in_picker*/ true);
    disabled.multi_agent_version = Some(MultiAgentVersion::Disabled);
    let tool = create_spawn_agent_tool_v2(SpawnAgentToolOptions {
        available_models: vec![
            visible,
            model_preset("hidden", /*show_in_picker*/ false),
            legacy,
            disabled,
        ],
        inherited_runtime: None,
        agent_type_description: "role help".to_string(),
        expose_agent_type: true,
        hide_agent_type_model_reasoning: false,
        expose_spawn_agent_model_overrides: true,
        multi_agent_version: MultiAgentVersion::V2,
        usage_hint_text: None,
    });

    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        output_schema,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");
    assert!(description.contains("Spawns an agent to work on the specified task."));
    assert!(description.contains("The spawned agent will have the same tools as you"));
    assert!(!description.contains("max_concurrent_threads_per_session"));
    assert!(description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V2));
    assert!(
        description.contains(
            "Available authorized exact runtime overrides (optional; omit both fields to inherit the current runtime)."
        )
    );
    assert!(description.contains(
        "- `example-provider` / `visible-model`; text-only; efforts: xhigh (default); tiers: priority"
    ));
    assert!(description.contains(
        "- `legacy-model`: legacy description Reasoning efforts: medium (default). Service tiers: priority."
    ));
    assert!(!description.contains("hidden-model"));
    assert!(!description.contains("disabled-model"));
    assert!(properties.contains_key("task_name"));
    assert!(properties.contains_key("message"));
    assert_eq!(
        properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        None
    );
    assert!(properties.contains_key("fork_turns"));
    assert!(!properties.contains_key("items"));
    assert!(!properties.contains_key("fork_context"));
    assert_eq!(
        properties
            .get("model")
            .and_then(|schema| schema.description.as_deref()),
        Some(SPAWN_AGENT_MODEL_OVERRIDE_DESCRIPTION)
    );
    assert_eq!(
        properties
            .get("reasoning_effort")
            .and_then(|schema| schema.description.as_deref()),
        Some("Reasoning effort override for the new agent. Omit to inherit the parent effort.")
    );
    assert_eq!(
        properties
            .get("service_tier")
            .and_then(|schema| schema.description.as_deref()),
        Some(SPAWN_AGENT_SERVICE_TIER_OVERRIDE_DESCRIPTION)
    );
    assert_eq!(
        parameters.required.as_ref(),
        Some(&vec!["task_name".to_string(), "message".to_string()])
    );
    assert_eq!(
        output_schema.expect("spawn_agent output schema")["required"],
        json!([
            "task_name",
            "nickname",
            "model_provider",
            "model",
            "reasoning_effort",
            "service_tier"
        ])
    );
}

#[test]
fn spawn_agent_catalog_exposes_parent_runtime_and_frontier_effort_policy() {
    let mut sol = model_preset("sol", /*show_in_picker*/ true);
    sol.model = "gpt-5.6-sol".to_string();
    sol.provider_id = Some("openai".to_string());
    sol.orchestration = Some(ModelOrchestrationMetadata::Eligible {
        provider_id: "openai".to_string(),
        capability: ModelCapabilityTier::Frontier,
        billing: ModelBilling::Plan {
            relative_burn_millis: 1_000,
        },
    });
    sol.supported_reasoning_efforts.extend([
        ReasoningEffortPreset {
            effort: ReasoningEffort::Custom("max".to_string()),
            description: "Maximum reasoning".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::Custom("ultra".to_string()),
            description: "Maximum reasoning with automatic delegation".to_string(),
        },
    ]);
    let inherited_runtime = SpawnAgentRuntime {
        model_provider: "claude-plan".to_string(),
        model: "claude-opus-5-plan".to_string(),
        reasoning_effort: Some(ReasoningEffort::High),
        service_tier: None,
    };

    let description =
        spawn_agent_models_description(&[sol], MultiAgentVersion::V2, Some(&inherited_runtime));

    assert!(
        description.contains(
            "Current inherited runtime: `claude-plan` / `claude-opus-5-plan`; effort high."
        )
    );
    assert!(description.contains(
        "Default allocation policy: compare the task with this catalogue before every spawn."
    ));
    assert!(description.contains(
        "Prefer an authorized `plan` runtime over a `metered` runtime when both can do the work"
    ));
    assert!(description.contains(
        "`openai` / `gpt-5.6-sol`; plan, burn 1x, frontier; frontier efforts: max, ultra (ultra includes automatic delegation)"
    ));
    assert!(
        description
            .contains("If the user names a provider or model, treat it as an exact constraint")
    );
}

#[test]
fn spawn_agent_tool_v1_keeps_legacy_fork_context_field() {
    let tool = create_spawn_agent_tool_v1(SpawnAgentToolOptions {
        available_models: Vec::new(),
        inherited_runtime: None,
        agent_type_description: "role help".to_string(),
        expose_agent_type: true,
        hide_agent_type_model_reasoning: false,
        expose_spawn_agent_model_overrides: true,
        multi_agent_version: MultiAgentVersion::V1,
        usage_hint_text: None,
    });

    let ToolSpec::Namespace(namespace) = tool else {
        panic!("spawn_agent v1 should be a namespace tool");
    };
    assert_eq!(namespace.name, MULTI_AGENT_V1_NAMESPACE);
    let Some(ResponsesApiNamespaceTool::Function(ResponsesApiTool {
        description,
        parameters,
        ..
    })) = namespace.tools.first()
    else {
        panic!("spawn_agent should be a namespace function tool");
    };
    assert_eq!(
        parameters.schema_type.clone(),
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");

    assert!(description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V1));
    assert!(properties.contains_key("fork_context"));
    assert!(!properties.contains_key("fork_turns"));
    assert_eq!(
        properties.get("agent_type"),
        Some(&JsonSchema::string(Some(format!(
            "{SPAWN_AGENT_TYPE_OVERRIDE_DESCRIPTION_V1}\nrole help"
        ))))
    );
    assert_eq!(
        properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        None
    );
    assert_eq!(
        properties
            .get("model")
            .and_then(|schema| schema.description.as_deref()),
        Some(SPAWN_AGENT_MODEL_OVERRIDE_DESCRIPTION)
    );
    assert_eq!(
        properties
            .get("service_tier")
            .and_then(|schema| schema.description.as_deref()),
        Some(SPAWN_AGENT_SERVICE_TIER_OVERRIDE_DESCRIPTION)
    );
}

#[test]
fn spawn_agent_tool_caps_visible_model_summaries() {
    let available_models = (0..=MAX_MODEL_OVERRIDES_IN_SPAWN_AGENT_DESCRIPTION)
        .map(|index| model_preset(&format!("model-{index}"), /*show_in_picker*/ true))
        .collect();
    let tool = create_spawn_agent_tool_v2(SpawnAgentToolOptions {
        available_models,
        inherited_runtime: None,
        agent_type_description: "role help".to_string(),
        expose_agent_type: true,
        hide_agent_type_model_reasoning: false,
        expose_spawn_agent_model_overrides: true,
        multi_agent_version: MultiAgentVersion::V2,
        usage_hint_text: None,
    });

    let ToolSpec::Function(ResponsesApiTool { description, .. }) = tool else {
        panic!("spawn_agent should be a function tool");
    };

    let last_visible = MAX_MODEL_OVERRIDES_IN_SPAWN_AGENT_DESCRIPTION - 1;
    assert!(description.contains(&format!("`model-{last_visible}-model`")));
    assert!(!description.contains(&format!(
        "`model-{MAX_MODEL_OVERRIDES_IN_SPAWN_AGENT_DESCRIPTION}-model`"
    )));
}

#[test]
fn spawn_agent_tool_caps_reasoning_effort_value_length() {
    let mut model = model_preset("visible", /*show_in_picker*/ true);
    let custom_effort = ReasoningEffort::Custom(
        "é".repeat(MAX_REASONING_EFFORT_CHARS_IN_SPAWN_AGENT_DESCRIPTION + 1),
    );
    model.default_reasoning_effort = custom_effort.clone();
    model.supported_reasoning_efforts = vec![ReasoningEffortPreset {
        effort: custom_effort,
        description: "Model-defined".to_string(),
    }];

    let description = spawn_agent_models_description(&[model], MultiAgentVersion::V2, None);
    let capped_effort = "é".repeat(MAX_REASONING_EFFORT_CHARS_IN_SPAWN_AGENT_DESCRIPTION);

    assert!(description.contains("Current inherited runtime: unavailable."));
    assert!(description.contains("Available authorized exact runtime overrides"));
    assert!(description.contains(&format!("efforts: {capped_effort} (default)")));
    assert!(!description.contains(&format!("{capped_effort}é")));
}

#[test]
fn spawn_agent_tool_keeps_model_controls_when_spawn_metadata_is_hidden() {
    let tool = create_spawn_agent_tool_v2(SpawnAgentToolOptions {
        available_models: vec![model_preset("visible", /*show_in_picker*/ true)],
        inherited_runtime: None,
        agent_type_description: "role help".to_string(),
        expose_agent_type: false,
        hide_agent_type_model_reasoning: true,
        expose_spawn_agent_model_overrides: true,
        multi_agent_version: MultiAgentVersion::V2,
        usage_hint_text: None,
    });

    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");

    assert!(!properties.contains_key("agent_type"));
    assert!(properties.contains_key("model"));
    assert!(properties.contains_key("reasoning_effort"));
    assert!(!properties.contains_key("service_tier"));
    assert!(!description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V2));
    assert!(description.contains("Available authorized exact runtime overrides"));
}

#[test]
fn spawn_agent_tool_hides_model_controls_without_override_exposure() {
    let tool = create_spawn_agent_tool_v2(SpawnAgentToolOptions {
        available_models: vec![model_preset("visible", /*show_in_picker*/ true)],
        inherited_runtime: None,
        agent_type_description: "role help".to_string(),
        expose_agent_type: false,
        hide_agent_type_model_reasoning: true,
        expose_spawn_agent_model_overrides: false,
        multi_agent_version: MultiAgentVersion::V2,
        usage_hint_text: None,
    });

    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");

    for property in ["agent_type", "model", "reasoning_effort", "service_tier"] {
        assert!(!properties.contains_key(property));
    }
    assert!(!description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V2));
    assert!(!description.contains("Available model overrides"));
}

#[test]
fn send_message_tool_requires_message_and_has_no_output_schema() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_send_message_tool()
    else {
        panic!("send_message should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("send_message should use object params");
    assert!(properties.contains_key("target"));
    assert!(properties.contains_key("message"));
    assert_eq!(
        properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        None
    );
    assert!(!properties.contains_key("interrupt"));
    assert!(!properties.contains_key("items"));
    assert_eq!(
        properties
            .get("target")
            .and_then(|schema| schema.description.as_deref()),
        Some("Relative or canonical task name to message (from spawn_agent).")
    );
    assert_eq!(
        parameters.required.as_ref(),
        Some(&vec!["target".to_string(), "message".to_string()])
    );
    assert_eq!(output_schema, None);
}

#[test]
fn followup_task_tool_requires_message_and_has_no_output_schema() {
    let ToolSpec::Function(ResponsesApiTool {
        name,
        description,
        parameters,
        output_schema,
        ..
    }) = create_followup_task_tool()
    else {
        panic!("followup_task should be a function tool");
    };
    assert_eq!(name, "followup_task");
    assert_eq!(
        description,
        "Send a follow-up task to an existing non-root target agent and trigger a turn if it is idle. If the target is already running, deliver the task promptly at message boundaries while sampling, or after the pending tool call completes."
    );
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("followup_task should use object params");
    assert!(properties.contains_key("target"));
    assert!(properties.contains_key("message"));
    assert_eq!(
        properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        None
    );
    assert!(!properties.contains_key("items"));
    assert_eq!(
        parameters.required.as_ref(),
        Some(&vec!["target".to_string(), "message".to_string()])
    );
    assert_eq!(output_schema, None);
}

#[test]
fn wait_agent_tool_v2_uses_timeout_only_summary_output() {
    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        output_schema,
        ..
    }) = create_wait_agent_tool_v2(WaitAgentTimeoutOptions {
        default_timeout_ms: 30_000,
        min_timeout_ms: 10_000,
        max_timeout_ms: 3_600_000,
    })
    else {
        panic!("wait_agent should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("wait_agent should use object params");
    assert!(!properties.contains_key("targets"));
    assert!(properties.contains_key("timeout_ms"));
    assert!(description.contains(
        "Does not return the content; returns either a summary of which agents have updates (if any)"
    ));
    assert_eq!(
        properties
            .get("timeout_ms")
            .and_then(|schema| schema.description.as_deref()),
        Some("Timeout in milliseconds. Defaults to 30000, min 10000, max 3600000.")
    );
    assert_eq!(parameters.required.as_ref(), None);
    let output_schema = output_schema.expect("wait output schema");
    assert_eq!(
        output_schema["properties"]["message"]["description"],
        json!("Brief wait summary without the agent's final content.")
    );
    assert_eq!(
        output_schema["required"],
        json!([
            "message",
            "timed_out",
            "waiting_for",
            "wake_conditions",
            "consecutive_empty_waits",
            "watchdog_escalated",
            "agents"
        ])
    );
    assert_eq!(
        output_schema["properties"]["agents"]["items"]["required"],
        json!([
            "agent_name",
            "agent_nickname",
            "agent_role",
            "agent_status",
            "last_task_message",
            "last_result_message"
        ])
    );
}

#[test]
fn list_agents_tool_includes_path_prefix_and_agent_fields() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_list_agents_tool()
    else {
        panic!("list_agents should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("list_agents should use object params");
    assert!(properties.contains_key("path_prefix"));
    assert_eq!(
        properties
            .get("path_prefix")
            .and_then(|schema| schema.description.as_deref()),
        Some("Task-path prefix filter without a trailing slash. Omit to list all live agents.")
    );
    assert_eq!(
        output_schema.expect("list_agents output schema")["properties"]["agents"]["items"]["required"],
        json!(["agent_name", "agent_status"])
    );
}

#[test]
fn list_agents_tool_status_schema_includes_interrupted() {
    let ToolSpec::Function(ResponsesApiTool { output_schema, .. }) = create_list_agents_tool()
    else {
        panic!("list_agents should be a function tool");
    };

    assert_eq!(
        output_schema.expect("list_agents output schema")["properties"]["agents"]["items"]["properties"]
            ["agent_status"]["allOf"][0]["oneOf"][0]["enum"],
        json!([
            "pending_init",
            "running",
            "interrupted",
            "shutdown",
            "not_found"
        ])
    );
}

#[test]
fn openai_reserved_collaboration_profile_restores_pinned_argument_contracts() {
    let spawn = apply_openai_reserved_collaboration_schema(create_spawn_agent_tool_v2(
        SpawnAgentToolOptions {
            available_models: vec![model_preset("visible", /*show_in_picker*/ true)],
            inherited_runtime: None,
            agent_type_description: "role help".to_string(),
            expose_agent_type: true,
            hide_agent_type_model_reasoning: false,
            expose_spawn_agent_model_overrides: true,
            multi_agent_version: MultiAgentVersion::V2,
            usage_hint_text: None,
        },
    ));
    let ToolSpec::Function(spawn) = spawn else {
        panic!("spawn_agent should be a function tool");
    };
    let spawn_properties = spawn
        .parameters
        .properties
        .expect("spawn_agent should use object params");
    assert_eq!(
        spawn_properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        Some(true)
    );
    assert_eq!(
        spawn_properties.keys().cloned().collect::<Vec<_>>(),
        vec![
            "fork_turns".to_string(),
            "message".to_string(),
            "task_name".to_string()
        ]
    );
    assert_eq!(
        spawn.output_schema.expect("spawn output schema")["required"],
        json!(["task_name"])
    );

    for tool in [create_send_message_tool(), create_followup_task_tool()] {
        let ToolSpec::Function(tool) = apply_openai_reserved_collaboration_schema(tool) else {
            panic!("message collaboration tool should be a function");
        };
        assert_eq!(
            tool.parameters
                .properties
                .as_ref()
                .and_then(|properties| properties.get("message"))
                .and_then(|schema| schema.encrypted),
            Some(true)
        );
    }

    let ToolSpec::Function(interrupt) =
        apply_openai_reserved_collaboration_schema(create_interrupt_agent_tool_v2())
    else {
        panic!("interrupt_agent should be a function");
    };
    let interrupt_properties = interrupt
        .parameters
        .properties
        .expect("interrupt_agent should use object params");
    assert_eq!(
        interrupt_properties.keys().cloned().collect::<Vec<_>>(),
        vec!["target".to_string()]
    );
    assert_eq!(
        interrupt.parameters.required,
        Some(vec!["target".to_string()])
    );
    assert_eq!(
        interrupt.output_schema.expect("interrupt output schema")["required"],
        json!(["previous_status"])
    );
}

#[test]
fn openai_reserved_collaboration_profile_restores_pinned_result_contracts() {
    let ToolSpec::Function(wait) = apply_openai_reserved_collaboration_schema(
        create_wait_agent_tool_v2(WaitAgentTimeoutOptions::default()),
    ) else {
        panic!("wait_agent should be a function");
    };
    assert_eq!(
        wait.output_schema.expect("wait output schema")["required"],
        json!(["message", "timed_out"])
    );

    let ToolSpec::Function(list) =
        apply_openai_reserved_collaboration_schema(create_list_agents_tool())
    else {
        panic!("list_agents should be a function");
    };
    assert_eq!(
        list.output_schema.expect("list output schema")["properties"]["agents"]["items"]["required"],
        json!(["agent_name", "agent_status"])
    );
}
