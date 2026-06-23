use crate::auth::SharedAuthProvider;
use crate::common::ChatCompletionsRequest;
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
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
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

pub struct ChatCompletionsClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

#[derive(Default)]
pub struct ChatCompletionsOptions {
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub session_source: Option<SessionSource>,
    pub extra_headers: HeaderMap,
}

impl<T: HttpTransport> ChatCompletionsClient<T> {
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
        name = "chat_completions.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "chat_completions_http",
            http.method = "POST",
            api.path = "chat/completions"
        )
    )]
    pub async fn stream_request(
        &self,
        request: ChatCompletionsRequest,
        options: ChatCompletionsOptions,
    ) -> Result<ResponseStream, ApiError> {
        let ChatCompletionsOptions {
            session_id,
            thread_id,
            session_source,
            extra_headers,
        } = options;

        let body = EncodedJsonBody::encode(&request).map_err(|e| {
            ApiError::Stream(format!("failed to encode chat completions request: {e}"))
        })?;

        let mut headers = extra_headers;
        if let Some(ref thread_id) = thread_id {
            insert_header(&mut headers, "x-client-request-id", thread_id);
        }
        headers.extend(build_session_headers(session_id, thread_id));
        if let Some(subagent) = subagent_header(&session_source) {
            insert_header(&mut headers, "x-openai-subagent", &subagent);
        }

        let stream_response = self
            .session
            .stream_encoded_json_with(Method::POST, Self::path(), headers, Some(body), |req| {
                req.headers.insert(
                    http::header::ACCEPT,
                    HeaderValue::from_static("text/event-stream"),
                );
            })
            .await?;

        Ok(spawn_chat_completions_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
        ))
    }

    fn path() -> &'static str {
        "chat/completions"
    }
}

fn spawn_chat_completions_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) -> ResponseStream {
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    let response_id_hint = upstream_request_id.clone();
    tokio::spawn(async move {
        let _ = tx_event.send(Ok(ResponseEvent::Created)).await;
        process_chat_sse(
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
struct ChatCompletionChunk {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    delta: ChatDelta,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ChatToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<ChatFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: i64,
    #[serde(default)]
    completion_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
    #[serde(default)]
    prompt_tokens_details: Option<ChatPromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<ChatCompletionTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct ChatPromptTokensDetails {
    #[serde(default)]
    cached_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: i64,
}

impl From<ChatUsage> for TokenUsage {
    fn from(value: ChatUsage) -> Self {
        Self {
            input_tokens: value.prompt_tokens,
            cached_input_tokens: value
                .prompt_tokens_details
                .map(|details| details.cached_tokens)
                .unwrap_or(0),
            output_tokens: value.completion_tokens,
            reasoning_output_tokens: value
                .completion_tokens_details
                .map(|details| details.reasoning_tokens)
                .unwrap_or(0),
            total_tokens: value.total_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatErrorEnvelope {
    error: ChatError,
}

#[derive(Debug, Deserialize)]
struct ChatError {
    message: Option<String>,
    code: Option<String>,
}

#[derive(Debug, Default)]
struct PendingToolCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct TextFunctionCall {
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
    arguments: Option<Value>,
    call_id: Option<String>,
}

#[derive(Debug)]
struct ChatStreamState {
    response_id: Option<String>,
    last_server_model: Option<String>,
    message_added: bool,
    message_text: String,
    emitted_text_len: usize,
    reasoning_added: bool,
    reasoning_done: bool,
    reasoning_text: String,
    tool_calls: BTreeMap<usize, PendingToolCall>,
    token_usage: Option<TokenUsage>,
    response_id_hint: Option<String>,
}

impl ChatStreamState {
    fn new(response_id_hint: Option<String>) -> Self {
        Self {
            response_id: None,
            last_server_model: None,
            message_added: false,
            message_text: String::new(),
            emitted_text_len: 0,
            reasoning_added: false,
            reasoning_done: false,
            reasoning_text: String::new(),
            tool_calls: BTreeMap::new(),
            token_usage: None,
            response_id_hint,
        }
    }

    async fn process_chunk(
        &mut self,
        chunk: ChatCompletionChunk,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if self.response_id.is_none() {
            self.response_id = chunk.id.clone();
        }
        if let Some(model) = chunk.model
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
        if let Some(usage) = chunk.usage {
            self.token_usage = Some(usage.into());
        }

        for choice in chunk.choices {
            if let Some(delta) = choice.delta.reasoning_content
                && !delta.is_empty()
            {
                if self.reasoning_done || self.message_added {
                    trace!(
                        "dropping late chat completions reasoning_content after visible output started"
                    );
                } else {
                    if !self.ensure_reasoning_item_added(tx_event).await {
                        return false;
                    }
                    self.reasoning_text.push_str(&delta);
                    if tx_event
                        .send(Ok(ResponseEvent::ReasoningContentDelta {
                            delta,
                            content_index: 0,
                        }))
                        .await
                        .is_err()
                    {
                        return false;
                    }
                }
            }

            if let Some(delta) = choice.delta.content
                && !delta.is_empty()
            {
                if !self.finish_reasoning_item(tx_event).await {
                    return false;
                }
                self.message_text.push_str(&delta);
                if !self.should_delay_text_delta() && !self.emit_pending_text_delta(tx_event).await
                {
                    return false;
                }
            }

            for tool_delta in choice.delta.tool_calls {
                let tool_call = self.tool_calls.entry(tool_delta.index).or_default();
                if let Some(id) = tool_delta.id {
                    tool_call.id = Some(id);
                }
                if let Some(function) = tool_delta.function {
                    if let Some(name) = function.name {
                        tool_call.name.push_str(&name);
                    }
                    if let Some(arguments) = function.arguments {
                        tool_call.arguments.push_str(&arguments);
                    }
                }
            }
        }

        true
    }

    async fn complete(mut self, tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>) {
        let response_id = self.response_id();
        let message_id = format!("msg_{response_id}");
        let token_usage = self.token_usage.take();

        if !self.finish_reasoning_item(tx_event).await {
            return;
        }

        if !self.message_text.is_empty() {
            match parse_serialized_function_call_text(&self.message_text) {
                Ok(Some(item)) => {
                    if tx_event
                        .send(Ok(ResponseEvent::OutputItemDone(item)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(None) => {
                    if self.emitted_text_len < self.message_text.len()
                        && !self.emit_pending_text_delta(tx_event).await
                    {
                        return;
                    }
                    if self.message_added {
                        let item = ResponseItem::Message {
                            id: Some(message_id),
                            role: "assistant".to_string(),
                            content: vec![ContentItem::OutputText {
                                text: self.message_text,
                            }],
                            phase: None,
                            metadata: None,
                        };
                        if tx_event
                            .send(Ok(ResponseEvent::OutputItemDone(item)))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Err(message) => {
                    let _ = tx_event.send(Err(ApiError::Stream(message))).await;
                    return;
                }
            }
        }

        for (index, tool_call) in self.tool_calls {
            if tool_call.name.is_empty() {
                let call_id = tool_call.id.as_deref().unwrap_or("<missing>");
                let _ = tx_event
                    .send(Err(ApiError::Stream(format!(
                        "chat completions stream emitted a tool call without a function name \
                         at index {index}; call_id={call_id}; arguments excerpt: {}",
                        diagnostic_excerpt(&tool_call.arguments)
                    ))))
                    .await;
                return;
            }
            let call_id = tool_call
                .id
                .unwrap_or_else(|| format!("chatcmpl_call_{index}"));
            let item = ResponseItem::FunctionCall {
                id: Some(format!("fc_{call_id}")),
                name: tool_call.name,
                namespace: None,
                arguments: tool_call.arguments,
                call_id,
                metadata: None,
            };
            if tx_event
                .send(Ok(ResponseEvent::OutputItemDone(item)))
                .await
                .is_err()
            {
                return;
            }
        }

        let _ = tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id,
                token_usage,
                end_turn: None,
            }))
            .await;
    }

    fn message_id(&self) -> String {
        format!("msg_{}", self.response_id())
    }

    fn response_id(&self) -> String {
        self.response_id
            .clone()
            .or_else(|| self.response_id_hint.clone())
            .unwrap_or_else(|| "chatcmpl-unknown".to_string())
    }

    fn reasoning_id(&self) -> String {
        format!("rs_{}", self.response_id())
    }

    async fn ensure_reasoning_item_added(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if self.reasoning_added {
            return true;
        }

        let item = ResponseItem::Reasoning {
            id: Some(self.reasoning_id()),
            summary: Vec::new(),
            content: Some(Vec::new()),
            encrypted_content: None,
            metadata: None,
        };
        if tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(item)))
            .await
            .is_err()
        {
            return false;
        }
        self.reasoning_added = true;
        true
    }

    async fn finish_reasoning_item(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if !self.reasoning_added || self.reasoning_done {
            return true;
        }

        let content = (!self.reasoning_text.is_empty()).then(|| {
            vec![ReasoningItemContent::ReasoningText {
                text: self.reasoning_text.clone(),
            }]
        });
        let item = ResponseItem::Reasoning {
            id: Some(self.reasoning_id()),
            summary: Vec::<ReasoningItemReasoningSummary>::new(),
            content,
            encrypted_content: None,
            metadata: None,
        };
        if tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_err()
        {
            return false;
        }
        self.reasoning_done = true;
        true
    }

    fn should_delay_text_delta(&self) -> bool {
        self.emitted_text_len == 0 && is_potential_serialized_tool_text(&self.message_text)
    }

    async fn emit_pending_text_delta(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> bool {
        if self.emitted_text_len >= self.message_text.len() {
            return true;
        }

        if !self.message_added {
            let item = ResponseItem::Message {
                id: Some(self.message_id()),
                role: "assistant".to_string(),
                content: Vec::new(),
                phase: None,
                metadata: None,
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

        let delta = self.message_text[self.emitted_text_len..].to_string();
        self.emitted_text_len = self.message_text.len();
        tx_event
            .send(Ok(ResponseEvent::OutputTextDelta(delta)))
            .await
            .is_ok()
    }
}

fn is_potential_serialized_tool_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return true;
    }
    if !trimmed.starts_with('{') {
        return false;
    }

    let probe: String = trimmed.chars().take(256).collect();
    if probe.contains("\"function_call\"") || probe.contains("\"custom_tool_call\"") {
        return true;
    }

    probe.len() < 256
}

fn parse_serialized_function_call_text(text: &str) -> Result<Option<ResponseItem>, String> {
    if !looks_like_serialized_tool_call(text) {
        return Ok(None);
    }

    let trimmed = text.trim();
    let parsed: TextFunctionCall = serde_json::from_str(trimmed).map_err(|err| {
        format!(
            "chat completions stream emitted malformed function-call JSON as assistant text: \
             {err}; raw excerpt: {}",
            diagnostic_excerpt(trimmed)
        )
    })?;

    if parsed.kind != "function_call" {
        return Err(format!(
            "chat completions stream emitted unsupported serialized tool call type `{}` as \
             assistant text; raw excerpt: {}",
            parsed.kind,
            diagnostic_excerpt(trimmed)
        ));
    }

    let name = parsed.name.filter(|name| !name.is_empty()).ok_or_else(|| {
        format!(
            "chat completions stream emitted serialized function_call text without a name; \
             raw excerpt: {}",
            diagnostic_excerpt(trimmed)
        )
    })?;
    let call_id = parsed
        .call_id
        .filter(|call_id| !call_id.is_empty())
        .ok_or_else(|| {
            format!(
                "chat completions stream emitted serialized function_call text without call_id; \
             raw excerpt: {}",
                diagnostic_excerpt(trimmed)
            )
        })?;
    let arguments = parsed.arguments.ok_or_else(|| {
        format!(
            "chat completions stream emitted serialized function_call text without arguments; \
             raw excerpt: {}",
            diagnostic_excerpt(trimmed)
        )
    })?;
    let arguments = match arguments {
        Value::String(arguments) => arguments,
        other => serde_json::to_string(&other).map_err(|err| {
            format!(
                "chat completions stream emitted function_call arguments that could not be \
                 serialized: {err}; raw excerpt: {}",
                diagnostic_excerpt(trimmed)
            )
        })?,
    };

    Ok(Some(ResponseItem::FunctionCall {
        id: Some(format!("fc_{call_id}")),
        name,
        namespace: None,
        arguments,
        call_id,
        metadata: None,
    }))
}

fn looks_like_serialized_tool_call(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('{')
        && (trimmed.contains("\"function_call\"") || trimmed.contains("\"custom_tool_call\""))
}

fn diagnostic_excerpt(text: &str) -> String {
    const MAX_EXCERPT_CHARS: usize = 800;
    let mut excerpt: String = text.chars().take(MAX_EXCERPT_CHARS).collect();
    if text.chars().count() > MAX_EXCERPT_CHARS {
        excerpt.push_str("...");
    }
    excerpt.replace('\n', "\\n")
}

async fn process_chat_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    response_id_hint: Option<String>,
) {
    let mut stream = stream.eventsource();
    let mut state = ChatStreamState::new(response_id_hint);

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("Chat completions SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before chat completions finished".into(),
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

        trace!("Chat completions SSE event: {}", &sse.data);

        if sse.data.trim() == "[DONE]" {
            state.complete(&tx_event).await;
            return;
        }

        if let Ok(error) = serde_json::from_str::<ChatErrorEnvelope>(&sse.data) {
            let mut message = error
                .error
                .message
                .unwrap_or_else(|| "chat completions stream returned an error".to_string());
            if let Some(code) = error.error.code
                && !code.is_empty()
            {
                message = format!("{message} ({code})");
            }
            let _ = tx_event.send(Err(ApiError::Stream(message))).await;
            return;
        }

        let chunk = match serde_json::from_str::<ChatCompletionChunk>(&sse.data) {
            Ok(chunk) => chunk,
            Err(err) => {
                debug!(
                    "failed to parse chat completions SSE event: {err}, data: {}",
                    &sse.data
                );
                continue;
            }
        };

        if !state.process_chunk(chunk, &tx_event).await {
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
        process_chat_sse(
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

    fn content_event(id: &str, content: &str) -> Vec<u8> {
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": id,
                "choices": [
                    {
                        "delta": {
                            "content": content,
                        }
                    }
                ],
            })
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn parses_text_deltas_and_usage() {
        let events = collect_events(&[
            br#"data: {"id":"chatcmpl-1","model":"ambient/large","choices":[{"delta":{"role":"assistant","content":"he"}}],"usage":null}"#,
            b"\n\n",
            br#"data: {"id":"chatcmpl-1","model":"ambient/large","choices":[{"delta":{"content":"llo"}}],"usage":null}"#,
            b"\n\n",
            br#"data: {"id":"chatcmpl-1","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5,"prompt_tokens_details":{"cached_tokens":1},"completion_tokens_details":{"reasoning_tokens":0}}}"#,
            b"\n\n",
            b"data: [DONE]\n\n",
        ])
        .await;

        assert_matches!(&events[0], Ok(ResponseEvent::ServerModel(model)) if model == "ambient/large");
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
        );
        assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "he");
        assert_matches!(&events[3], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "llo");
        assert_matches!(
            &events[4],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
                if content == &vec![ContentItem::OutputText { text: "hello".to_string() }]
        );
        assert_matches!(
            &events[5],
            Ok(ResponseEvent::Completed {
                response_id,
                token_usage: Some(TokenUsage {
                    input_tokens: 3,
                    cached_input_tokens: 1,
                    output_tokens: 2,
                    reasoning_output_tokens: 0,
                    total_tokens: 5,
                }),
                ..
            }) if response_id == "chatcmpl-1"
        );
        assert_eq!(events.len(), 6);
    }

    #[tokio::test]
    async fn parses_streamed_tool_call() {
        let events = collect_events(&[
            br#"data: {"id":"chatcmpl-2","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"exec_command","arguments":"{\"cmd\":"}}]}}]}"#,
            b"\n\n",
            br#"data: {"id":"chatcmpl-2","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"date\"}"}}]}}]}"#,
            b"\n\n",
            b"data: [DONE]\n\n",
        ])
        .await;

        assert_matches!(
            &events[0],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            })) if name == "exec_command" && arguments == "{\"cmd\":\"date\"}" && call_id == "call_1"
        );
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::Completed { response_id, .. }) if response_id == "chatcmpl-2"
        );
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn parses_reasoning_content_without_leaking_as_text() {
        let events = collect_events(&[
            br#"data: {"id":"chatcmpl-reasoning","choices":[{"delta":{"reasoning_content":"private thought "}}]}"#,
            b"\n\n",
            br#"data: {"id":"chatcmpl-reasoning","choices":[{"delta":{"reasoning_content":"trace"}}]}"#,
            b"\n\n",
            br#"data: {"id":"chatcmpl-reasoning","choices":[{"delta":{"content":"visible answer"}}]}"#,
            b"\n\n",
            b"data: [DONE]\n\n",
        ])
        .await;

        assert_matches!(
            &events[0],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning { id: Some(id), .. }))
                if id == "rs_chatcmpl-reasoning"
        );
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::ReasoningContentDelta { delta, content_index })
                if delta == "private thought " && *content_index == 0
        );
        assert_matches!(
            &events[2],
            Ok(ResponseEvent::ReasoningContentDelta { delta, content_index })
                if delta == "trace" && *content_index == 0
        );
        assert_matches!(
            &events[3],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                content: Some(content),
                ..
            })) if content == &vec![ReasoningItemContent::ReasoningText {
                text: "private thought trace".to_string(),
            }]
        );
        assert_matches!(
            &events[4],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
        );
        assert_matches!(
            &events[5],
            Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "visible answer"
        );
        assert_matches!(
            &events[6],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
                if content == &vec![ContentItem::OutputText {
                    text: "visible answer".to_string(),
                }]
        );
        assert_matches!(
            &events[7],
            Ok(ResponseEvent::Completed { response_id, .. }) if response_id == "chatcmpl-reasoning"
        );
        assert_eq!(events.len(), 8);
    }

    #[tokio::test]
    async fn parses_serialized_function_call_text_without_leaking_as_text() {
        let content = serde_json::json!({
            "type": "function_call",
            "name": "exec_command",
            "arguments": {
                "cmd": "date"
            },
            "call_id": "chatcmpl-tool-1",
        })
        .to_string();
        let event = content_event("chatcmpl-serialized", &content);

        let events = collect_events(&[event.as_slice(), b"data: [DONE]\n\n"]).await;

        assert_matches!(
            &events[0],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            })) if name == "exec_command" && arguments == "{\"cmd\":\"date\"}" && call_id == "chatcmpl-tool-1"
        );
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::Completed { response_id, .. }) if response_id == "chatcmpl-serialized"
        );
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn errors_on_malformed_serialized_function_call_text() {
        let content = r#"{"type":"function_call","name":"exec_command","arguments":"{"cmd":"date"}","call_id":"chatcmpl-tool-bad"}"#;
        let event = content_event("chatcmpl-malformed", content);

        let events = collect_events(&[event.as_slice(), b"data: [DONE]\n\n"]).await;

        assert_matches!(
            &events[0],
            Err(ApiError::Stream(message))
                if message.contains("malformed function-call JSON as assistant text")
                    && message.contains("raw excerpt")
                    && message.contains("chatcmpl-tool-bad")
        );
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn errors_on_tool_call_delta_without_function_name() {
        let events = collect_events(&[
            br#"data: {"id":"chatcmpl-nameless","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"arguments":"{\"cmd\":\"date\"}"}}]}}]}"#,
            b"\n\n",
            b"data: [DONE]\n\n",
        ])
        .await;

        assert_matches!(
            &events[0],
            Err(ApiError::Stream(message))
                if message.contains("tool call without a function name")
                    && message.contains("call_id=call_1")
                    && message.contains(r#"{"cmd":"date"}"#)
        );
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn returns_error_when_stream_closes_without_done() {
        let reader = IoBuilder::new()
            .read(br#"data: {"id":"chatcmpl-3","choices":[]}"#)
            .read(b"\n\n")
            .build();
        let body =
            ReaderStream::new(reader).map_err(|err| TransportError::Network(err.to_string()));
        let (tx_event, mut rx_event) = mpsc::channel(1600);

        process_chat_sse(
            Box::pin(body),
            tx_event,
            Duration::from_secs(5),
            /*telemetry*/ None,
            None,
        )
        .await;

        let event = rx_event.recv().await.expect("event should be emitted");
        assert_matches!(event, Err(ApiError::Stream(_)));
    }
}
