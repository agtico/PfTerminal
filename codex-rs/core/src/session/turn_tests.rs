use super::*;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributor;
use codex_protocol::items::AgentMessageContent;
use pretty_assertions::assert_eq;
use std::sync::Arc;

struct RewriteAgentMessageContributor;

impl TurnItemContributor for RewriteAgentMessageContributor {
    fn contribute<'a>(
        &'a self,
        _thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> codex_extension_api::ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let TurnItem::AgentMessage(agent_message) = item {
                agent_message.content = vec![AgentMessageContent::Text {
                    text: "plan contributed assistant text".to_string(),
                }];
            }
            Ok(())
        })
    }
}

fn assistant_output_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some("msg-1".to_string()),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        metadata: None,
    }
}

#[test]
fn explicit_shell_command_budget_parses_common_phrasings() {
    let cases = [
        ("Use at most 5 shell commands.", Some(5)),
        ("Run no more than 4 commands, then answer.", Some(4)),
        ("Maximum of 3 shell commands for this review.", Some(3)),
        ("Max 2 commands.", Some(2)),
        ("Use 6 or fewer shell commands.", Some(6)),
        ("Use at most 0 shell commands.", None),
    ];

    for (text, expected) in cases {
        assert_eq!(
            explicit_shell_command_budget_from_text(text),
            expected,
            "{text}"
        );
    }
}

#[test]
fn explicit_shell_command_budget_ignores_non_command_numbers() {
    assert_eq!(
        explicit_shell_command_budget_from_text(
            "Review the 5 largest files and finish within 300 seconds."
        ),
        None
    );
    assert_eq!(
        explicit_shell_command_budget_from_text(
            "Use at most 7 shell commands, but no more than 5 commands if possible."
        ),
        Some(5)
    );
}

fn token_usage(input_tokens: i64, cached_input_tokens: i64) -> TokenUsage {
    TokenUsage {
        input_tokens,
        cached_input_tokens,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: input_tokens,
    }
}

#[tokio::test]
async fn provider_request_lease_guard_releases_on_drop() {
    let codex_home = tempfile::tempdir().expect("tempdir");
    let state_db = codex_state::StateRuntime::init(
        codex_home.path().to_path_buf(),
        "test-provider".to_string(),
    )
    .await
    .expect("state db");
    let key = ProviderRequestKey {
        provider_id: "zai".to_string(),
        model: "glm-5.2".to_string(),
        key_fingerprint: "stored:ZAI_API_KEY:test".to_string(),
    };
    let preflight = ProviderRequestPreflight {
        input_tokens: 91_817,
        cached_input_tokens: 0,
        request_bytes: 375_150,
        thread_id: Some("thread-a".to_string()),
        turn_id: Some("turn-a".to_string()),
    };
    let ProviderRequestLeaseDecision::Acquired(lease) = state_db
        .try_acquire_provider_request_lease(&key, &preflight, "worker-a", 600_000, 1_000)
        .await
        .expect("acquire first lease")
    else {
        panic!("expected first lease");
    };

    drop(ProviderRequestLeaseGuard {
        state_db: Some(state_db.clone()),
        runtime_handle: tokio::runtime::Handle::current(),
        lease: Some(lease),
    });

    for attempt in 0..20 {
        let decision = state_db
            .try_acquire_provider_request_lease(
                &key,
                &preflight,
                "worker-b",
                600_000,
                2_000 + attempt * 100,
            )
            .await
            .expect("retry lease");
        if matches!(decision, ProviderRequestLeaseDecision::Acquired(_)) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("drop guard did not release provider request lease");
}

#[test]
fn third_party_cache_health_uses_last_provider_usage() {
    let healthy = token_usage(17_136, 17_088);

    assert!(third_party_cache_looks_healthy(Some(&healthy)));
    assert!(!third_party_cache_miss_is_known(Some(&healthy)));
}

#[test]
fn third_party_cache_miss_requires_large_recent_usage() {
    let miss = token_usage(17_136, 0);
    let small = token_usage(1_000, 0);

    assert!(third_party_cache_miss_is_known(Some(&miss)));
    assert!(!third_party_cache_looks_healthy(Some(&miss)));
    assert!(!third_party_cache_miss_is_known(Some(&small)));
    assert!(!third_party_cache_looks_healthy(Some(&small)));
    assert_eq!(cache_hit_rate(None), None);
}

#[tokio::test]
async fn plan_mode_uses_contributed_turn_item_for_last_agent_message() {
    let (mut session, turn_context) = crate::session::tests::make_session_and_context().await;
    let mut builder = codex_extension_api::ExtensionRegistryBuilder::new();
    builder.turn_item_contributor(Arc::new(RewriteAgentMessageContributor));
    session.services.extensions = Arc::new(builder.build());
    let turn_store = ExtensionData::new(turn_context.sub_id.clone());
    let mut state = PlanModeStreamState::new(&turn_context.sub_id);
    let mut last_agent_message = None;
    let item = assistant_output_text("original assistant text");

    let handled = handle_assistant_item_done_in_plan_mode(
        &session,
        &turn_context,
        &turn_store,
        &item,
        &mut state,
        /*previously_active_item*/ None,
        &mut last_agent_message,
    )
    .await;

    assert!(handled);
    assert_eq!(
        last_agent_message.as_deref(),
        Some("plan contributed assistant text")
    );
}
