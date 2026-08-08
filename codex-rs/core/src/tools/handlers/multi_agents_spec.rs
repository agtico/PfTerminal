use super::multi_agents_common::MAX_SPAWN_AGENT_MODEL_OVERRIDES;
use super::multi_agents_common::model_supports_multi_agent_backend;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelBilling;
use codex_protocol::openai_models::ModelCapabilityTier;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::MultiAgentVersion;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

pub const MULTI_AGENT_V1_NAMESPACE: &str = "multi_agent_v1";
const MULTI_AGENT_V1_NAMESPACE_DESCRIPTION: &str = "Tools for spawning and managing sub-agents.";

const SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V1: &str = "Spawned agents inherit your current model by default. Omit `model` to use that preferred default; set `model` only when an explicit override is needed.";
const SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V2: &str = "Spawned agents inherit your current provider and model by default. Omit both `model_provider` and `model` to inherit that runtime. To use another runtime, set both fields explicitly.";
const SPAWN_AGENT_TYPE_OVERRIDE_DESCRIPTION_V1: &str = "Agent type override for the new agent. Omit to inherit the parent agent type with a full-history fork; otherwise, `default` is used.";
const SPAWN_AGENT_MODEL_OVERRIDE_DESCRIPTION: &str =
    "Model override for the new agent. Omit unless an explicit override is needed.";
const SPAWN_AGENT_PROVIDER_OVERRIDE_DESCRIPTION: &str = "Provider override for the new agent. Set this together with `model`; omit both to inherit the parent runtime.";
const SPAWN_AGENT_SERVICE_TIER_OVERRIDE_DESCRIPTION: &str =
    "Service tier override for the new agent. Omit unless explicitly requested.";
const MAX_REASONING_EFFORT_CHARS_IN_SPAWN_AGENT_DESCRIPTION: usize = 64;
const MAX_MODEL_OVERRIDES_IN_SPAWN_AGENT_DESCRIPTION: usize = MAX_SPAWN_AGENT_MODEL_OVERRIDES;

#[derive(Debug, Clone)]
pub struct SpawnAgentToolOptions {
    pub available_models: Vec<ModelPreset>,
    pub inherited_runtime: Option<SpawnAgentRuntime>,
    pub agent_type_description: String,
    pub expose_agent_type: bool,
    pub hide_agent_type_model_reasoning: bool,
    pub expose_spawn_agent_model_overrides: bool,
    pub multi_agent_version: MultiAgentVersion,
    pub usage_hint_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnAgentRuntime {
    pub model_provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub service_tier: Option<String>,
}

impl Default for SpawnAgentToolOptions {
    fn default() -> Self {
        Self {
            available_models: Vec::new(),
            inherited_runtime: None,
            agent_type_description: String::new(),
            expose_agent_type: true,
            hide_agent_type_model_reasoning: false,
            expose_spawn_agent_model_overrides: false,
            multi_agent_version: MultiAgentVersion::Disabled,
            usage_hint_text: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitAgentTimeoutOptions {
    pub default_timeout_ms: i64,
    pub min_timeout_ms: i64,
    pub max_timeout_ms: i64,
}

impl Default for WaitAgentTimeoutOptions {
    fn default() -> Self {
        Self {
            default_timeout_ms: super::multi_agents_common::DEFAULT_WAIT_TIMEOUT_MS,
            min_timeout_ms: super::multi_agents_common::MIN_WAIT_TIMEOUT_MS,
            max_timeout_ms: super::multi_agents_common::MAX_WAIT_TIMEOUT_MS,
        }
    }
}

pub fn create_spawn_agent_tool_v1(options: SpawnAgentToolOptions) -> ToolSpec {
    let available_models_description = (!options.hide_agent_type_model_reasoning).then(|| {
        spawn_agent_models_description(
            &options.available_models,
            options.multi_agent_version,
            options.inherited_runtime.as_ref(),
        )
    });
    let inherited_model_guidance = (!options.hide_agent_type_model_reasoning)
        .then_some(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V1);
    let return_value_description =
        "Returns the spawned agent id plus the user-facing nickname when available.";
    let mut properties = spawn_agent_common_properties_v1(&options.agent_type_description);
    if !options.expose_agent_type {
        properties.remove("agent_type");
    }
    if options.hide_agent_type_model_reasoning {
        hide_spawn_agent_metadata_options(&mut properties);
    }

    ToolSpec::Namespace(ResponsesApiNamespace {
        name: MULTI_AGENT_V1_NAMESPACE.to_string(),
        description: MULTI_AGENT_V1_NAMESPACE_DESCRIPTION.to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "spawn_agent".to_string(),
            description: spawn_agent_tool_description(
                available_models_description.as_deref(),
                inherited_model_guidance,
                return_value_description,
                options.usage_hint_text,
            ),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
            output_schema: Some(spawn_agent_output_schema_v1()),
        })],
    })
}

pub fn create_spawn_agent_tool_v2(options: SpawnAgentToolOptions) -> ToolSpec {
    let available_models_description = options.expose_spawn_agent_model_overrides.then(|| {
        spawn_agent_models_description(
            &options.available_models,
            options.multi_agent_version,
            options.inherited_runtime.as_ref(),
        )
    });
    let inherited_model_guidance = (options.expose_spawn_agent_model_overrides
        && !options.hide_agent_type_model_reasoning)
        .then_some(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V2);
    let mut properties = spawn_agent_common_properties_v2(&options.agent_type_description);
    if !options.expose_agent_type {
        properties.remove("agent_type");
    }
    if options.hide_agent_type_model_reasoning {
        properties.remove("service_tier");
    }
    if !options.expose_spawn_agent_model_overrides {
        properties.remove("model");
        properties.remove("reasoning_effort");
    }
    properties.insert(
        "task_name".to_string(),
        JsonSchema::string(Some(
            "Task name for the new agent. Use lowercase letters, digits, and underscores."
                .to_string(),
        )),
    );

    ToolSpec::Function(ResponsesApiTool {
        name: "spawn_agent".to_string(),
        description: spawn_agent_tool_description_v2(
            available_models_description.as_deref(),
            inherited_model_guidance,
            options.usage_hint_text,
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["task_name".to_string(), "message".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(spawn_agent_output_schema_v2(
            options.hide_agent_type_model_reasoning,
        )),
    })
}

pub fn create_send_input_tool_v1() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "target".to_string(),
            JsonSchema::string(Some("Agent id to message (from spawn_agent).".to_string())),
        ),
        (
            "message".to_string(),
            JsonSchema::string(Some(
                "Legacy plain-text message to send to the agent. Use either message or items."
                    .to_string(),
            )),
        ),
        ("items".to_string(), create_collab_input_items_schema()),
        (
            "interrupt".to_string(),
            JsonSchema::boolean(Some(
                "True interrupts the current task and handles this message immediately; false or omitted queues it."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Namespace(ResponsesApiNamespace {
        name: MULTI_AGENT_V1_NAMESPACE.to_string(),
        description: MULTI_AGENT_V1_NAMESPACE_DESCRIPTION.to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "send_input".to_string(),
            description: "Send a message to an existing agent. Use interrupt=true to redirect work immediately. You should reuse the agent by send_input if you believe your assigned task is highly dependent on the context of a previous task."
                .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(properties, Some(vec!["target".to_string()]), Some(false.into())),
            output_schema: Some(send_input_output_schema()),
        })],
    })
}

pub fn create_send_message_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "target".to_string(),
            JsonSchema::string(Some(
                "Relative or canonical task name to message (from spawn_agent).".to_string(),
            )),
        ),
        (
            "message".to_string(),
            JsonSchema::string(Some(
                "Message text to queue on the target agent.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "send_message".to_string(),
        description: "Send a message to an existing agent. The message will be delivered promptly. Does not trigger a new turn."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["target".to_string(), "message".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_followup_task_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "target".to_string(),
            JsonSchema::string(Some(
                "Agent id or canonical task name to send a follow-up task to (from spawn_agent)."
                    .to_string(),
            )),
        ),
        (
            "message".to_string(),
            JsonSchema::string(Some(
                "Message text to send to the target agent.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "followup_task".to_string(),
        description: "Send a follow-up task to an existing non-root target agent and trigger a turn if it is idle. If the target is already running, deliver the task promptly at message boundaries while sampling, or after the pending tool call completes."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec!["target".to_string(), "message".to_string()]), Some(false.into())),
        output_schema: None,
    })
}

pub fn create_resume_agent_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "id".to_string(),
        JsonSchema::string(Some("Agent id to resume.".to_string())),
    )]);

    ToolSpec::Namespace(ResponsesApiNamespace {
        name: MULTI_AGENT_V1_NAMESPACE.to_string(),
        description: MULTI_AGENT_V1_NAMESPACE_DESCRIPTION.to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "resume_agent".to_string(),
            description:
                "Resume a previously closed agent by id so it can receive send_input and wait_agent calls."
                    .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(properties, Some(vec!["id".to_string()]), Some(false.into())),
            output_schema: Some(resume_agent_output_schema()),
        })],
    })
}

pub fn create_wait_agent_tool_v1(options: WaitAgentTimeoutOptions) -> ToolSpec {
    ToolSpec::Namespace(ResponsesApiNamespace {
        name: MULTI_AGENT_V1_NAMESPACE.to_string(),
        description: MULTI_AGENT_V1_NAMESPACE_DESCRIPTION.to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "wait_agent".to_string(),
            description: "Wait for agents to reach a final status. Completed statuses may include the agent's final message. Returns empty status when timed out. Once the agent reaches a final status, a notification message will be received containing the same completed status."
                .to_string(),
            strict: false,
            defer_loading: None,
            parameters: wait_agent_tool_parameters_v1(options),
            output_schema: Some(wait_output_schema_v1()),
        })],
    })
}

pub fn create_wait_agent_tool_v2(options: WaitAgentTimeoutOptions) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: "wait_agent".to_string(),
        description: "Wait for a mailbox update from any live agent, including queued messages and final-status notifications. The wait also ends early when new user input is steered into the active turn. Does not return the content; returns either a summary of which agents have updates (if any), an interruption summary for steered input, or a timeout summary if no activity arrives before the deadline."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: wait_agent_tool_parameters_v2(options),
        output_schema: Some(wait_output_schema_v2()),
    })
}

pub fn create_list_agents_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "path_prefix".to_string(),
        JsonSchema::string(Some(
            "Task-path prefix filter without a trailing slash. Omit to list all live agents."
                .to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "list_agents".to_string(),
        description:
            "List live agents in the current root thread tree. Optionally filter by task-path prefix."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: Some(list_agents_output_schema()),
    })
}

pub fn create_close_agent_tool_v1() -> ToolSpec {
    let properties = BTreeMap::from([(
        "target".to_string(),
        JsonSchema::string(Some("Agent id to close (from spawn_agent).".to_string())),
    )]);

    ToolSpec::Namespace(ResponsesApiNamespace {
        name: MULTI_AGENT_V1_NAMESPACE.to_string(),
        description: MULTI_AGENT_V1_NAMESPACE_DESCRIPTION.to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "close_agent".to_string(),
            description: "Close an agent and any open descendants when they are no longer needed, and return the target agent's previous status before shutdown was requested. Completed agents remain open and count toward the concurrency limit until closed. Don't keep agents open for too long if they are not needed anymore.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(properties, Some(vec!["target".to_string()]), Some(false.into())),
            output_schema: Some(agent_previous_status_output_schema(
                "The agent status observed before shutdown was requested.",
            )),
        })],
    })
}

pub fn create_interrupt_agent_tool_v2() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "target".to_string(),
            JsonSchema::string(Some(
                "Agent id or canonical task name to interrupt (from spawn_agent).".to_string(),
            )),
        ),
        (
            "reason".to_string(),
            JsonSchema::string(Some(
                "Operator-visible reason for interrupting the current turn.".to_string(),
            )),
        ),
        (
            "superseding_task".to_string(),
            JsonSchema::string(Some(
                "Replacement instruction or dispatch identifier, when this interrupt supersedes work."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "interrupt_agent".to_string(),
        description: "Interrupt an agent's current model turn with an operator-visible reason, and return its previous status. The agent remains available for messages and follow-up tasks.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["target".to_string(), "reason".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(interrupt_agent_output_schema()),
    })
}

/// Restore the pinned first-party Responses contract for collaboration tools.
///
/// OpenAI reserves the `collaboration.*` function names and rejects requests when their schemas
/// diverge from the native Codex contract. PF Terminal may expose richer routing and lifecycle
/// metadata to other providers, but that metadata must stay outside the reserved first-party wire
/// schema. The runtime handlers intentionally continue to accept the provider-neutral superset.
pub(crate) fn apply_openai_reserved_collaboration_schema(spec: ToolSpec) -> ToolSpec {
    match spec {
        ToolSpec::Function(tool) => {
            ToolSpec::Function(apply_openai_reserved_collaboration_function_schema(tool))
        }
        ToolSpec::Namespace(mut namespace) => {
            namespace.tools = namespace
                .tools
                .into_iter()
                .map(|tool| match tool {
                    ResponsesApiNamespaceTool::Function(tool) => {
                        ResponsesApiNamespaceTool::Function(
                            apply_openai_reserved_collaboration_function_schema(tool),
                        )
                    }
                    ResponsesApiNamespaceTool::Custom(tool) => {
                        ResponsesApiNamespaceTool::Custom(tool)
                    }
                })
                .collect();
            ToolSpec::Namespace(namespace)
        }
        spec => spec,
    }
}

fn apply_openai_reserved_collaboration_function_schema(
    mut tool: ResponsesApiTool,
) -> ResponsesApiTool {
    match tool.name.as_str() {
        "spawn_agent" => {
            if let Some(properties) = tool.parameters.properties.as_mut() {
                // The first-party reserved surface deliberately contains only assignment,
                // canonical task name, and history-fork controls. PF role and runtime selection
                // are applied by the product's typed spawn request before this model-visible
                // protocol boundary.
                for property in [
                    "agent_type",
                    "model_provider",
                    "model",
                    "reasoning_effort",
                    "service_tier",
                ] {
                    properties.remove(property);
                }
                if let Some(message) = properties.get_mut("message") {
                    message.encrypted = Some(true);
                }
            }
            tool.description = spawn_agent_tool_description_v2(
                /*available_models_description*/ None, /*inherited_model_guidance*/ None,
                /*usage_hint_text*/ None,
            );
            tool.output_schema = Some(openai_spawn_agent_output_schema_v2(
                /*hide_agent_metadata*/ true,
            ));
        }
        "send_message" | "followup_task" => {
            if let Some(message) = tool
                .parameters
                .properties
                .as_mut()
                .and_then(|properties| properties.get_mut("message"))
            {
                message.encrypted = Some(true);
            }
        }
        "wait_agent" => {
            tool.output_schema = Some(openai_wait_output_schema_v2());
        }
        "interrupt_agent" => {
            let properties = BTreeMap::from([(
                "target".to_string(),
                JsonSchema::string(Some(
                    "Agent id or canonical task name to interrupt (from spawn_agent).".to_string(),
                )),
            )]);
            tool.description = "Interrupt an agent's current turn, if any, and return its previous status. The agent remains available for messages and follow-up tasks.".to_string();
            tool.parameters = JsonSchema::object(
                properties,
                Some(vec!["target".to_string()]),
                Some(false.into()),
            );
            tool.output_schema = Some(agent_previous_status_output_schema(
                "The agent status observed before the interrupt request was handled.",
            ));
        }
        "list_agents" => {
            tool.output_schema = Some(openai_list_agents_output_schema());
        }
        _ => {}
    }
    tool
}

fn interrupt_agent_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "previous_status": {
                "description": "The agent status observed before the interrupt request was handled.",
                "allOf": [agent_status_output_schema()]
            },
            "actor": {"type": "string"},
            "target": {"type": "string"},
            "reason": {"type": "string"},
            "superseding_task": {"type": ["string", "null"]},
            "process_effect": {"type": "string"}
        },
        "required": ["previous_status", "actor", "target", "reason", "superseding_task", "process_effect"],
        "additionalProperties": false
    })
}

fn agent_status_output_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "string",
                "enum": ["pending_init", "running", "interrupted", "shutdown", "not_found"]
            },
            {
                "type": "object",
                "properties": {
                    "completed": {
                        "type": ["string", "null"]
                    }
                },
                "required": ["completed"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "errored": {
                        "type": "string"
                    }
                },
                "required": ["errored"],
                "additionalProperties": false
            }
        ]
    })
}

fn spawn_agent_output_schema_v1() -> Value {
    json!({
        "type": "object",
        "properties": {
            "agent_id": {
                "type": "string",
                "description": "Thread identifier for the spawned agent."
            },
            "nickname": {
                "type": ["string", "null"],
                "description": "User-facing nickname for the spawned agent when available."
            }
        },
        "required": ["agent_id", "nickname"],
        "additionalProperties": false
    })
}

fn spawn_agent_output_schema_v2(hide_agent_metadata: bool) -> Value {
    if hide_agent_metadata {
        return json!({
            "type": "object",
            "properties": {
                "task_name": {
                    "type": "string",
                    "description": "Canonical task name for the spawned agent."
                }
            },
            "required": ["task_name"],
            "additionalProperties": false
        });
    }

    json!({
        "type": "object",
        "properties": {
            "task_name": {
                "type": "string",
                "description": "Canonical task name for the spawned agent."
            },
            "nickname": {
                "type": ["string", "null"],
                "description": "User-facing nickname for the spawned agent when available."
            },
            "model_provider": {
                "type": "string",
                "description": "Resolved provider used by the spawned agent."
            },
            "model": {
                "type": "string",
                "description": "Resolved model used by the spawned agent."
            },
            "reasoning_effort": {
                "type": ["string", "null"],
                "description": "Resolved reasoning effort used by the spawned agent."
            },
            "service_tier": {
                "type": ["string", "null"],
                "description": "Resolved service tier used by the spawned agent."
            }
        },
        "required": [
            "task_name",
            "nickname",
            "model_provider",
            "model",
            "reasoning_effort",
            "service_tier"
        ],
        "additionalProperties": false
    })
}

fn openai_spawn_agent_output_schema_v2(hide_agent_metadata: bool) -> Value {
    if hide_agent_metadata {
        return json!({
            "type": "object",
            "properties": {
                "task_name": {
                    "type": "string",
                    "description": "Canonical task name for the spawned agent."
                }
            },
            "required": ["task_name"],
            "additionalProperties": false
        });
    }

    json!({
        "type": "object",
        "properties": {
            "task_name": {
                "type": "string",
                "description": "Canonical task name for the spawned agent."
            },
            "nickname": {
                "type": ["string", "null"],
                "description": "User-facing nickname for the spawned agent when available."
            }
        },
        "required": ["task_name", "nickname"],
        "additionalProperties": false
    })
}

fn send_input_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "submission_id": {
                "type": "string",
                "description": "Identifier for the queued input submission."
            }
        },
        "required": ["submission_id"],
        "additionalProperties": false
    })
}

fn list_agents_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "agents": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "agent_name": {
                            "type": "string",
                            "description": "Canonical task name for the agent when available, otherwise the agent id."
                        },
                        "agent_nickname": {
                            "type": ["string", "null"],
                            "description": "User-facing nickname for the agent when available."
                        },
                        "agent_role": {
                            "type": ["string", "null"],
                            "description": "Role name for the agent when available, for example troll or orc."
                        },
                        "agent_status": {
                            "description": "Last known status of the agent.",
                            "allOf": [agent_status_output_schema()]
                        }
                    },
                    "required": ["agent_name", "agent_status"],
                    "additionalProperties": false
                },
                "description": "Live agents visible in the current root thread tree."
            }
        },
        "required": ["agents"],
        "additionalProperties": false
    })
}

fn openai_list_agents_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "agents": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "agent_name": {
                            "type": "string",
                            "description": "Canonical task name for the agent when available, otherwise the agent id."
                        },
                        "agent_status": {
                            "description": "Last known status of the agent.",
                            "allOf": [agent_status_output_schema()]
                        }
                    },
                    "required": ["agent_name", "agent_status"],
                    "additionalProperties": false
                },
                "description": "Live agents visible in the current root thread tree."
            }
        },
        "required": ["agents"],
        "additionalProperties": false
    })
}

fn resume_agent_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "status": agent_status_output_schema()
        },
        "required": ["status"],
        "additionalProperties": false
    })
}

fn wait_output_schema_v1() -> Value {
    json!({
        "type": "object",
        "properties": {
            "status": {
                "type": "object",
                "description": "Final statuses keyed by agent id.",
                "additionalProperties": agent_status_output_schema()
            },
            "timed_out": {
                "type": "boolean",
                "description": "Whether the wait call returned due to timeout before any agent reached a final status."
            }
        },
        "required": ["status", "timed_out"],
        "additionalProperties": false
    })
}

fn wait_output_schema_v2() -> Value {
    json!({
        "type": "object",
        "properties": {
            "message": {
                "type": "string",
                "description": "Brief wait summary without the agent's final content."
            },
            "timed_out": {
                "type": "boolean",
                "description": "Whether the wait call returned because no mailbox update arrived before the timeout."
            },
            "waiting_for": {
                "type": "string",
                "description": "The eligible child-agent subtree whose mailbox activity is being awaited."
            },
            "wake_conditions": {
                "type": "string",
                "description": "The events that end the wait."
            },
            "consecutive_empty_waits": {"type": "integer"},
            "watchdog_escalated": {"type": "boolean"},
            "agents": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "agent_name": {
                            "type": "string",
                            "description": "Canonical task name for the agent when available, otherwise the agent id."
                        },
                        "agent_nickname": {
                            "type": ["string", "null"],
                            "description": "User-facing nickname for the agent when available."
                        },
                        "agent_role": {
                            "type": ["string", "null"],
                            "description": "Role name for the agent when available, for example troll or orc."
                        },
                        "agent_status": {
                            "description": "Last known status of the agent.",
                            "allOf": [agent_status_output_schema()]
                        },
                        "last_task_message": {
                            "type": ["string", "null"],
                            "description": "Most recent user or inter-agent instruction received by the agent, when available."
                        },
                        "last_result_message": {
                            "type": ["string", "null"],
                            "description": "Bounded preview of the agent's most recent final result or terminal error, when available."
                        }
                    },
                    "required": ["agent_name", "agent_nickname", "agent_role", "agent_status", "last_task_message", "last_result_message"],
                    "additionalProperties": false
                },
                "description": "Live agents visible in the current root thread tree after the wait."
            }
        },
        "required": ["message", "timed_out", "waiting_for", "wake_conditions", "consecutive_empty_waits", "watchdog_escalated", "agents"],
        "additionalProperties": false
    })
}

fn openai_wait_output_schema_v2() -> Value {
    json!({
        "type": "object",
        "properties": {
            "message": {
                "type": "string",
                "description": "Brief wait summary without the agent's final content."
            },
            "timed_out": {
                "type": "boolean",
                "description": "Whether the wait call returned because no mailbox update arrived before the timeout."
            }
        },
        "required": ["message", "timed_out"],
        "additionalProperties": false
    })
}

fn agent_previous_status_output_schema(previous_status_description: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "previous_status": {
                "description": previous_status_description,
                "allOf": [agent_status_output_schema()]
            }
        },
        "required": ["previous_status"],
        "additionalProperties": false
    })
}

fn create_collab_input_items_schema() -> JsonSchema {
    let properties = BTreeMap::from([
        (
            "type".to_string(),
            JsonSchema::string(Some(
                "Input item type: text, image, local_image, audio, local_audio, skill, or mention."
                    .to_string(),
            )),
        ),
        (
            "text".to_string(),
            JsonSchema::string(Some("Text content when type is text.".to_string())),
        ),
        (
            "image_url".to_string(),
            JsonSchema::string(Some("Image URL when type is image.".to_string())),
        ),
        (
            "audio_url".to_string(),
            JsonSchema::string(Some("Audio data URL when type is audio.".to_string())),
        ),
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path when type is local_image/local_audio/skill, or structured mention target such as app://<connector-id> or plugin://<plugin-name>@<marketplace-name> when type is mention."
                    .to_string(),
            )),
        ),
        (
            "name".to_string(),
            JsonSchema::string(Some("Display name when type is skill or mention.".to_string())),
        ),
    ]);

    JsonSchema::array(JsonSchema::object(properties, /*required*/ None, Some(false.into())), Some(
            "Structured input items. Use this to pass explicit mentions (for example app:// connector paths)."
                .to_string(),
        ))
}

fn spawn_agent_common_properties_v1(agent_type_description: &str) -> BTreeMap<String, JsonSchema> {
    BTreeMap::from([
        (
            "message".to_string(),
            JsonSchema::string(Some(
                "Initial plain-text task for the new agent. Use either message or items."
                    .to_string(),
            )),
        ),
        ("items".to_string(), create_collab_input_items_schema()),
        (
            "agent_type".to_string(),
            JsonSchema::string(Some(format!(
                "{SPAWN_AGENT_TYPE_OVERRIDE_DESCRIPTION_V1}\n{agent_type_description}"
            ))),
        ),
        (
            "fork_context".to_string(),
            JsonSchema::boolean(Some(
                "True forks the current thread history into the new agent; false or omitted starts with only the initial prompt."
                    .to_string(),
            )),
        ),
        (
            "model".to_string(),
            JsonSchema::string(Some(
                SPAWN_AGENT_MODEL_OVERRIDE_DESCRIPTION.to_string(),
            )),
        ),
        (
            "reasoning_effort".to_string(),
            JsonSchema::string(Some(
                "Reasoning effort override for the new agent. Omit to inherit the parent effort."
                    .to_string(),
            )),
        ),
        (
            "service_tier".to_string(),
            JsonSchema::string(Some(
                SPAWN_AGENT_SERVICE_TIER_OVERRIDE_DESCRIPTION.to_string(),
            )),
        ),
    ])
}

fn spawn_agent_common_properties_v2(agent_type_description: &str) -> BTreeMap<String, JsonSchema> {
    BTreeMap::from([
        (
            "message".to_string(),
            JsonSchema::string(Some(
                "Initial plain-text task for the new agent.".to_string(),
            )),
        ),
        (
            "agent_type".to_string(),
            JsonSchema::string(Some(format!(
                "Agent type override for the new agent. Omit unless explicitly asked. Set `fork_turns` to `none` or a positive integer when an explicit override is needed.\n{agent_type_description}"
            ))),
        ),
        (
            "fork_turns".to_string(),
            JsonSchema::string(Some(
                "Optional number of turns to fork. Defaults to `all`. Use `none`, `all`, or a positive integer string such as `3` to fork only the most recent turns."
                    .to_string(),
            )),
        ),
        (
            "model".to_string(),
            JsonSchema::string(Some(
                SPAWN_AGENT_MODEL_OVERRIDE_DESCRIPTION.to_string(),
            )),
        ),
        (
            "model_provider".to_string(),
            JsonSchema::string(Some(
                SPAWN_AGENT_PROVIDER_OVERRIDE_DESCRIPTION.to_string(),
            )),
        ),
        (
            "reasoning_effort".to_string(),
            JsonSchema::string(Some(
                "Reasoning effort override for the new agent. Omit to inherit the parent effort."
                    .to_string(),
            )),
        ),
        (
            "service_tier".to_string(),
            JsonSchema::string(Some(
                SPAWN_AGENT_SERVICE_TIER_OVERRIDE_DESCRIPTION.to_string(),
            )),
        ),
    ])
}

fn hide_spawn_agent_metadata_options(properties: &mut BTreeMap<String, JsonSchema>) {
    properties.remove("agent_type");
    properties.remove("model");
    properties.remove("model_provider");
    properties.remove("reasoning_effort");
    properties.remove("service_tier");
}

fn spawn_agent_tool_description(
    available_models_description: Option<&str>,
    inherited_model_guidance: Option<&str>,
    return_value_description: &str,
    usage_hint_text: Option<String>,
) -> String {
    let agent_role_guidance = available_models_description.unwrap_or_default();
    let inherited_model_guidance = inherited_model_guidance.unwrap_or_default();

    let tool_description = format!(
        r#"
        {agent_role_guidance}
        Spawn a sub-agent for a well-scoped task. {return_value_description} {inherited_model_guidance}"#
    );

    if let Some(usage_hint_text) = usage_hint_text {
        return format!(
            r#"
        {tool_description}
{usage_hint_text}"#
        );
    }
    let agent_role_usage_hint = available_models_description
        .map(|_| {
            "Agent-role guidance below only helps choose which agent to use after spawning is already authorized; it never authorizes spawning by itself."
        })
        .unwrap_or_default();
    format!(
        r#"
        {tool_description}
This spawn_agent tool provides you access to sub-agents that inherit your current model by default. Do not set the `model` field unless the user explicitly asks for a different model or there is a clear task-specific reason. You should follow the rules and guidelines below to use this tool.

Do not spawn sub-agents unless the user or applicable AGENTS.md/skill instructions explicitly ask for sub-agents, delegation, or parallel agent work.
Requests for depth, thoroughness, research, investigation, or detailed codebase analysis do not count as permission to spawn.
{agent_role_usage_hint}

### When to delegate vs. do the subtask yourself
- First, quickly analyze the overall user task and form a succinct high-level plan. Identify which tasks are immediate blockers on the critical path, and which tasks are sidecar tasks that are needed but can run in parallel without blocking the next local step. As part of that plan, explicitly decide what immediate task you should do locally right now. Do this planning step before delegating to agents so you do not hand off the immediate blocking task to a submodel and then waste time waiting on it.
- Use a subagent when a subtask is easy enough for it to handle and can run in parallel with your local work. Prefer delegating concrete, bounded sidecar tasks that materially advance the main task without blocking your immediate next local step.
- Do not delegate urgent blocking work when your immediate next step depends on that result. If the very next action is blocked on that task, the main rollout should usually do it locally to keep the critical path moving.
- Keep work local when the subtask is too difficult to delegate well and when it is tightly coupled, urgent, or likely to block your immediate next step.

### Designing delegated subtasks
- Subtasks must be concrete, well-defined, and self-contained.
- Delegated subtasks must materially advance the main task.
- Do not duplicate work between the main rollout and delegated subtasks.
- Avoid issuing multiple delegate calls on the same unresolved thread unless the new delegated task is genuinely different and necessary.
- Narrow the delegated ask to the concrete output you need next.
- For coding tasks, prefer delegating concrete code-change worker subtasks over read-only explorer analysis when the subagent can make a bounded patch in a clear write scope.
- When delegating coding work, instruct the submodel to edit files directly in its forked workspace and list the file paths it changed in the final answer.
- For code-edit subtasks, decompose work so each delegated task has a disjoint write set.

### After you delegate
- Call wait_agent very sparingly. Only call wait_agent when you need the result immediately for the next critical-path step and you are blocked until it returns.
- Do not redo delegated subagent tasks yourself; focus on integrating results or tackling non-overlapping work.
- While the subagent is running in the background, do meaningful non-overlapping work immediately.
- Do not repeatedly wait by reflex.
- When a delegated coding task returns, quickly review the uploaded changes, then integrate or refine them.

### Parallel delegation patterns
- Run multiple independent information-seeking subtasks in parallel when you have distinct questions that can be answered independently.
- Split implementation into disjoint codebase slices and spawn multiple agents for them in parallel when the write scopes do not overlap.
- Delegate verification only when it can run in parallel with ongoing implementation and is likely to catch a concrete risk before final integration.
- The key is to find opportunities to spawn multiple independent subtasks in parallel within the same round, while ensuring each subtask is well-defined, self-contained, and materially advances the main task."#
    )
}

fn spawn_agent_tool_description_v2(
    available_models_description: Option<&str>,
    inherited_model_guidance: Option<&str>,
    usage_hint_text: Option<String>,
) -> String {
    let agent_role_guidance = available_models_description.unwrap_or_default();
    let inherited_model_guidance = inherited_model_guidance.unwrap_or_default();

    let tool_description = format!(
        r#"
        {agent_role_guidance}
        Spawns an agent to work on the specified task. If your current task is `/root/task1` and you spawn_agent with task_name "task_3" the agent will have canonical task name `/root/task1/task_3`.
You are then able to refer to this agent as `task_3` or `/root/task1/task_3` interchangeably. However an agent `/root/task2/task_3` would only be able to communicate with this agent via its canonical name `/root/task1/task_3`.
The spawned agent will have the same tools as you and the ability to spawn its own subagents.
{inherited_model_guidance}
Only call this tool for a concrete, bounded subtask that can run independently alongside useful local work; otherwise continue locally.
It will be able to send you and other running agents messages, and its final answer will be provided to you when it finishes.
The new agent's canonical task name will be provided to it along with the message.

Note that passing `fork_turns="none"` will not pass any surrounding context to the spawned subagent, which may cause the agent to lack the context it needs to complete its task, whereas `fork_turns="all"` will provide the subagent with all surrounding context."#
    );

    if let Some(usage_hint_text) = usage_hint_text {
        return format!(
            r#"
        {tool_description}
{usage_hint_text}"#
        );
    }
    tool_description
}

fn spawn_agent_models_description(
    models: &[ModelPreset],
    multi_agent_version: MultiAgentVersion,
    inherited_runtime: Option<&SpawnAgentRuntime>,
) -> String {
    let inherited_runtime = inherited_runtime.map_or_else(
        || "Current inherited runtime: unavailable.".to_string(),
        |runtime| {
            let effort = runtime
                .reasoning_effort
                .as_ref()
                .map_or("provider default", ReasoningEffort::as_str);
            let service_tier = runtime
                .service_tier
                .as_deref()
                .map_or_else(String::new, |tier| format!("; service tier {tier}"));
            format!(
                "Current inherited runtime: `{}` / `{}`; effort {effort}{service_tier}.",
                runtime.model_provider, runtime.model
            )
        },
    );
    let visible_models: Vec<&ModelPreset> = models
        .iter()
        .filter(|model| model.show_in_picker)
        .filter(|model| model_supports_multi_agent_backend(model, multi_agent_version))
        .take(MAX_MODEL_OVERRIDES_IN_SPAWN_AGENT_DESCRIPTION)
        .collect();
    if visible_models.is_empty() {
        return format!(
            "{inherited_runtime}\nNo authorized picker-visible model overrides are currently loaded."
        );
    }

    let model_descriptions = visible_models
        .into_iter()
        .map(|model| {
            let default_reasoning_effort = &model.default_reasoning_effort;
            let efforts = model
                .supported_reasoning_efforts
                .iter()
                .map(|preset| {
                    let effort = preset.effort.as_str();
                    let effort = match effort
                        .char_indices()
                        .nth(MAX_REASONING_EFFORT_CHARS_IN_SPAWN_AGENT_DESCRIPTION)
                    {
                        Some((index, _)) => &effort[..index],
                        None => effort,
                    };
                    if &preset.effort == default_reasoning_effort {
                        format!("{effort} (default)")
                    } else {
                        effort.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let reasoning_efforts_suffix = if efforts.is_empty() {
                String::new()
            } else {
                format!(" Reasoning efforts: {efforts}.")
            };
            let service_tiers = model
                .service_tiers
                .iter()
                .map(|tier| tier.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let service_tiers_suffix = if service_tiers.is_empty() {
                String::new()
            } else {
                format!(" tiers: {service_tiers};")
            };
            let model_slug = &model.model;
            let runtime = model.provider_id.as_deref().map_or_else(
                || format!("model `{model_slug}` (provider unspecified; not a cross-provider route)"),
                |provider| format!("`{provider}` / `{model_slug}`"),
            );
            let economics_suffix = model
                .orchestration
                .as_ref()
                .and_then(|metadata| metadata.billing().map(|billing| (metadata, billing)))
                .map_or_else(String::new, |(metadata, billing)| {
                    let billing = match billing {
                        ModelBilling::Plan {
                            relative_burn_millis,
                        } => {
                            format!("plan, burn {}x", format_millis(*relative_burn_millis))
                        }
                        ModelBilling::PlanSchedule {
                            off_peak_relative_burn_millis,
                            peak_relative_burn_millis,
                            peak_start_utc_hour,
                            peak_end_utc_hour,
                            promotional_off_peak_relative_burn_millis,
                            promotion_valid_through_utc,
                        } => {
                            let promotion = promotional_off_peak_relative_burn_millis
                                .zip(promotion_valid_through_utc.as_deref())
                                .map_or_else(String::new, |(burn, valid_through)| {
                                    format!(
                                        ", promotional off-peak {}x through {valid_through}",
                                        format_millis(burn)
                                    )
                                });
                            format!(
                                "plan, normal off-peak {}x / peak {}x at {:02}:00-{:02}:00 UTC{}",
                                format_millis(*off_peak_relative_burn_millis),
                                format_millis(*peak_relative_burn_millis),
                                peak_start_utc_hour,
                                peak_end_utc_hour,
                                promotion
                            )
                        }
                        ModelBilling::Metered {
                            input_milli_usd_per_million_tokens,
                            output_milli_usd_per_million_tokens,
                            ..
                        } => format!(
                            "metered ${}/${} per M tok",
                            format_millis(*input_milli_usd_per_million_tokens),
                            format_millis(*output_milli_usd_per_million_tokens)
                        ),
                        ModelBilling::AuthDependent {
                            plan_relative_burn_millis,
                            api_key_input_milli_usd_per_million_tokens,
                            api_key_output_milli_usd_per_million_tokens,
                            ..
                        } => format!(
                            "auth-dependent: subscription burn {}x or API ${}/${} per M tok",
                            format_millis(*plan_relative_burn_millis),
                            format_millis(*api_key_input_milli_usd_per_million_tokens),
                            format_millis(*api_key_output_milli_usd_per_million_tokens)
                        ),
                        ModelBilling::Local => "local".to_string(),
                    };
                    format!(" {billing}, {};", metadata.capability())
                });
            let frontier_effort_suffix = model
                .orchestration
                .as_ref()
                .filter(|metadata| metadata.capability() == ModelCapabilityTier::Frontier)
                .and_then(|_| {
                    let frontier_efforts = model
                        .supported_reasoning_efforts
                        .iter()
                        .filter_map(|preset| {
                            let effort = preset.effort.as_str();
                            matches!(effort, "max" | "ultra").then_some(effort)
                        })
                        .collect::<Vec<_>>();
                    (!frontier_efforts.is_empty()).then(|| {
                        format!(
                            " frontier efforts: {}{};",
                            frontier_efforts.join(", "),
                            if frontier_efforts
                                .contains(&"ultra") { " (ultra includes automatic delegation)" } else { Default::default() }
                        )
                    })
                })
                .unwrap_or_default();
            let vision_suffix = if model
                .input_modalities
                .iter()
                .any(|modality| matches!(modality, InputModality::Image))
            {
                " vision;"
            } else {
                " text-only;"
            };
            let reasoning_efforts_suffix = reasoning_efforts_suffix
                .strip_prefix(" Reasoning efforts: ")
                .and_then(|suffix| suffix.strip_suffix('.'))
                .map_or_else(String::new, |efforts| format!(" efforts: {efforts};"));
            format!(
                "- {runtime};{economics_suffix}{frontier_effort_suffix}{vision_suffix}{reasoning_efforts_suffix}{service_tiers_suffix}"
            )
                .trim_end_matches(';')
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{inherited_runtime}\n\
Available authorized exact runtime overrides (optional; omit both fields to inherit the current runtime). Pass the provider as `model_provider` and the model as `model`.\n\
Default allocation policy: compare the task with this catalogue before every spawn. Prefer an authorized `plan` runtime over a `metered` runtime when both can do the work, then choose the lowest-burn capable plan runtime. Use `fast` for mechanical or tightly specified work, `balanced` for ordinary engineering, and `frontier` only for genuinely hard reasoning, planning, or review. For frontier models with `max` or `ultra`, reserve those efforts for frontier work; `ultra` is the orchestration setting when automatic delegation is actually needed. Vision work requires a `vision` runtime; never send images to `text-only`. Plan capacity is finite, not free. If the user names a provider or model, treat it as an exact constraint: if it is unavailable or unauthorized, report that failure and do not substitute another runtime without the user's explicit consent.\n{model_descriptions}"
    )
}

fn format_millis(value: u32) -> String {
    let whole = value / 1000;
    let remainder = value % 1000;
    if remainder == 0 {
        return whole.to_string();
    }
    format!("{whole}.{remainder:03}")
        .trim_end_matches('0')
        .to_string()
}

fn wait_agent_tool_parameters_v1(options: WaitAgentTimeoutOptions) -> JsonSchema {
    let properties = BTreeMap::from([
        (
            "targets".to_string(),
            JsonSchema::array(
                JsonSchema::string(/*description*/ None),
                Some(
                    "Agent ids to wait on. Pass multiple ids to wait for whichever finishes first."
                        .to_string(),
                ),
            ),
        ),
        (
            "timeout_ms".to_string(),
            JsonSchema::number(Some(format!(
                "Timeout in milliseconds. Defaults to {}, min {}, max {}. Prefer longer waits (minutes) to avoid busy polling.",
                options.default_timeout_ms, options.min_timeout_ms, options.max_timeout_ms,
            ))),
        ),
    ]);

    JsonSchema::object(
        properties,
        Some(vec!["targets".to_string()]),
        Some(false.into()),
    )
}

fn wait_agent_tool_parameters_v2(options: WaitAgentTimeoutOptions) -> JsonSchema {
    let properties = BTreeMap::from([(
        "timeout_ms".to_string(),
        JsonSchema::number(Some(format!(
            "Timeout in milliseconds. Defaults to {}, min {}, max {}.",
            options.default_timeout_ms, options.min_timeout_ms, options.max_timeout_ms,
        ))),
    )]);

    JsonSchema::object(properties, /*required*/ None, Some(false.into()))
}

#[cfg(test)]
#[path = "multi_agents_spec_tests.rs"]
mod tests;
