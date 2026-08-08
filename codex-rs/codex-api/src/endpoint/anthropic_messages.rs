use crate::auth::SharedAuthProvider;
use crate::common::AnthropicMessagesRequest;
use crate::common::CompletionFinishReason;
use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::headers::build_session_headers;
use crate::requests::headers::insert_header;
use crate::requests::headers::subagent_header;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::EncodedJsonBody;
use codex_client::HttpTransport;
use codex_client::RequestTelemetry;
use codex_client::StreamResponse;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::instrument;
use tracing::trace;

const REQUEST_ID_HEADER: &str = "x-request-id";

pub struct AnthropicMessagesClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

#[derive(Default)]
pub struct AnthropicMessagesOptions {
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub session_source: Option<SessionSource>,
    pub extra_headers: HeaderMap,
}

impl<T: HttpTransport> AnthropicMessagesClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    #[instrument(
        name = "anthropic_messages.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "anthropic_messages_http",
            http.method = "POST",
            api.path = "messages"
        )
    )]
    pub async fn stream_request(
        &self,
        request: AnthropicMessagesRequest,
        options: AnthropicMessagesOptions,
    ) -> Result<ResponseStream, ApiError> {
        let AnthropicMessagesOptions {
            session_id,
            thread_id,
            session_source,
            extra_headers,
        } = options;

        let body = EncodedJsonBody::encode(&request).map_err(|e| {
            ApiError::Stream(format!("failed to encode Anthropic messages request: {e}"))
        })?;

        let mut headers = extra_headers;
        if let Some(ref thread_id) = thread_id {
            insert_header(&mut headers, "x-client-request-id", thread_id);
        }
        headers.extend(build_session_headers(session_id, thread_id));
        if let Some(subagent) = subagent_header(&session_source) {
            insert_header(&mut headers, "x-openai-subagent", &subagent);
        }
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let stream_response = self
            .session
            .stream_encoded_json_with(Method::POST, Self::path(), headers, Some(body), |req| {
                req.headers.insert(
                    http::header::ACCEPT,
                    HeaderValue::from_static("text/event-stream"),
                );
            })
            .await?;

        Ok(spawn_anthropic_messages_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
        ))
    }

    fn path() -> &'static str {
        "messages"
    }
}

fn spawn_anthropic_messages_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) -> ResponseStream {
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let response_id_hint = upstream_request_id.clone();
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        let _ = tx_event.send(Ok(ResponseEvent::Created)).await;
        process_anthropic_sse(
            stream_response.bytes,
            tx_event,
            idle_timeout,
            telemetry,
            response_id_hint,
        )
        .await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamEvent {
    r#type: String,
    message: Option<AnthropicMessageStart>,
    index: Option<usize>,
    content_block: Option<Value>,
    delta: Option<AnthropicDelta>,
    usage: Option<AnthropicUsage>,
    error: Option<AnthropicError>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    id: Option<String>,
    model: Option<String>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    r#type: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    r#type: Option<String>,
    message: Option<String>,
}

impl From<AnthropicUsage> for TokenUsage {
    fn from(usage: AnthropicUsage) -> Self {
        let non_cached = usage.input_tokens.unwrap_or(0);
        let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
        let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
        let input_tokens = non_cached + cache_creation + cache_read;
        let output_tokens = usage.output_tokens.unwrap_or(0);
        Self {
            input_tokens,
            cached_input_tokens: cache_read,
            cache_write_input_tokens: cache_creation,
            output_tokens,
            reasoning_output_tokens: 0,
            total_tokens: input_tokens + output_tokens,
            codex_rollout_budget_units: None,
        }
    }
}

#[derive(Default)]
struct AnthropicToolCallState {
    id: Option<String>,
    name: String,
    arguments: String,
    emitted: bool,
}

#[derive(Default)]
struct AnthropicServerToolUseState {
    id: Option<String>,
    name: String,
    arguments: String,
    emitted: bool,
}

#[derive(Default)]
struct AnthropicWebSearchResultState {
    tool_use_id: Option<String>,
    content: Option<Value>,
    emitted: bool,
}

struct AnthropicStreamState {
    response_id: Option<String>,
    response_id_hint: Option<String>,
    last_server_model: Option<String>,
    message_text: String,
    message_added: bool,
    reasoning_text: String,
    reasoning_block: Option<Value>,
    reasoning_added: bool,
    reasoning_header_emitted: bool,
    reasoning_sequence: usize,
    block_types: BTreeMap<usize, String>,
    tool_calls: BTreeMap<usize, AnthropicToolCallState>,
    server_tool_uses: BTreeMap<usize, AnthropicServerToolUseState>,
    web_search_results: BTreeMap<usize, AnthropicWebSearchResultState>,
    token_usage: Option<TokenUsage>,
    end_turn: Option<bool>,
    finish_reason: Option<CompletionFinishReason>,
}

impl AnthropicStreamState {
    fn new(response_id_hint: Option<String>) -> Self {
        Self {
            response_id: None,
            response_id_hint,
            last_server_model: None,
            message_text: String::new(),
            message_added: false,
            reasoning_text: String::new(),
            reasoning_block: None,
            reasoning_added: false,
            reasoning_header_emitted: false,
            reasoning_sequence: 0,
            block_types: BTreeMap::new(),
            tool_calls: BTreeMap::new(),
            server_tool_uses: BTreeMap::new(),
            web_search_results: BTreeMap::new(),
            token_usage: None,
            end_turn: None,
            finish_reason: None,
        }
    }

    fn response_id(&self) -> String {
        self.response_id
            .clone()
            .or_else(|| self.response_id_hint.clone())
            .unwrap_or_else(|| "anthropic_messages_response".to_string())
    }

    fn message_id(&self) -> String {
        format!("msg_{}", self.response_id())
    }

    fn reasoning_id(&self) -> String {
        let response_id = self.response_id();
        if self.reasoning_sequence == 0 {
            format!("rs_{response_id}")
        } else {
            format!("rs_{response_id}_{}", self.reasoning_sequence)
        }
    }

    async fn process_event(
        &mut self,
        event: AnthropicStreamEvent,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        match event.r#type.as_str() {
            "message_start" => self.on_message_start(event, tx_event).await,
            "content_block_start" => self.on_content_block_start(event, tx_event).await,
            "content_block_delta" => self.on_content_block_delta(event, tx_event).await,
            "content_block_stop" => self.on_content_block_stop(event, tx_event).await,
            "message_delta" => {
                self.on_message_delta(event);
                true
            }
            "message_stop" => {
                self.complete(tx_event).await;
                false
            }
            "error" => {
                let message = event.error.map_or_else(
                    || "Anthropic messages stream returned an error".to_string(),
                    |error| match (error.r#type, error.message) {
                        (Some(kind), Some(message)) => format!("{kind}: {message}"),
                        (Some(kind), None) => kind,
                        (None, Some(message)) => message,
                        (None, None) => "Anthropic messages stream returned an error".to_string(),
                    },
                );
                let _ = tx_event.send(Err(ApiError::Stream(message))).await;
                false
            }
            "ping" => true,
            _ => true,
        }
    }

    async fn on_message_start(
        &mut self,
        event: AnthropicStreamEvent,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        let Some(message) = event.message else {
            return true;
        };
        if self.response_id.is_none() {
            self.response_id = message.id;
        }
        if let Some(model) = message.model
            && self.last_server_model.as_deref() != Some(model.as_str())
        {
            if tx_event
                .send(Ok(ResponseEvent::ServerModel(model.clone())))
                .await
                .is_err()
            {
                return false;
            }
            self.last_server_model = Some(model);
        }
        if let Some(usage) = message.usage {
            self.token_usage = Some(usage.into());
        }
        true
    }

    #[allow(clippy::collapsible_match)]
    async fn on_content_block_start(
        &mut self,
        event: AnthropicStreamEvent,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        let Some(block) = event.content_block else {
            return true;
        };
        let block_type = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let index = event.index.unwrap_or(self.block_types.len());
        self.block_types.insert(index, block_type.clone());
        match block_type.as_str() {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    let text = text.to_string();
                    self.message_text.push_str(&text);
                    return self.emit_text_delta(text, tx_event).await;
                }
            }
            "thinking" => {
                if !self.start_reasoning_block(block, tx_event).await {
                    return false;
                }
                if let Some(thinking) = self
                    .reasoning_block
                    .as_ref()
                    .and_then(|block| block.get("thinking"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    && !thinking.is_empty()
                {
                    self.reasoning_text.push_str(&thinking);
                    return self.emit_reasoning_visible_delta(thinking, tx_event).await;
                }
            }
            "redacted_thinking" => {
                if !self.start_reasoning_block(block, tx_event).await {
                    return false;
                }
            }
            "tool_use" => {
                let mut state = AnthropicToolCallState {
                    id: block.get("id").and_then(Value::as_str).map(str::to_string),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: String::new(),
                    emitted: false,
                };
                if let Some(input) = block.get("input")
                    && !matches!(input, Value::Object(map) if map.is_empty())
                {
                    state.arguments = input.clone().to_string();
                }
                self.tool_calls.insert(index, state);
            }
            "server_tool_use" => {
                let mut state = AnthropicServerToolUseState {
                    id: block.get("id").and_then(Value::as_str).map(str::to_string),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: String::new(),
                    emitted: false,
                };
                if let Some(input) = block.get("input")
                    && !matches!(input, Value::Object(map) if map.is_empty())
                {
                    state.arguments = input.clone().to_string();
                }
                self.server_tool_uses.insert(index, state);
            }
            "web_search_tool_result" => {
                self.web_search_results.insert(
                    index,
                    AnthropicWebSearchResultState {
                        tool_use_id: block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        content: block.get("content").cloned(),
                        emitted: false,
                    },
                );
            }
            _ => {}
        }
        true
    }

    async fn on_content_block_delta(
        &mut self,
        event: AnthropicStreamEvent,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        let Some(delta) = event.delta else {
            return true;
        };
        match delta.r#type.as_deref() {
            Some("text_delta") => {
                if let Some(text) = delta.text
                    && !text.is_empty()
                {
                    self.message_text.push_str(&text);
                    return self.emit_text_delta(text, tx_event).await;
                }
            }
            Some("thinking_delta") => {
                if let Some(thinking) = delta.thinking
                    && !thinking.is_empty()
                {
                    self.append_reasoning_block_string_field("thinking", &thinking);
                    return self.emit_reasoning_delta(thinking, tx_event).await;
                }
            }
            Some("signature_delta") => {
                if let Some(signature) = delta.signature
                    && !signature.is_empty()
                {
                    self.set_reasoning_block_string_field("signature", &signature);
                }
            }
            Some("input_json_delta") => {
                if let Some(index) = event.index
                    && let Some(partial_json) = delta.partial_json
                {
                    if let Some(tool_call) = self.tool_calls.get_mut(&index) {
                        tool_call.arguments.push_str(&partial_json);
                    } else if let Some(server_tool_use) = self.server_tool_uses.get_mut(&index) {
                        server_tool_use.arguments.push_str(&partial_json);
                    }
                }
            }
            _ => {}
        }
        true
    }

    async fn on_content_block_stop(
        &mut self,
        event: AnthropicStreamEvent,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        let Some(index) = event.index else {
            return true;
        };
        if matches!(
            self.block_types.get(&index).map(String::as_str),
            Some("thinking" | "redacted_thinking")
        ) {
            return self.finish_reasoning_item(tx_event).await;
        }
        if !self.finish_reasoning_item(tx_event).await {
            return false;
        }
        if !self.emit_server_tool_use(index, tx_event).await {
            return false;
        }
        if !self.emit_web_search_result(index, tx_event).await {
            return false;
        }
        // Anthropic reports `stop_reason` in the later `message_delta` event.
        // Hold client tool calls until `message_stop` so a max-token cutoff
        // cannot execute a syntactically valid but semantically truncated call.
        true
    }

    fn on_message_delta(&mut self, event: AnthropicStreamEvent) {
        if let Some(usage) = event.usage {
            self.token_usage = Some(usage.into());
        }
        if let Some(delta) = event.delta
            && let Some(stop_reason) = delta.stop_reason.as_deref()
        {
            let (end_turn, finish_reason) = anthropic_completion_state(stop_reason);
            self.end_turn = end_turn;
            self.finish_reason = Some(finish_reason);
        }
    }

    /// Whether this stream carried anything the caller can act on.
    ///
    /// A tool-only turn is ordinary and produces no assistant text, so emptiness
    /// must be judged across every output channel rather than by message text alone.
    fn produced_model_output(&self) -> bool {
        self.message_added
            || self.reasoning_added
            || !self.tool_calls.is_empty()
            || !self.server_tool_uses.is_empty()
            || !self.web_search_results.is_empty()
    }

    async fn emit_text_delta(
        &mut self,
        text: String,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if !self.finish_reasoning_item(tx_event).await {
            return false;
        }
        if !self.message_added {
            let item = ResponseItem::Message {
                id: Some(ResponseItemId::from_server(self.message_id())),
                role: "assistant".to_string(),
                content: Vec::new(),
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            };
            if tx_event
                .send(Ok(ResponseEvent::OutputItemAdded(item)))
                .await
                .is_err()
            {
                return false;
            }
            self.message_added = true;
        }
        tx_event
            .send(Ok(ResponseEvent::OutputTextDelta(text)))
            .await
            .is_ok()
    }

    async fn emit_reasoning_delta(
        &mut self,
        thinking: String,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        self.reasoning_text.push_str(&thinking);
        self.emit_reasoning_visible_delta(thinking, tx_event).await
    }

    async fn start_reasoning_block(
        &mut self,
        block: Value,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if !self.finish_reasoning_item(tx_event).await {
            return false;
        }
        self.reasoning_text.clear();
        self.reasoning_block = Some(block);
        self.reasoning_added = false;
        self.reasoning_header_emitted = false;
        self.ensure_reasoning_item_added(tx_event).await
    }

    async fn ensure_reasoning_item_added(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if !self.reasoning_added {
            let item = ResponseItem::Reasoning {
                id: Some(ResponseItemId::from_server(self.reasoning_id())),
                summary: Vec::new(),
                content: None,
                encrypted_content: None,
                anthropic_content_block: None,
                internal_chat_message_metadata_passthrough: None,
            };
            if tx_event
                .send(Ok(ResponseEvent::OutputItemAdded(item)))
                .await
                .is_err()
            {
                return false;
            }
            self.reasoning_added = true;
        }
        true
    }

    async fn emit_reasoning_visible_delta(
        &mut self,
        thinking: String,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if !self.ensure_reasoning_item_added(tx_event).await {
            return false;
        }
        if !self.reasoning_header_emitted {
            if tx_event
                .send(Ok(ResponseEvent::ReasoningSummaryDelta {
                    delta: "**Reasoning**\n\n".to_string(),
                    summary_index: 0,
                }))
                .await
                .is_err()
            {
                return false;
            }
            self.reasoning_header_emitted = true;
        }
        tx_event
            .send(Ok(ResponseEvent::ReasoningSummaryDelta {
                delta: thinking,
                summary_index: 0,
            }))
            .await
            .is_ok()
    }

    async fn finish_reasoning_item(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if !self.reasoning_added {
            return true;
        }
        let content = (!self.reasoning_text.is_empty()).then(|| {
            vec![ReasoningItemContent::ReasoningText {
                text: self.reasoning_text.clone(),
            }]
        });
        let anthropic_content_block = self.reasoning_block.take();
        let item = ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server(self.reasoning_id())),
            summary: Vec::<ReasoningItemReasoningSummary>::new(),
            content,
            encrypted_content: None,
            anthropic_content_block,
            internal_chat_message_metadata_passthrough: None,
        };
        if tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_err()
        {
            return false;
        }
        self.reasoning_text.clear();
        self.reasoning_added = false;
        self.reasoning_header_emitted = false;
        self.reasoning_sequence += 1;
        true
    }

    fn append_reasoning_block_string_field(&mut self, key: &str, delta: &str) {
        let Some(Value::Object(block)) = self.reasoning_block.as_mut() else {
            return;
        };
        let entry = block
            .entry(key.to_string())
            .or_insert_with(|| Value::String(String::new()));
        match entry {
            Value::String(existing) => existing.push_str(delta),
            other => *other = Value::String(delta.to_string()),
        }
    }

    fn set_reasoning_block_string_field(&mut self, key: &str, value: &str) {
        let Some(Value::Object(block)) = self.reasoning_block.as_mut() else {
            return;
        };
        block.insert(key.to_string(), Value::String(value.to_string()));
    }

    async fn emit_tool_call(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        let response_id = self.response_id();
        let Some(tool_call) = self.tool_calls.get_mut(&index) else {
            return true;
        };
        if tool_call.emitted {
            return true;
        }
        if tool_call.name.is_empty() {
            let call_id = tool_call.id.as_deref().unwrap_or("<missing>");
            let _ = tx_event
                .send(Err(ApiError::Stream(format!(
                    "Anthropic messages stream emitted a tool call without a name \
                     at index {index}; call_id={call_id}"
                ))))
                .await;
            return false;
        }
        let call_id = tool_call
            .id
            .clone()
            .unwrap_or_else(|| format!("anthropic_call_{index}"));
        let item = ResponseItem::FunctionCall {
            id: Some(ResponseItemId::from_server(format!(
                "fc_{response_id}_{index}"
            ))),
            name: tool_call.name.clone(),
            namespace: None,
            arguments: if tool_call.arguments.trim().is_empty() {
                "{}".to_string()
            } else {
                tool_call.arguments.clone()
            },
            encrypted_function_args: None,
            call_id,
            internal_chat_message_metadata_passthrough: None,
        };
        tool_call.emitted = true;
        tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_ok()
    }

    async fn emit_server_tool_use(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        let Some(server_tool_use) = self.server_tool_uses.get_mut(&index) else {
            return true;
        };
        if server_tool_use.emitted {
            return true;
        }
        if server_tool_use.name != "web_search" {
            server_tool_use.emitted = true;
            return true;
        }

        let input = parse_anthropic_json_object(&server_tool_use.arguments);
        let block = anthropic_server_tool_use_block(
            server_tool_use.id.as_deref(),
            &server_tool_use.name,
            input.clone(),
        );
        let item = ResponseItem::WebSearchCall {
            id: server_tool_use.id.clone().map(ResponseItemId::from_server),
            status: Some("in_progress".to_string()),
            action: anthropic_web_search_action_from_input(&input),
            anthropic_content_block: Some(block),
            internal_chat_message_metadata_passthrough: None,
        };
        server_tool_use.emitted = true;
        tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_ok()
    }

    async fn emit_web_search_result(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        let response_id = self.response_id();
        let Some(result) = self.web_search_results.get_mut(&index) else {
            return true;
        };
        if result.emitted {
            return true;
        }

        let content = result.content.clone().unwrap_or_else(json_array);
        let status = if content.is_object() {
            "failed"
        } else {
            "completed"
        };
        let block = anthropic_web_search_tool_result_block(result.tool_use_id.as_deref(), content);
        let item = ResponseItem::WebSearchCall {
            id: Some(ResponseItemId::from_server(format!(
                "wsr_{response_id}_{index}"
            ))),
            status: Some(status.to_string()),
            action: None,
            anthropic_content_block: Some(block),
            internal_chat_message_metadata_passthrough: None,
        };
        result.emitted = true;
        tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_ok()
    }

    async fn complete(&mut self, tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>) {
        let response_id = self.response_id();
        // A stream that reaches `message_stop` having produced no assistant text, no
        // reasoning, and no tool call carries no model output. Reporting it as a
        // completed turn makes a degenerate response indistinguishable from real work:
        // the caller records a finished turn, a sub-agent reports success to its
        // parent, and the assignment is silently dropped. Surface it as a stream
        // failure so the existing retry path decides what to do.
        if !self.produced_model_output() {
            let _ = tx_event
                .send(Err(ApiError::Stream(
                    "Anthropic messages stream completed without any model output".to_string(),
                )))
                .await;
            return;
        }
        if matches!(self.finish_reason, Some(CompletionFinishReason::Length)) {
            let _ = tx_event
                .send(Ok(ResponseEvent::Completed {
                    response_id,
                    token_usage: self.token_usage.take(),
                    end_turn: self.end_turn,
                    finish_reason: self.finish_reason.take(),
                }))
                .await;
            return;
        }
        if !self.finish_reasoning_item(tx_event).await {
            return;
        }
        if self.message_added {
            let text = std::mem::take(&mut self.message_text);
            let item = ResponseItem::Message {
                id: Some(ResponseItemId::from_server(self.message_id())),
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText { text }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            };
            if tx_event
                .send(Ok(ResponseEvent::OutputItemDone(item)))
                .await
                .is_err()
            {
                return;
            }
        }
        let pending = self.tool_calls.keys().copied().collect::<Vec<_>>();
        for index in pending {
            if !self.emit_tool_call(index, tx_event).await {
                return;
            }
        }
        let pending = self.server_tool_uses.keys().copied().collect::<Vec<_>>();
        for index in pending {
            if !self.emit_server_tool_use(index, tx_event).await {
                return;
            }
        }
        let pending = self.web_search_results.keys().copied().collect::<Vec<_>>();
        for index in pending {
            if !self.emit_web_search_result(index, tx_event).await {
                return;
            }
        }
        let _ = tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id,
                token_usage: self.token_usage.take(),
                end_turn: self.end_turn,
                finish_reason: self.finish_reason.take(),
            }))
            .await;
    }
}

fn anthropic_completion_state(stop_reason: &str) -> (Option<bool>, CompletionFinishReason) {
    match stop_reason {
        "end_turn" | "stop_sequence" => (Some(true), CompletionFinishReason::Stop),
        "tool_use" | "pause_turn" => (Some(false), CompletionFinishReason::ToolCalls),
        "max_tokens" | "model_context_window_exceeded" => {
            (Some(false), CompletionFinishReason::Length)
        }
        "refusal" => (Some(true), CompletionFinishReason::ContentFilter),
        other => (None, CompletionFinishReason::Unknown(other.to_string())),
    }
}

fn json_array() -> Value {
    Value::Array(Vec::new())
}

fn parse_anthropic_json_object(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(arguments).unwrap_or_else(|_| Value::Object(Default::default()))
}

fn anthropic_server_tool_use_block(id: Option<&str>, name: &str, input: Value) -> Value {
    let mut block = serde_json::Map::new();
    block.insert(
        "type".to_string(),
        Value::String("server_tool_use".to_string()),
    );
    if let Some(id) = id {
        block.insert("id".to_string(), Value::String(id.to_string()));
    }
    block.insert("name".to_string(), Value::String(name.to_string()));
    block.insert("input".to_string(), input);
    Value::Object(block)
}

fn anthropic_web_search_tool_result_block(tool_use_id: Option<&str>, content: Value) -> Value {
    let mut block = serde_json::Map::new();
    block.insert(
        "type".to_string(),
        Value::String("web_search_tool_result".to_string()),
    );
    if let Some(tool_use_id) = tool_use_id {
        block.insert(
            "tool_use_id".to_string(),
            Value::String(tool_use_id.to_string()),
        );
    }
    block.insert("content".to_string(), content);
    Value::Object(block)
}

fn anthropic_web_search_action_from_input(input: &Value) -> Option<WebSearchAction> {
    let query = input
        .get("query")
        .or_else(|| input.get("search_query"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let queries = input
        .get("queries")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|queries| !queries.is_empty());
    if query.is_none() && queries.is_none() {
        None
    } else {
        Some(WebSearchAction::Search { query, queries })
    }
}

async fn process_anthropic_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    response_id_hint: Option<String>,
) {
    let mut stream = stream.eventsource();
    let mut state = AnthropicStreamState::new(response_id_hint);

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("Anthropic messages SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before Anthropic messages finished".into(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("Anthropic messages SSE event: {}", &sse.data);

        let event = match serde_json::from_str::<AnthropicStreamEvent>(&sse.data) {
            Ok(event) => event,
            Err(err) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(format!(
                        "failed to parse Anthropic messages SSE event: {err}; data: {}",
                        sse.data
                    ))))
                    .await;
                return;
            }
        };

        if !state.process_event(event, &tx_event).await {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use codex_client::TransportError;
    use futures::TryStreamExt;
    use pretty_assertions::assert_eq;
    use tokio_test::io::Builder as IoBuilder;
    use tokio_util::io::ReaderStream;

    async fn collect_events(chunks: &[&[u8]]) -> Vec<Result<ResponseEvent, ApiError>> {
        let mut builder = IoBuilder::new();
        for chunk in chunks {
            builder.read(chunk);
        }

        let reader = builder.build();
        let body =
            ReaderStream::new(reader).map_err(|err| TransportError::Network(err.to_string()));
        let (tx_event, mut rx_event) = mpsc::channel(1600);
        process_anthropic_sse(
            Box::pin(body),
            tx_event,
            Duration::from_secs(5),
            /*telemetry*/ None,
            Some("req_123".to_string()),
        )
        .await;

        let mut events = Vec::new();
        while let Some(event) = rx_event.recv().await {
            events.push(event);
        }
        events
    }

    #[test]
    fn anthropic_stop_reasons_preserve_completion_semantics() {
        assert_eq!(
            anthropic_completion_state("end_turn"),
            (Some(true), CompletionFinishReason::Stop)
        );
        assert_eq!(
            anthropic_completion_state("stop_sequence"),
            (Some(true), CompletionFinishReason::Stop)
        );
        assert_eq!(
            anthropic_completion_state("tool_use"),
            (Some(false), CompletionFinishReason::ToolCalls)
        );
        assert_eq!(
            anthropic_completion_state("pause_turn"),
            (Some(false), CompletionFinishReason::ToolCalls)
        );
        assert_eq!(
            anthropic_completion_state("max_tokens"),
            (Some(false), CompletionFinishReason::Length)
        );
        assert_eq!(
            anthropic_completion_state("model_context_window_exceeded"),
            (Some(false), CompletionFinishReason::Length)
        );
        assert_eq!(
            anthropic_completion_state("refusal"),
            (Some(true), CompletionFinishReason::ContentFilter)
        );
        assert_eq!(
            anthropic_completion_state("future_reason"),
            (
                None,
                CompletionFinishReason::Unknown("future_reason".to_string())
            )
        );
    }

    #[tokio::test]
    async fn empty_stream_is_a_failure_not_a_completed_turn() {
        // message_start -> stop_reason -> message_stop with no content block at all.
        // The provider produced nothing; reporting Completed here would record a
        // finished turn and let a sub-agent report success having done no work.
        for stop_reason in ["end_turn", "tool_use", "pause_turn"] {
            let events = collect_events(&[
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-fable-5\",\"usage\":{\"input_tokens\":9,\"output_tokens\":0}}}\n\n".to_string()
                .as_bytes(),
                format!(
                    "event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{stop_reason}\"}},\"usage\":{{\"input_tokens\":9,\"output_tokens\":0}}}}\n\n"
                )
                .as_bytes(),
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ])
            .await;

            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. }))),
                "{stop_reason}: an empty stream must not report a completed turn: {events:?}"
            );
            assert!(
                events.iter().any(|event| event.is_err()),
                "{stop_reason}: an empty stream must surface a failure: {events:?}"
            );
        }
    }

    #[tokio::test]
    async fn tool_only_stream_still_completes() {
        // A turn that emits only a tool call carries no assistant text. That is
        // ordinary and must keep completing, so the emptiness guard cannot key on
        // message text alone.
        let events = collect_events(&[
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","model":"claude-fable-5","usage":{"input_tokens":1,"output_tokens":1}}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"shell","input":{}}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

"#,
            br#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":1,"output_tokens":3}}

"#,
            br#"event: message_stop
data: {"type":"message_stop"}

"#,
        ])
        .await;

        assert!(
            events
                .iter()
                .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. }))),
            "a tool-only turn must still complete: {events:?}"
        );
    }

    #[tokio::test]
    async fn max_tokens_does_not_emit_or_execute_partial_tool_call() {
        let events = collect_events(&[
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_cutoff","model":"claude-opus-5","usage":{"input_tokens":1,"output_tokens":1}}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_cutoff","name":"structured_write","input":{}}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"runtime/engine.js\""}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

"#,
            br#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"input_tokens":1,"output_tokens":32000}}

"#,
            br#"event: message_stop
data: {"type":"message_stop"}

"#,
        ])
        .await;

        assert!(
            !events.iter().any(|event| matches!(
                event,
                Ok(ResponseEvent::OutputItemDone(
                    ResponseItem::FunctionCall { .. } | ResponseItem::CustomToolCall { .. }
                ))
            )),
            "a length-truncated tool call must never be dispatched: {events:?}"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(ResponseEvent::Completed {
                end_turn: Some(false),
                finish_reason: Some(CompletionFinishReason::Length),
                ..
            })
        )));
    }

    #[tokio::test]
    async fn parses_text_usage_and_cache_tokens() {
        let events = collect_events(&[
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","model":"glm-5.2","usage":{"input_tokens":9,"cache_creation_input_tokens":5,"cache_read_input_tokens":7,"output_tokens":1}}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"OK"}}

"#,
            br#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":9,"cache_creation_input_tokens":0,"cache_read_input_tokens":12,"output_tokens":2}}

"#,
            br#"event: message_stop
data: {"type":"message_stop"}

"#,
        ])
        .await;

        assert_matches!(&events[0], Ok(ResponseEvent::ServerModel(model)) if model == "glm-5.2");
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
        );
        assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "OK");
        assert_matches!(
            &events[3],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
                if content == &vec![ContentItem::OutputText { text: "OK".to_string() }]
        );
        assert_matches!(
            &events[4],
            Ok(ResponseEvent::Completed {
                response_id,
                token_usage: Some(TokenUsage {
                    input_tokens: 21,
                    cached_input_tokens: 12,
                    output_tokens: 2,
                    reasoning_output_tokens: 0,
                    total_tokens: 23,
                    ..
                }),
                end_turn: Some(true),
                ..
            }) if response_id == "msg_1"
        );
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn parses_thinking_as_visible_reasoning_summary_and_preserves_signed_block() {
        let events = collect_events(&[
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_think","model":"claude-fable-5","usage":{"input_tokens":1,"output_tokens":0}}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"first thought"}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-123"}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"visible"}}

"#,
            br#"event: message_stop
data: {"type":"message_stop"}

"#,
        ])
        .await;

        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemAdded(
                ResponseItem::Reasoning { .. }
            ))
        );
        assert_matches!(
            &events[2],
            Ok(ResponseEvent::ReasoningSummaryDelta { delta, summary_index })
                if delta == "**Reasoning**\n\n" && *summary_index == 0
        );
        assert_matches!(
            &events[3],
            Ok(ResponseEvent::ReasoningSummaryDelta { delta, summary_index })
                if delta == "first thought" && *summary_index == 0
        );
        assert_matches!(
            &events[4],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                content: Some(content),
                anthropic_content_block: Some(block),
                ..
            })) if content == &vec![ReasoningItemContent::ReasoningText {
                text: "first thought".to_string(),
            }]
                && block.pointer("/type").and_then(Value::as_str) == Some("thinking")
                && block.pointer("/thinking").and_then(Value::as_str) == Some("first thought")
                && block.pointer("/signature").and_then(Value::as_str) == Some("sig-123")
        );
        assert_matches!(
            &events[5],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
        );
        assert_matches!(&events[6], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "visible");
    }

    #[tokio::test]
    async fn preserves_thinking_before_tool_use_in_stream_order() {
        let events = collect_events(&[
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_think_tool","model":"claude-fable-5","usage":{"input_tokens":1,"output_tokens":0}}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"need a file"}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-tool"}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_read","name":"exec_command","input":{}}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"pwd\"}"}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":1}

"#,
            br#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":1,"output_tokens":3}}

"#,
            br#"event: message_stop
data: {"type":"message_stop"}

"#,
        ])
        .await;

        let reasoning_done = events.iter().position(|event| {
            matches!(
                event,
                Ok(ResponseEvent::OutputItemDone(
                    ResponseItem::Reasoning { .. }
                ))
            )
        });
        let tool_done = events.iter().position(|event| {
            matches!(
                event,
                Ok(ResponseEvent::OutputItemDone(
                    ResponseItem::FunctionCall { .. }
                ))
            )
        });
        assert!(reasoning_done.is_some_and(|index| index < tool_done.unwrap_or(usize::MAX)));
        assert_matches!(
            &events[4],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                anthropic_content_block: Some(block),
                ..
            })) if block.pointer("/type").and_then(Value::as_str) == Some("thinking")
                && block.pointer("/thinking").and_then(Value::as_str) == Some("need a file")
                && block.pointer("/signature").and_then(Value::as_str) == Some("sig-tool")
        );
        assert_matches!(
            &events[5],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                call_id,
                arguments,
                ..
            })) if name == "exec_command"
                && call_id == "toolu_read"
                && arguments == "{\"cmd\":\"pwd\"}"
        );
    }

    #[tokio::test]
    async fn preserves_redacted_thinking_before_tool_use() {
        let events = collect_events(&[
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_redacted","model":"claude-fable-5","usage":{"input_tokens":1,"output_tokens":0}}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"opaque-redacted"}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_read","name":"exec_command","input":{"cmd":"pwd"}}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":1}

"#,
            br#"event: message_stop
data: {"type":"message_stop"}

"#,
        ])
        .await;

        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemAdded(
                ResponseItem::Reasoning { .. }
            ))
        );
        assert_matches!(
            &events[2],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                content: None,
                anthropic_content_block: Some(block),
                ..
            })) if block.pointer("/type").and_then(Value::as_str) == Some("redacted_thinking")
                && block.pointer("/data").and_then(Value::as_str) == Some("opaque-redacted")
        );
        assert_matches!(
            &events[3],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, .. }))
                if call_id == "toolu_read"
        );
    }

    #[tokio::test]
    async fn parses_streamed_tool_use() {
        let events = collect_events(&[
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_tool","model":"glm-5.2","usage":{"input_tokens":1,"output_tokens":0}}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"exec_command","input":{}}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"date\"}"}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

"#,
            br#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":1,"output_tokens":3}}

"#,
            br#"event: message_stop
data: {"type":"message_stop"}

"#,
        ])
        .await;

        assert_matches!(
            &events[0],
            Ok(ResponseEvent::ServerModel(model)) if model == "glm-5.2"
        );
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            })) if name == "exec_command" && arguments == "{\"cmd\":\"date\"}" && call_id == "toolu_1"
        );
        assert_matches!(
            &events[2],
            Ok(ResponseEvent::Completed {
                response_id,
                end_turn: Some(false),
                ..
            }) if response_id == "msg_tool"
        );
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn parses_server_side_web_search_blocks_and_pause_turn() {
        let events = collect_events(&[
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_web","model":"claude-fable","usage":{"input_tokens":2,"output_tokens":0}}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Found current data."}}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":"}}

"#,
            br#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"10 year treasury yield\"}"}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":1}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","url":"https://example.com/yield","title":"Yield","encrypted_content":"abc"}]}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":2}

"#,
            br#"event: content_block_start
data: {"type":"content_block_start","index":3,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_2","content":{"type":"web_search_tool_result_error","error_code":"max_uses_exceeded"}}}

"#,
            br#"event: content_block_stop
data: {"type":"content_block_stop","index":3}

"#,
            br#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"pause_turn"},"usage":{"input_tokens":2,"output_tokens":4}}

"#,
            br#"event: message_stop
data: {"type":"message_stop"}

"#,
        ])
        .await;

        assert_matches!(&events[0], Ok(ResponseEvent::ServerModel(model)) if model == "claude-fable");
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
        );
        assert_matches!(
            &events[3],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall {
                id,
                status,
                action: Some(WebSearchAction::Search { query: Some(query), .. }),
                anthropic_content_block: Some(block),
                ..
            })) if id.as_deref() == Some("srvtoolu_1")
                && status.as_deref() == Some("in_progress")
                && query == "10 year treasury yield"
                && block.pointer("/type").and_then(Value::as_str) == Some("server_tool_use")
                && block.pointer("/input/query").and_then(Value::as_str) == Some("10 year treasury yield")
        );
        assert_matches!(
            &events[4],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall {
                status,
                anthropic_content_block: Some(block),
                ..
            })) if status.as_deref() == Some("completed")
                && block.pointer("/content/0/type").and_then(Value::as_str) == Some("web_search_result")
        );
        assert_matches!(
            &events[5],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall {
                status,
                anthropic_content_block: Some(block),
                ..
            })) if status.as_deref() == Some("failed")
                && block.pointer("/content/type").and_then(Value::as_str) == Some("web_search_tool_result_error")
        );
        assert_matches!(
            &events[7],
            Ok(ResponseEvent::Completed {
                response_id,
                end_turn: Some(false),
                ..
            }) if response_id == "msg_web"
        );
        assert_eq!(events.len(), 8);
    }
}
