use crate::TransportError;
use crate::error::ApiError;
use crate::rate_limits::parse_promo_message;
use crate::rate_limits::parse_rate_limit_for_limit;
use crate::rate_limits::parse_rate_limit_reached_type;
use base64::Engine;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::auth::PlanType;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::RetryLimitReachedError;
use codex_protocol::error::UnexpectedResponseError;
use codex_protocol::error::UsageLimitReachedError;
use http::HeaderMap;
use serde::Deserialize;
use serde_json::Value;

pub fn map_api_error(err: ApiError) -> CodexErr {
    match err {
        ApiError::ContextWindowExceeded => CodexErr::ContextWindowExceeded,
        ApiError::QuotaExceeded => CodexErr::QuotaExceeded,
        ApiError::UsageNotIncluded => CodexErr::UsageNotIncluded,
        ApiError::Retryable { message, delay } => {
            let error = CodexErr::Stream(message);
            match delay {
                Some(delay) => error.with_retry_delay(delay),
                None => error,
            }
        }
        ApiError::Stream(msg) => CodexErr::Stream(msg),
        ApiError::ServerOverloaded => CodexErr::ServerOverloaded,
        ApiError::Api { status, message } => {
            let user_message = api_error_user_message(status, &message);
            CodexErr::UnexpectedStatus(UnexpectedResponseError {
                status,
                body: message,
                user_message,
                url: None,
                cf_ray: None,
                request_id: None,
                identity_authorization_error: None,
                identity_error_code: None,
            })
        }
        ApiError::InvalidRequest { message } => CodexErr::InvalidRequest(message),
        ApiError::CyberPolicy { message } => {
            CodexErr::new(CodexErrorDetails::CyberPolicy { message })
        }
        ApiError::Transport(transport) => match transport {
            TransportError::Http {
                status,
                url,
                headers,
                body,
            } => {
                let body_text = body.unwrap_or_default();

                if status == http::StatusCode::SERVICE_UNAVAILABLE
                    && let Ok(value) = serde_json::from_str::<serde_json::Value>(&body_text)
                    && matches!(
                        value
                            .get("error")
                            .and_then(|error| error.get("code"))
                            .and_then(serde_json::Value::as_str),
                        Some("server_is_overloaded" | "slow_down")
                    )
                {
                    return CodexErr::ServerOverloaded;
                }

                if status == http::StatusCode::BAD_REQUEST {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&body_text)
                        && let Some(error) = parsed.get("error")
                        && error.get("code").and_then(Value::as_str)
                            == Some(CYBER_POLICY_ERROR_CODE)
                    {
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .filter(|message| !message.trim().is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| CYBER_POLICY_FALLBACK_MESSAGE.to_string());
                        CodexErr::new(CodexErrorDetails::CyberPolicy { message })
                    } else if body_text
                        .contains("The image data you provided does not represent a valid image")
                    {
                        CodexErr::InvalidImageRequest()
                    } else {
                        CodexErr::InvalidRequest(body_text)
                    }
                } else if status == http::StatusCode::INTERNAL_SERVER_ERROR {
                    CodexErr::InternalServerError
                } else if status == http::StatusCode::UNAUTHORIZED
                    && classify_unauthorized_response(&body_text)
                        == UnauthorizedResponseKind::Entitlement
                {
                    // Some providers (e.g. Kimi Code) answer requests beyond the
                    // plan's context entitlement with 401 instead of 4xx/413.
                    // Classify separately from an invalid credential so users get
                    // actionable guidance.
                    CodexErr::PlanEntitlementExceeded(body_text)
                } else if status == http::StatusCode::TOO_MANY_REQUESTS {
                    if let Ok(err) = serde_json::from_str::<UsageErrorResponse>(&body_text) {
                        if matches!(
                            err.error.error_type.as_deref(),
                            Some("usage_limit_reached" | "plan_limit_reached")
                        ) {
                            let limit_id = extract_header(headers.as_ref(), ACTIVE_LIMIT_HEADER);
                            let promo_message = headers.as_ref().and_then(parse_promo_message);
                            let rate_limit_reached_type =
                                headers.as_ref().and_then(parse_rate_limit_reached_type);
                            let rate_limits = headers
                                .as_ref()
                                .and_then(|map| {
                                    parse_rate_limit_for_limit(map, limit_id.as_deref())
                                })
                                .map(|mut snapshot| {
                                    snapshot.rate_limit_reached_type = rate_limit_reached_type;
                                    snapshot
                                });
                            let resets_at = err
                                .error
                                .resets_at
                                .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0));
                            return CodexErr::UsageLimitReached(UsageLimitReachedError {
                                plan_type: err.error.plan_type,
                                resets_at,
                                rate_limits: rate_limits.map(Box::new),
                                promo_message,
                                rate_limit_reached_type,
                            });
                        } else if err.error.error_type.as_deref() == Some("usage_not_included") {
                            return CodexErr::UsageNotIncluded;
                        }
                    }

                    CodexErr::RetryLimit(RetryLimitReachedError {
                        status,
                        request_id: extract_request_tracking_id(headers.as_ref()),
                        retry_after_ms: extract_retry_after_ms(headers.as_ref()),
                    })
                } else {
                    CodexErr::UnexpectedStatus(UnexpectedResponseError {
                        status,
                        user_message: api_error_user_message(status, &body_text),
                        body: body_text,
                        url,
                        cf_ray: extract_header(headers.as_ref(), CF_RAY_HEADER),
                        request_id: extract_request_id(headers.as_ref()),
                        identity_authorization_error: extract_header(
                            headers.as_ref(),
                            X_OPENAI_AUTHORIZATION_ERROR_HEADER,
                        ),
                        identity_error_code: extract_x_error_json_code(headers.as_ref()),
                    })
                }
            }
            TransportError::RetryLimit => CodexErr::RetryLimit(RetryLimitReachedError {
                status: http::StatusCode::INTERNAL_SERVER_ERROR,
                request_id: None,
                retry_after_ms: None,
            }),
            TransportError::Timeout => CodexErr::RequestTimeout,
            TransportError::Network(msg) | TransportError::Build(msg) => CodexErr::Stream(msg),
        },
        ApiError::RateLimit(msg) => CodexErr::Stream(msg),
    }
}

const ACTIVE_LIMIT_HEADER: &str = "x-codex-active-limit";
const REQUEST_ID_HEADER: &str = "x-request-id";
const OAI_REQUEST_ID_HEADER: &str = "x-oai-request-id";
const CF_RAY_HEADER: &str = "cf-ray";
const RETRY_AFTER_HEADER: &str = "retry-after";
const RETRY_AFTER_MS_HEADER: &str = "retry-after-ms";
const X_RATELIMIT_RESET_HEADER: &str = "x-ratelimit-reset";
const X_RATELIMIT_RESET_MS_HEADER: &str = "x-ratelimit-reset-ms";
const X_OPENAI_AUTHORIZATION_ERROR_HEADER: &str = "x-openai-authorization-error";
const X_ERROR_JSON_HEADER: &str = "x-error-json";
const CYBER_POLICY_ERROR_CODE: &str = "cyber_policy";
const CYBER_POLICY_FALLBACK_MESSAGE: &str =
    "This request has been flagged for possible cybersecurity risk.";
const CLOUDFLARE_BLOCKED_MESSAGE: &str =
    "Access blocked by Cloudflare. This usually happens when connecting from a restricted region";

/// The reason a provider returned HTTP 401.
///
/// Some plan-backed providers incorrectly use 401 for request-entitlement failures. Callers use
/// this classification before deciding whether rotating credentials can actually help.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnauthorizedResponseKind {
    Authentication,
    Entitlement,
    Unknown,
}

pub fn classify_unauthorized_response(body: &str) -> UnauthorizedResponseKind {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let error = value.get("error").unwrap_or(&value);
        let error_type = error
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let error_code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();

        if [error_type.as_str(), error_code.as_str()]
            .iter()
            .any(|value| {
                value.contains("authentication")
                    || value.contains("invalid_token")
                    || value.contains("token_expired")
                    || value.contains("invalid_api_key")
            })
        {
            return UnauthorizedResponseKind::Authentication;
        }
    }

    if indicates_plan_entitlement_rejection(body) {
        UnauthorizedResponseKind::Entitlement
    } else if indicates_authentication_rejection(body) {
        UnauthorizedResponseKind::Authentication
    } else {
        UnauthorizedResponseKind::Unknown
    }
}

/// Heuristic for providers that signal "request exceeds plan entitlement" with a 401.
/// Matches on semantic phrases (context/token limit combined with plan/quota language),
/// never on one provider's exact sentence.
fn indicates_plan_entitlement_rejection(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    let mentions_context_or_tokens = lower.contains("context") || lower.contains("token");
    let mentions_entitlement = lower.contains("plan")
        || lower.contains("quota")
        || lower.contains("entitlement")
        || lower.contains("exceed")
        || lower.contains("maximum allowed");
    mentions_context_or_tokens && mentions_entitlement
}

fn indicates_authentication_rejection(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    let mentions_credential = lower.contains("access token")
        || lower.contains("oauth token")
        || lower.contains("api key")
        || lower.contains("authorization header")
        || lower.contains("credential");
    let mentions_rejection = lower.contains("expired")
        || lower.contains("invalid")
        || lower.contains("revoked")
        || lower.contains("missing")
        || lower.contains("unauthorized");
    mentions_credential && mentions_rejection
}

#[cfg(test)]
#[path = "api_bridge_tests.rs"]
mod tests;

fn extract_request_tracking_id(headers: Option<&HeaderMap>) -> Option<String> {
    extract_request_id(headers).or_else(|| extract_header(headers, CF_RAY_HEADER))
}

fn api_error_user_message(status: http::StatusCode, body: &str) -> Option<String> {
    if status == http::StatusCode::FORBIDDEN
        && body.contains("Cloudflare")
        && body.contains("blocked")
    {
        Some(format!("{CLOUDFLARE_BLOCKED_MESSAGE} (status {status})"))
    } else {
        None
    }
}

fn extract_request_id(headers: Option<&HeaderMap>) -> Option<String> {
    extract_header(headers, REQUEST_ID_HEADER)
        .or_else(|| extract_header(headers, OAI_REQUEST_ID_HEADER))
}

fn extract_header(headers: Option<&HeaderMap>, name: &str) -> Option<String> {
    headers.and_then(|map| {
        map.get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    })
}

fn extract_retry_after_ms(headers: Option<&HeaderMap>) -> Option<i64> {
    extract_header_millis_delta(headers, RETRY_AFTER_MS_HEADER)
        .or_else(|| extract_retry_after_seconds_or_date(headers))
        .or_else(|| extract_header_reset_delta_ms(headers, X_RATELIMIT_RESET_MS_HEADER))
        .or_else(|| extract_header_reset_delta_ms(headers, X_RATELIMIT_RESET_HEADER))
}

fn extract_header_millis_delta(headers: Option<&HeaderMap>, name: &str) -> Option<i64> {
    let value = extract_header(headers, name)?;
    value.trim().parse::<i64>().ok().filter(|ms| *ms > 0)
}

fn extract_retry_after_seconds_or_date(headers: Option<&HeaderMap>) -> Option<i64> {
    let value = extract_header(headers, RETRY_AFTER_HEADER)?;
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return (seconds > 0).then_some(seconds.saturating_mul(1000));
    }
    let retry_at = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    Some(
        retry_at
            .timestamp_millis()
            .saturating_sub(Utc::now().timestamp_millis())
            .max(0),
    )
    .filter(|ms| *ms > 0)
}

fn extract_header_reset_delta_ms(headers: Option<&HeaderMap>, name: &str) -> Option<i64> {
    let value = extract_header(headers, name)?;
    let reset = value.trim().parse::<i64>().ok()?;
    let reset_ms = if reset > 10_000_000_000 {
        reset
    } else {
        reset.saturating_mul(1000)
    };
    Some(reset_ms.saturating_sub(Utc::now().timestamp_millis())).filter(|ms| *ms > 0)
}

fn extract_x_error_json_code(headers: Option<&HeaderMap>) -> Option<String> {
    let encoded = extract_header(headers, X_ERROR_JSON_HEADER)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let parsed = serde_json::from_slice::<Value>(&decoded).ok()?;
    parsed
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[derive(Debug, Deserialize)]
struct UsageErrorResponse {
    error: UsageErrorBody,
}

#[derive(Debug, Deserialize)]
struct UsageErrorBody {
    #[serde(rename = "type")]
    error_type: Option<String>,
    plan_type: Option<PlanType>,
    resets_at: Option<i64>,
}
