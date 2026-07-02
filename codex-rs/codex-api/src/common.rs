use crate::error::ApiError;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::config_types::Verbosity as VerbosityConfig;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::ModelVerification;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TurnModerationMetadataEvent;
use codex_protocol::protocol::W3cTraceContext;
use futures::Stream;
use serde::Deserialize;
use serde::Serialize;
use serde::ser::SerializeStruct;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio::sync::mpsc;

pub const WS_REQUEST_HEADER_TRACEPARENT_CLIENT_METADATA_KEY: &str = "ws_request_header_traceparent";
pub const WS_REQUEST_HEADER_TRACESTATE_CLIENT_METADATA_KEY: &str = "ws_request_header_tracestate";

/// Canonical input payload for the compaction endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct CompactionInput<'a> {
    pub model: &'a str,
    pub input: &'a [ResponseItem],
    #[serde(skip_serializing_if = "str::is_empty")]
    pub instructions: &'a str,
    pub tools: Vec<Value>,
    pub parallel_tool_calls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextControls>,
}

/// Canonical input payload for the memory summarize endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct MemorySummarizeInput {
    pub model: String,
    #[serde(rename = "traces")]
    pub raw_memories: Vec<RawMemory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawMemory {
    pub id: String,
    pub metadata: RawMemoryMetadata,
    pub items: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawMemoryMetadata {
    pub source_path: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MemorySummarizeOutput {
    #[serde(rename = "trace_summary", alias = "raw_memory")]
    pub raw_memory: String,
    pub memory_summary: String,
}

#[derive(Debug)]
pub enum ResponseEvent {
    Created,
    OutputItemDone(ResponseItem),
    OutputItemAdded(ResponseItem),
    /// Emitted when the server includes `OpenAI-Model` on the stream response.
    /// This can differ from the requested model when backend safety routing applies.
    ServerModel(String),
    /// Emitted when the server recommends additional account verification.
    ModelVerifications(Vec<ModelVerification>),
    /// Emitted when the server includes moderation metadata for first-party turn presentation.
    TurnModerationMetadata(TurnModerationMetadataEvent),
    /// Emitted when `X-Reasoning-Included: true` is present on the response,
    /// meaning the server already accounted for past reasoning tokens and the
    /// client should not re-estimate them.
    ServerReasoningIncluded(bool),
    Completed {
        response_id: String,
        token_usage: Option<TokenUsage>,
        /// Did the model affirmatively end its turn? Some providers do not set this,
        /// so we rely on fallback logic when this is `None`.
        end_turn: Option<bool>,
    },
    OutputTextDelta(String),
    ToolCallInputDelta {
        item_id: String,
        call_id: Option<String>,
        delta: String,
    },
    ReasoningSummaryDelta {
        delta: String,
        summary_index: i64,
    },
    ReasoningContentDelta {
        delta: String,
        content_index: i64,
    },
    ReasoningSummaryPartAdded {
        summary_index: i64,
    },
    RateLimits(RateLimitSnapshot),
    ModelsEtag(String),
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningContext {
    Auto,
    CurrentTurn,
    AllTurns,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Reasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffortConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReasoningSummaryConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ReasoningContext>,
}

#[derive(Debug, Serialize, Default, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TextFormatType {
    #[default]
    JsonSchema,
}

#[derive(Debug, Serialize, Default, Clone, PartialEq)]
pub struct TextFormat {
    /// Format type used by the OpenAI text controls.
    pub r#type: TextFormatType,
    /// When true, the server is expected to strictly validate responses.
    pub strict: bool,
    /// JSON schema for the desired output.
    pub schema: Value,
    /// Friendly name for the format, used in telemetry/debugging.
    pub name: String,
}

/// Controls the `text` field for the Responses API, combining verbosity and
/// optional JSON schema output formatting.
#[derive(Debug, Serialize, Default, Clone, PartialEq)]
pub struct TextControls {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<OpenAiVerbosity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
}

#[derive(Debug, Serialize, Default, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiVerbosity {
    Low,
    #[default]
    Medium,
    High,
}

impl From<VerbosityConfig> for OpenAiVerbosity {
    fn from(v: VerbosityConfig) -> Self {
        match v {
            VerbosityConfig::Low => OpenAiVerbosity::Low,
            VerbosityConfig::Medium => OpenAiVerbosity::Medium,
            VerbosityConfig::High => OpenAiVerbosity::High,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponsesApiRequest {
    pub model: String,
    pub instructions: String,
    pub previous_response_id: Option<String>,
    pub input: Vec<ResponseItem>,
    pub tools: Vec<serde_json::Value>,
    pub tool_choice: String,
    pub parallel_tool_calls: bool,
    pub reasoning: Option<Reasoning>,
    pub store: bool,
    pub stream: bool,
    pub include: Vec<String>,
    pub service_tier: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub text: Option<TextControls>,
    pub client_metadata: Option<HashMap<String, String>>,
    pub thinking_budget: Option<i64>,
    pub emit_usage: Option<bool>,
    pub enable_thinking: Option<bool>,
    pub reasoning_effort: Option<String>,
}

impl ResponsesApiRequest {
    fn uses_ambient_input_format(&self) -> bool {
        self.thinking_budget.is_some()
            || self.emit_usage == Some(true)
            || self.enable_thinking.is_some()
            || self.reasoning_effort.is_some()
    }
}

impl Serialize for ResponsesApiRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut field_count = 8;
        field_count += usize::from(!self.instructions.is_empty());
        field_count += usize::from(self.previous_response_id.is_some());
        field_count += usize::from(self.reasoning.is_some());
        field_count += usize::from(self.service_tier.is_some());
        field_count += usize::from(self.prompt_cache_key.is_some());
        field_count += usize::from(self.text.is_some());
        field_count += usize::from(self.client_metadata.is_some());
        field_count += usize::from(self.thinking_budget.is_some());
        field_count += usize::from(self.emit_usage.is_some());
        field_count += usize::from(self.enable_thinking.is_some());
        field_count += usize::from(self.reasoning_effort.is_some());

        let mut state = serializer.serialize_struct("ResponsesApiRequest", field_count)?;
        state.serialize_field("model", &self.model)?;
        if !self.instructions.is_empty() {
            state.serialize_field("instructions", &self.instructions)?;
        }
        if let Some(previous_response_id) = &self.previous_response_id {
            state.serialize_field("previous_response_id", previous_response_id)?;
        }
        if self.uses_ambient_input_format() {
            state.serialize_field("input", &ambient_input_from_response_items(&self.input))?;
        } else {
            state.serialize_field("input", &self.input)?;
        }
        state.serialize_field("tools", &self.tools)?;
        state.serialize_field("tool_choice", &self.tool_choice)?;
        state.serialize_field("parallel_tool_calls", &self.parallel_tool_calls)?;
        if let Some(reasoning) = &self.reasoning {
            state.serialize_field("reasoning", reasoning)?;
        }
        state.serialize_field("store", &self.store)?;
        state.serialize_field("stream", &self.stream)?;
        state.serialize_field("include", &self.include)?;
        if let Some(service_tier) = &self.service_tier {
            state.serialize_field("service_tier", service_tier)?;
        }
        if let Some(prompt_cache_key) = &self.prompt_cache_key {
            state.serialize_field("prompt_cache_key", prompt_cache_key)?;
        }
        if let Some(text) = &self.text {
            state.serialize_field("text", text)?;
        }
        if let Some(client_metadata) = &self.client_metadata {
            state.serialize_field("client_metadata", client_metadata)?;
        }
        if let Some(thinking_budget) = self.thinking_budget {
            state.serialize_field("thinking_budget", &thinking_budget)?;
        }
        if let Some(emit_usage) = self.emit_usage {
            state.serialize_field("emit_usage", &emit_usage)?;
        }
        if let Some(enable_thinking) = self.enable_thinking {
            state.serialize_field("enable_thinking", &enable_thinking)?;
        }
        if let Some(reasoning_effort) = &self.reasoning_effort {
            state.serialize_field("reasoning_effort", reasoning_effort)?;
        }
        state.end()
    }
}

#[derive(Debug)]
struct AmbientReasoningContentChunk {
    role: String,
    content: String,
}

fn ambient_input_from_response_items(input: &[ResponseItem]) -> String {
    let chunks: Vec<_> = input
        .iter()
        .filter_map(ambient_chunk_from_response_item)
        .collect();

    if chunks.is_empty() {
        return String::new();
    }

    if chunks.len() == 1 && chunks[0].role == "user" {
        return chunks[0].content.clone();
    }

    chunks
        .into_iter()
        .map(|chunk| format!("{}:\n{}", chunk.role, chunk.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn ambient_chunk_from_response_item(item: &ResponseItem) -> Option<AmbientReasoningContentChunk> {
    match item {
        ResponseItem::Message { role, content, .. } => ambient_text_from_content_items(content)
            .map(|content| AmbientReasoningContentChunk {
                role: role.clone(),
                content,
            }),
        ResponseItem::AgentMessage {
            author,
            recipient,
            content,
            ..
        } => ambient_text_from_agent_message_content(content).map(|content| {
            AmbientReasoningContentChunk {
                role: "assistant".to_string(),
                content: format!("{author} to {recipient}:\n{content}"),
            }
        }),
        // Hidden reasoning is model-internal state. Replaying it into Ambient/Z.AI
        // string input makes the next turn treat private chain-of-thought as user
        // visible context and can amplify a single provider leak into a loop.
        ResponseItem::Reasoning { .. } => None,
        ResponseItem::FunctionCall { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. } => ambient_json_chunk("assistant", item),
        ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::CompactionTrigger { .. } => ambient_json_chunk("tool", item),
        ResponseItem::Other => None,
    }
}

fn ambient_json_chunk(role: &str, item: &ResponseItem) -> Option<AmbientReasoningContentChunk> {
    serde_json::to_string(item)
        .ok()
        .filter(|content| !content.trim().is_empty())
        .map(|content| AmbientReasoningContentChunk {
            role: role.to_string(),
            content,
        })
}

fn ambient_text_from_content_items(content: &[ContentItem]) -> Option<String> {
    let parts: Vec<_> = content
        .iter()
        .map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => text.clone(),
            ContentItem::InputImage { image_url, .. } => format!("[image: {image_url}]"),
        })
        .filter(|text| !text.trim().is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn ambient_text_from_agent_message_content(content: &[AgentMessageInputContent]) -> Option<String> {
    let parts: Vec<_> = content
        .iter()
        .filter_map(|item| match item {
            AgentMessageInputContent::InputText { text } => Some(text.clone()),
            AgentMessageInputContent::EncryptedContent { .. } => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChatCompletionsRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<ChatStreamOptions>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emit_usage: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Value>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChatStreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<Value>,
    pub messages: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    pub stream: bool,
    pub max_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<Value>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<ChatMessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCall>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum ChatMessageContent {
    Text(String),
    Parts(Vec<ChatContentPart>),
}

impl ChatMessageContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn cache_control_text(text: impl Into<String>) -> Self {
        Self::Parts(vec![ChatContentPart::cache_control_text(text)])
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChatContentPart {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<ChatCacheControl>,
}

impl ChatContentPart {
    pub fn cache_control_text(text: impl Into<String>) -> Self {
        Self {
            kind: "text".to_string(),
            text: text.into(),
            cache_control: Some(ChatCacheControl::ephemeral()),
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChatCacheControl {
    #[serde(rename = "type")]
    pub kind: String,
}

impl ChatCacheControl {
    pub fn ephemeral() -> Self {
        Self {
            kind: "ephemeral".to_string(),
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChatToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ChatToolFunction,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChatToolFunction {
    pub name: String,
    pub arguments: String,
}

impl From<&ResponsesApiRequest> for ResponseCreateWsRequest {
    fn from(request: &ResponsesApiRequest) -> Self {
        Self {
            model: request.model.clone(),
            instructions: request.instructions.clone(),
            previous_response_id: request.previous_response_id.clone(),
            input: request.input.clone(),
            tools: request.tools.clone(),
            tool_choice: request.tool_choice.clone(),
            parallel_tool_calls: request.parallel_tool_calls,
            reasoning: request.reasoning.clone(),
            store: request.store,
            stream: request.stream,
            include: request.include.clone(),
            service_tier: request.service_tier.clone(),
            prompt_cache_key: request.prompt_cache_key.clone(),
            text: request.text.clone(),
            generate: None,
            client_metadata: request.client_metadata.clone(),
            thinking_budget: request.thinking_budget,
            emit_usage: request.emit_usage,
            enable_thinking: request.enable_thinking,
            reasoning_effort: request.reasoning_effort.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ResponseCreateWsRequest {
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    pub input: Vec<ResponseItem>,
    pub tools: Vec<Value>,
    pub tool_choice: String,
    pub parallel_tool_calls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    pub store: bool,
    pub stream: bool,
    pub include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextControls>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_metadata: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emit_usage: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

pub fn response_create_client_metadata(
    client_metadata: Option<HashMap<String, String>>,
    trace: Option<&W3cTraceContext>,
) -> Option<HashMap<String, String>> {
    let mut client_metadata = client_metadata.unwrap_or_default();

    if let Some(traceparent) = trace.and_then(|trace| trace.traceparent.as_deref()) {
        client_metadata.insert(
            WS_REQUEST_HEADER_TRACEPARENT_CLIENT_METADATA_KEY.to_string(),
            traceparent.to_string(),
        );
    }
    if let Some(tracestate) = trace.and_then(|trace| trace.tracestate.as_deref()) {
        client_metadata.insert(
            WS_REQUEST_HEADER_TRACESTATE_CLIENT_METADATA_KEY.to_string(),
            tracestate.to_string(),
        );
    }

    (!client_metadata.is_empty()).then_some(client_metadata)
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum ResponsesWsRequest {
    #[serde(rename = "response.create")]
    ResponseCreate(ResponseCreateWsRequest),
}

pub fn create_text_param_for_request(
    verbosity: Option<VerbosityConfig>,
    output_schema: &Option<Value>,
    output_schema_strict: bool,
) -> Option<TextControls> {
    if verbosity.is_none() && output_schema.is_none() {
        return None;
    }

    Some(TextControls {
        verbosity: verbosity.map(std::convert::Into::into),
        format: output_schema.as_ref().map(|schema| TextFormat {
            r#type: TextFormatType::JsonSchema,
            strict: output_schema_strict,
            schema: schema.clone(),
            name: "codex_output_schema".to_string(),
        }),
    })
}

pub struct ResponseStream {
    pub rx_event: mpsc::Receiver<Result<ResponseEvent, ApiError>>,
    /// Server-assigned `x-request-id` response header, when present.
    pub upstream_request_id: Option<String>,
}

impl Stream for ResponseStream {
    type Item = Result<ResponseEvent, ApiError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx_event.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::ReasoningItemContent;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn chat_completions_request_without_provider_serializes_unchanged() {
        let request = ChatCompletionsRequest {
            model: "z-ai/glm-5.2".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some(ChatMessageContent::text("hello")),
                tool_call_id: None,
                tool_calls: Vec::new(),
            }],
            stream: true,
            stream_options: Some(ChatStreamOptions {
                include_usage: true,
            }),
            tools: vec![json!({
                "type": "function",
                "function": {
                    "name": "noop"
                }
            })],
            tool_choice: Some("auto".to_string()),
            parallel_tool_calls: Some(true),
            prompt_cache_key: Some("cache-key".to_string()),
            response_format: None,
            emit_usage: None,
            enable_thinking: None,
            reasoning_effort: None,
            reasoning: Some(json!({ "effort": "medium" })),
            provider: None,
        };

        let serialized = serde_json::to_string(&request).expect("serialize request");
        assert_eq!(
            serialized,
            r#"{"model":"z-ai/glm-5.2","messages":[{"role":"user","content":"hello"}],"stream":true,"stream_options":{"include_usage":true},"tools":[{"function":{"name":"noop"},"type":"function"}],"tool_choice":"auto","parallel_tool_calls":true,"prompt_cache_key":"cache-key","reasoning":{"effort":"medium"}}"#
        );
    }

    #[test]
    fn chat_completions_request_serializes_provider_object_when_set() {
        let request = ChatCompletionsRequest {
            model: "z-ai/glm-5.2".to_string(),
            messages: Vec::new(),
            stream: true,
            stream_options: None,
            tools: Vec::new(),
            tool_choice: None,
            parallel_tool_calls: None,
            prompt_cache_key: None,
            response_format: None,
            emit_usage: None,
            enable_thinking: None,
            reasoning_effort: None,
            reasoning: None,
            provider: Some(json!({
                "sort": "throughput",
                "require_parameters": true,
            })),
        };

        let body = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(body["provider"]["sort"], "throughput");
        assert_eq!(body["provider"]["require_parameters"], true);
    }

    #[test]
    fn ambient_input_does_not_replay_reasoning_items() {
        let input = vec![
            ResponseItem::Reasoning {
                id: Some("rs_1".to_string()),
                summary: Vec::new(),
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: "private chain of thought".to_string(),
                }]),
                encrypted_content: None,
                metadata: None,
            },
            ResponseItem::Message {
                id: Some("msg_1".to_string()),
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "inspect StakeHub".to_string(),
                }],
                phase: None,
                metadata: None,
            },
        ];

        assert_eq!(
            ambient_input_from_response_items(&input),
            "inspect StakeHub".to_string()
        );
    }
}
