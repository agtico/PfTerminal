use codex_model_provider_info::ANTHROPIC_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use wiremock::Mock;
use wiremock::Request;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

#[derive(Default)]
struct PayloadTooLargeThenSuccess {
    calls: AtomicUsize,
}

#[derive(Default)]
struct CutoffThenSuccess {
    calls: AtomicUsize,
}

#[derive(Default)]
struct SignedThinkingStreamErrorThenSuccess {
    calls: AtomicUsize,
}

impl Respond for PayloadTooLargeThenSuccess {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => ResponseTemplate::new(413).set_body_json(serde_json::json!({
                "type": "error",
                "error": {
                    "type": "request_too_large",
                    "message": "Request exceeds the maximum size",
                }
            })),
            1 => anthropic_success_response(),
            call => panic!("unexpected Anthropic request {call}"),
        }
    }
}

impl Respond for CutoffThenSuccess {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => anthropic_length_truncated_tool_response(),
            1 => anthropic_success_response(),
            call => panic!("unexpected Anthropic request {call}"),
        }
    }
}

impl Respond for SignedThinkingStreamErrorThenSuccess {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => anthropic_signed_thinking_response(),
            1 => anthropic_signed_thinking_history_error_response(),
            2 => anthropic_success_response(),
            call => panic!("unexpected Anthropic request {call}"),
        }
    }
}

fn anthropic_success_response() -> ResponseTemplate {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_payload_retry\",",
        "\"model\":\"claude-opus-5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,",
        "\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,",
        "\"delta\":{\"type\":\"text_delta\",\"text\":\"Recovered after pruning.\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},",
        "\"usage\":{\"input_tokens\":10,\"output_tokens\":4}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

fn anthropic_length_truncated_tool_response() -> ResponseTemplate {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_cutoff\",",
        "\"model\":\"claude-opus-5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,",
        "\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,",
        "\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"write the file\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,",
        "\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-cutoff\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,",
        "\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_cutoff\",",
        "\"name\":\"structured_write\",\"input\":{\"path\":\"should-not-exist.txt\",",
        "\"mode\":\"overwrite\",\"content\":\"unsafe partial result\"}}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},",
        "\"usage\":{\"input_tokens\":10,\"output_tokens\":32000}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

fn anthropic_signed_thinking_response() -> ResponseTemplate {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_signed_history\",",
        "\"model\":\"claude-opus-5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,",
        "\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,",
        "\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"inspect state\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,",
        "\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-history\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,",
        "\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,",
        "\"delta\":{\"type\":\"text_delta\",\"text\":\"Initial answer.\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},",
        "\"usage\":{\"input_tokens\":10,\"output_tokens\":4}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

fn anthropic_signed_thinking_history_error_response() -> ResponseTemplate {
    let body = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",",
        "\"message\":\"messages.299.content.1: `thinking` or `redacted_thinking` blocks in the ",
        "latest assistant message cannot be modified. These blocks must remain as they were in ",
        "the original response.\"}}\n\n",
    );
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_413_retries_once_instead_of_poisoning_turn() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/messages$"))
        .respond_with(PayloadTooLargeThenSuccess::default())
        .expect(2)
        .mount(&server)
        .await;

    let provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".to_string()),
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(2_000),
        ..ModelProviderInfo::create_anthropic_provider()
    };
    let test = test_codex()
        .with_config(move |config| {
            config.model = Some("claude-opus-5".to_string());
            config.model_provider_id = ANTHROPIC_PROVIDER_ID.to_string();
            config.model_provider = provider;
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Continue the visual repair.".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    loop {
        let event = test.codex.next_event().await?;
        match event.msg {
            EventMsg::TurnComplete(_) => break,
            EventMsg::Error(error) => panic!("turn failed after recoverable 413: {error:?}"),
            _ => {}
        }
    }

    let requests = server.received_requests().await.expect("recorded requests");
    assert_eq!(requests.len(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_streamed_signed_thinking_rejection_repairs_and_retries_turn()
-> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/messages$"))
        .respond_with(SignedThinkingStreamErrorThenSuccess::default())
        .expect(3)
        .mount(&server)
        .await;

    let provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".to_string()),
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(2_000),
        ..ModelProviderInfo::create_anthropic_provider()
    };
    let test = test_codex()
        .with_config(move |config| {
            config.model = Some("claude-opus-5".to_string());
            config.model_provider_id = ANTHROPIC_PROVIDER_ID.to_string();
            config.model_provider = provider;
        })
        .build(&server)
        .await?;

    for prompt in [
        "Create signed history.",
        "Continue after the history rejection.",
    ] {
        test.codex
            .submit(Op::UserInput {
                items: vec![UserInput::Text {
                    text: prompt.to_string(),
                    text_elements: Vec::new(),
                }],
                final_output_json_schema: None,
                responsesapi_client_metadata: None,
                additional_context: Default::default(),
                thread_settings: Default::default(),
            })
            .await?;

        loop {
            let event = test.codex.next_event().await?;
            match event.msg {
                EventMsg::TurnComplete(_) => break,
                EventMsg::Error(error) => {
                    panic!("turn failed after streamed history repair: {error:?}")
                }
                _ => {}
            }
        }
    }

    let requests = server.received_requests().await.expect("recorded requests");
    assert_eq!(requests.len(), 3);
    let rejected_body = String::from_utf8_lossy(&requests[1].body);
    let repaired_body = String::from_utf8_lossy(&requests[2].body);
    assert!(rejected_body.contains("sig-history"));
    assert!(!repaired_body.contains("sig-history"));
    assert!(repaired_body.contains("Continue after the history rejection."));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_max_tokens_never_executes_tool_and_next_turn_repairs_history()
-> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/messages$"))
        .respond_with(CutoffThenSuccess::default())
        .expect(2)
        .mount(&server)
        .await;

    let provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".to_string()),
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(2_000),
        ..ModelProviderInfo::create_anthropic_provider()
    };
    let test = test_codex()
        .with_config(move |config| {
            config.model = Some("claude-opus-5".to_string());
            config.model_provider_id = ANTHROPIC_PROVIDER_ID.to_string();
            config.model_provider = provider;
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Write the file.".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    let mut saw_length_error = false;
    loop {
        let event = test.codex.next_event().await?;
        match event.msg {
            EventMsg::Error(error) => {
                assert!(
                    error
                        .message
                        .contains("stopped generation at its output or context limit"),
                    "{error:?}"
                );
                saw_length_error = true;
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }
    assert!(saw_length_error, "the truncated turn must surface an error");
    assert!(
        !test.cwd_path().join("should-not-exist.txt").exists(),
        "a length-truncated tool call must never touch the workspace"
    );

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Continue safely.".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    loop {
        let event = test.codex.next_event().await?;
        match event.msg {
            EventMsg::TurnComplete(_) => break,
            EventMsg::Error(error) => panic!("repaired follow-up failed: {error:?}"),
            _ => {}
        }
    }

    let requests = server.received_requests().await.expect("recorded requests");
    assert_eq!(requests.len(), 2);
    let second_body = String::from_utf8_lossy(&requests[1].body);
    assert!(!second_body.contains("sig-cutoff"));
    assert!(!second_body.contains("toolu_cutoff"));
    Ok(())
}
