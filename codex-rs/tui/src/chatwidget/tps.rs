use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

use crate::token_usage::TokenUsage;
use crate::token_usage::TokenUsageInfo;

const COMPLETED_SAMPLE_LIMIT: usize = 10;
const STREAM_ESTIMATE_CHARS_PER_TOKEN: f64 = 4.0;
const MIN_ACTIVE_SAMPLE_DURATION: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TpsTokenSource {
    ProviderUsage,
    StreamEstimate,
}

#[derive(Clone, Copy, Debug)]
struct TpsSample {
    generated_tokens: u64,
    duration: Duration,
    source: TpsTokenSource,
}

#[derive(Debug)]
struct ActiveModelCall {
    started_at: Instant,
    start_total_generated_tokens: u64,
    generated_tokens: u64,
    streamed_chars: usize,
    source: TpsTokenSource,
}

#[derive(Debug)]
struct PendingCompletedModelCall {
    start_total_generated_tokens: u64,
    duration: Duration,
}

#[derive(Debug, Default)]
pub(super) struct TpsEstimator {
    completed_samples: VecDeque<TpsSample>,
    active: Option<ActiveModelCall>,
    pending_completed: Option<PendingCompletedModelCall>,
}

impl TpsEstimator {
    pub(super) fn start_turn(
        &mut self,
        started_at: Instant,
        total_usage_at_start: Option<&TokenUsage>,
    ) {
        self.pending_completed = None;
        self.active = Some(ActiveModelCall {
            started_at,
            start_total_generated_tokens: total_usage_at_start
                .map(generated_tokens_for_usage)
                .unwrap_or_default(),
            generated_tokens: 0,
            streamed_chars: 0,
            source: TpsTokenSource::StreamEstimate,
        });
    }

    pub(super) fn record_stream_delta(&mut self, delta: &str) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if active.source == TpsTokenSource::ProviderUsage {
            return false;
        }

        let before = active.generated_tokens;
        active.streamed_chars = active.streamed_chars.saturating_add(delta.chars().count());
        active.generated_tokens = estimate_tokens_from_chars(active.streamed_chars);
        active.generated_tokens != before
    }

    pub(super) fn record_provider_usage(&mut self, info: &TokenUsageInfo) -> bool {
        let last_provider_tokens = provider_tokens_for_current_turn(info);

        if let Some(active) = self.active.as_mut() {
            let cumulative_tokens = generated_tokens_for_usage(&info.total_token_usage)
                .saturating_sub(active.start_total_generated_tokens);
            let provider_tokens = last_provider_tokens.max(cumulative_tokens);
            if provider_tokens == 0 {
                return false;
            }
            let changed = active.generated_tokens != provider_tokens
                || active.source != TpsTokenSource::ProviderUsage;
            active.generated_tokens = provider_tokens;
            active.source = TpsTokenSource::ProviderUsage;
            return changed;
        }

        if let Some(pending) = self.pending_completed.take() {
            let cumulative_tokens = generated_tokens_for_usage(&info.total_token_usage)
                .saturating_sub(pending.start_total_generated_tokens);
            let provider_tokens = last_provider_tokens.max(cumulative_tokens);
            if provider_tokens == 0 {
                self.pending_completed = Some(pending);
                return false;
            }
            self.push_completed_sample(TpsSample {
                generated_tokens: provider_tokens,
                duration: pending.duration,
                source: TpsTokenSource::ProviderUsage,
            });
            return true;
        }

        if last_provider_tokens == 0 {
            return false;
        }

        if let Some(sample) = self
            .completed_samples
            .back_mut()
            .filter(|sample| sample.source == TpsTokenSource::StreamEstimate)
        {
            let changed = sample.generated_tokens != last_provider_tokens
                || sample.source != TpsTokenSource::ProviderUsage;
            sample.generated_tokens = last_provider_tokens;
            sample.source = TpsTokenSource::ProviderUsage;
            return changed;
        }

        false
    }

    pub(super) fn complete_turn(&mut self, duration_ms: Option<i64>, now: Instant) -> bool {
        let Some(active) = self.active.take() else {
            return false;
        };
        let Some(duration) = positive_duration(duration_ms, now, active.started_at) else {
            return false;
        };
        if active.generated_tokens == 0 {
            self.pending_completed = Some(PendingCompletedModelCall {
                start_total_generated_tokens: active.start_total_generated_tokens,
                duration,
            });
            return false;
        }

        self.push_completed_sample(TpsSample {
            generated_tokens: active.generated_tokens,
            duration,
            source: active.source,
        });
        true
    }

    fn push_completed_sample(&mut self, sample: TpsSample) {
        self.completed_samples.push_back(sample);
        while self.completed_samples.len() > COMPLETED_SAMPLE_LIMIT {
            self.completed_samples.pop_front();
        }
    }

    pub(super) fn cancel_active(&mut self) {
        self.active = None;
        self.pending_completed = None;
    }

    pub(super) fn label(&self, now: Instant) -> String {
        let mut tokens: u64 = 0;
        let mut seconds = 0.0;
        let mut has_estimate = false;

        for sample in &self.completed_samples {
            if sample.generated_tokens == 0 || sample.duration.is_zero() {
                continue;
            }
            tokens = tokens.saturating_add(sample.generated_tokens);
            seconds += sample.duration.as_secs_f64();
            has_estimate |= sample.source == TpsTokenSource::StreamEstimate;
        }

        if let Some(active) = self.active_sample(now) {
            tokens = tokens.saturating_add(active.generated_tokens);
            seconds += active.duration.as_secs_f64();
            has_estimate |= active.source == TpsTokenSource::StreamEstimate;
        }

        if tokens == 0 || seconds <= 0.0 {
            return "TPS: -- tok/s".to_string();
        }

        let prefix = if has_estimate { "~" } else { "" };
        format!("TPS: {prefix}{:.1} tok/s", tokens as f64 / seconds)
    }

    fn active_sample(&self, now: Instant) -> Option<TpsSample> {
        let active = self.active.as_ref()?;
        if active.generated_tokens == 0 {
            return None;
        }
        let duration = now.saturating_duration_since(active.started_at);
        if duration < MIN_ACTIVE_SAMPLE_DURATION {
            return None;
        }
        Some(TpsSample {
            generated_tokens: active.generated_tokens,
            duration,
            source: active.source,
        })
    }
}

fn provider_tokens_for_current_turn(info: &TokenUsageInfo) -> u64 {
    generated_tokens_for_usage(&info.last_token_usage)
}

fn generated_tokens_for_usage(usage: &TokenUsage) -> u64 {
    u64::try_from(
        usage
            .output_tokens
            .max(0)
            .saturating_add(usage.reasoning_output_tokens.max(0)),
    )
    .unwrap_or_default()
}

fn estimate_tokens_from_chars(chars: usize) -> u64 {
    if chars == 0 {
        return 0;
    }
    (chars as f64 / STREAM_ESTIMATE_CHARS_PER_TOKEN).ceil() as u64
}

fn positive_duration(
    duration_ms: Option<i64>,
    now: Instant,
    started_at: Instant,
) -> Option<Duration> {
    if let Some(duration_ms) = duration_ms.and_then(|duration_ms| u64::try_from(duration_ms).ok())
        && duration_ms > 0
    {
        return Some(Duration::from_millis(duration_ms));
    }

    let elapsed = now.saturating_duration_since(started_at);
    (!elapsed.is_zero()).then_some(elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(output_tokens: i64, reasoning_output_tokens: i64) -> TokenUsage {
        TokenUsage {
            output_tokens,
            reasoning_output_tokens,
            total_tokens: output_tokens.saturating_add(reasoning_output_tokens),
            ..TokenUsage::default()
        }
    }

    fn usage_info(last: TokenUsage, total: TokenUsage) -> TokenUsageInfo {
        TokenUsageInfo {
            total_token_usage: total,
            last_token_usage: last,
            model_context_window: Some(128_000),
        }
    }

    #[test]
    fn label_is_placeholder_before_samples() {
        assert_eq!(
            TpsEstimator::default().label(Instant::now()),
            "TPS: -- tok/s"
        );
    }

    #[test]
    fn stream_estimate_renders_approximation_marker() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();
        estimator.start_turn(start, None);
        assert!(estimator.record_stream_delta("12345678901234567890"));

        assert_eq!(
            estimator.label(start + Duration::from_secs(1)),
            "TPS: ~5.0 tok/s"
        );
    }

    #[test]
    fn provider_usage_renders_without_approximation_marker() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();
        estimator.start_turn(start, None);
        assert!(estimator.record_provider_usage(&usage_info(usage(20, 5), usage(20, 5))));
        assert!(estimator.complete_turn(Some(1_000), start + Duration::from_secs(1)));

        assert_eq!(
            estimator.label(start + Duration::from_secs(1)),
            "TPS: 25.0 tok/s"
        );
    }

    #[test]
    fn provider_usage_overrides_stream_estimate_for_active_turn() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();
        estimator.start_turn(start, None);
        assert!(estimator.record_stream_delta("12345678901234567890"));
        assert!(estimator.record_provider_usage(&usage_info(usage(8, 4), usage(8, 4))));

        assert_eq!(
            estimator.label(start + Duration::from_secs(2)),
            "TPS: 6.0 tok/s"
        );
    }

    #[test]
    fn provider_usage_falls_back_to_cumulative_delta_for_active_turn() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();
        estimator.start_turn(start, Some(&usage(100, 0)));
        assert!(estimator.record_provider_usage(&usage_info(usage(0, 0), usage(125, 0))));
        assert!(estimator.complete_turn(Some(5_000), start + Duration::from_secs(5)));

        assert_eq!(
            estimator.label(start + Duration::from_secs(5)),
            "TPS: 5.0 tok/s"
        );
    }

    #[test]
    fn late_provider_usage_records_completed_zero_stream_turn() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();
        estimator.start_turn(start, Some(&usage(100, 0)));
        assert!(!estimator.complete_turn(Some(2_000), start + Duration::from_secs(2)));
        assert_eq!(
            estimator.label(start + Duration::from_secs(2)),
            "TPS: -- tok/s"
        );

        assert!(estimator.record_provider_usage(&usage_info(usage(0, 0), usage(130, 0))));

        assert_eq!(
            estimator.label(start + Duration::from_secs(2)),
            "TPS: 15.0 tok/s"
        );
    }

    #[test]
    fn weighted_window_is_not_unweighted_mean() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();

        estimator.start_turn(start, None);
        assert!(estimator.record_provider_usage(&usage_info(usage(100, 0), usage(100, 0))));
        assert!(estimator.complete_turn(Some(10_000), start + Duration::from_secs(10)));

        estimator.start_turn(start + Duration::from_secs(10), Some(&usage(100, 0)));
        assert!(estimator.record_provider_usage(&usage_info(usage(10, 0), usage(110, 0))));
        assert!(estimator.complete_turn(Some(1_000), start + Duration::from_secs(11)));

        assert_eq!(
            estimator.label(start + Duration::from_secs(11)),
            "TPS: 10.0 tok/s"
        );
    }

    #[test]
    fn keeps_last_10_completed_samples_plus_active_sample() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();

        for i in 0..11 {
            let turn_start = start + Duration::from_secs(i);
            estimator.start_turn(turn_start, None);
            assert!(estimator.record_provider_usage(&usage_info(
                usage(10 + i as i64, 0),
                usage(10 + i as i64, 0)
            )));
            assert!(estimator.complete_turn(Some(1_000), turn_start + Duration::from_secs(1)));
        }

        estimator.start_turn(start + Duration::from_secs(20), None);
        assert!(estimator.record_stream_delta("12345678901234567890"));

        assert_eq!(
            estimator.label(start + Duration::from_secs(21)),
            "TPS: ~14.5 tok/s"
        );
    }

    #[test]
    fn positive_tokens_zero_duration_renders_placeholder() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();
        estimator.start_turn(start, None);
        assert!(estimator.record_provider_usage(&usage_info(usage(10, 0), usage(10, 0))));

        assert_eq!(estimator.label(start), "TPS: -- tok/s");
    }

    #[test]
    fn cancel_discards_unfinished_active_sample() {
        let start = Instant::now();
        let mut estimator = TpsEstimator::default();
        estimator.start_turn(start, None);
        assert!(estimator.record_stream_delta("12345678901234567890"));
        estimator.cancel_active();

        assert_eq!(
            estimator.label(start + Duration::from_secs(1)),
            "TPS: -- tok/s"
        );
    }
}
