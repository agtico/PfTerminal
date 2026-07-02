//! Shared retry and transport fallback decisions for Responses requests.

use std::time::Duration;

use crate::client::ModelClientSession;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::util::backoff;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use tracing::warn;

const MAX_SAME_REQUEST_IDLE_FAILURES: u64 = 2;
const SSE_IDLE_TIMEOUT_MESSAGE: &str = "idle timeout waiting for SSE";

#[derive(Debug, Clone, Copy)]
pub(crate) enum ResponsesStreamRequest {
    Sampling,
    RemoteCompactionV2,
}

pub(crate) fn guard_same_request_idle_retry(
    err: &CodexErr,
    same_request_idle_failures: &mut u64,
) -> Result<(), CodexErr> {
    if is_sse_idle_timeout(err) {
        *same_request_idle_failures = (*same_request_idle_failures).saturating_add(1);
        if *same_request_idle_failures >= MAX_SAME_REQUEST_IDLE_FAILURES {
            return Err(CodexErr::Stream(
                format!(
                    "stream idle timeout repeated {} times for the same \
                     request; aborting instead of restarting it again",
                    *same_request_idle_failures,
                ),
                None,
            ));
        }
    } else {
        *same_request_idle_failures = 0;
    }

    Ok(())
}

fn is_sse_idle_timeout(err: &CodexErr) -> bool {
    matches!(
        err,
        CodexErr::Stream(message, _) if message.contains(SSE_IDLE_TIMEOUT_MESSAGE)
    )
}

/// Handles a retryable stream error and returns `Ok(())` when the caller should
/// retry the request loop.
pub(crate) async fn handle_retryable_response_stream_error(
    retries: &mut u64,
    max_retries: u64,
    err: CodexErr,
    client_session: &mut ModelClientSession,
    sess: &Session,
    turn_context: &TurnContext,
    request: ResponsesStreamRequest,
) -> Result<(), CodexErr> {
    if *retries >= max_retries
        && client_session.try_switch_fallback_transport(
            &turn_context.session_telemetry,
            &turn_context.model_info,
        )
    {
        sess.send_event(
            turn_context,
            EventMsg::Warning(WarningEvent {
                message: format!("Falling back from WebSockets to HTTPS transport. {err:#}"),
            }),
        )
        .await;
        *retries = 0;
        return Ok(());
    }

    if *retries < max_retries {
        *retries += 1;
        let retry_count = *retries;
        let delay = match &err {
            CodexErr::Stream(_, requested_delay) => {
                requested_delay.unwrap_or_else(|| backoff(retry_count))
            }
            _ => backoff(retry_count),
        };
        log_retry(request, turn_context, &err, retry_count, max_retries, delay);

        // In release builds, hide the first websocket retry notification to reduce noisy
        // transient reconnect messages. In debug builds, keep full visibility for diagnosis.
        let report_error = retry_count > 1
            || cfg!(debug_assertions)
            || !sess.services.responses_websocket_enabled();
        if report_error {
            // Surface retry information to any UI/front-end so the user understands what is
            // happening instead of staring at a seemingly frozen screen.
            sess.notify_stream_error(
                turn_context,
                format!("Reconnecting... {retry_count}/{max_retries}"),
                err,
            )
            .await;
        }
        tokio::time::sleep(delay).await;
        return Ok(());
    }

    Err(err)
}

fn log_retry(
    request: ResponsesStreamRequest,
    turn_context: &TurnContext,
    err: &CodexErr,
    retries: u64,
    max_retries: u64,
    delay: Duration,
) {
    match request {
        ResponsesStreamRequest::Sampling => {
            warn!(
                "stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})...",
            );
        }
        ResponsesStreamRequest::RemoteCompactionV2 => {
            warn!(
                turn_id = %turn_context.sub_id,
                retries,
                max_retries,
                compact_error = %err,
                "remote compaction v2 stream failed; retrying request after delay"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_request_idle_guard_aborts_after_second_idle_failure() {
        let mut failures = 0;
        let idle = CodexErr::Stream(SSE_IDLE_TIMEOUT_MESSAGE.to_string(), None);

        guard_same_request_idle_retry(&idle, &mut failures).expect("first idle failure can retry");
        assert_eq!(failures, 1);

        let err = guard_same_request_idle_retry(&idle, &mut failures)
            .expect_err("second same-request idle failure should abort");
        assert_eq!(failures, 2);
        assert!(
            err.to_string()
                .contains("aborting instead of restarting it again")
        );
    }

    #[test]
    fn same_request_idle_guard_resets_on_non_idle_error() {
        let mut failures = 1;
        let other = CodexErr::Stream("stream closed before completion".to_string(), None);

        guard_same_request_idle_retry(&other, &mut failures)
            .expect("non-idle stream error should not abort");

        assert_eq!(failures, 0);
    }
}
