use super::*;

const RATE_LIMIT_STATUS: i64 = 429;
const DEFAULT_COOLDOWN_CAP_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRequestKey {
    pub provider_id: String,
    pub model: String,
    pub key_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRequestPreflight {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub request_bytes: i64,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRequestLease {
    pub key: ProviderRequestKey,
    pub owner: String,
    pub lease_until_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRequestBlock {
    pub reason: ProviderRequestBlockReason,
    pub until_ms: i64,
    pub remaining_ms: i64,
    pub lease_owner: Option<String>,
    pub last_status: Option<i64>,
    pub last_request_id: Option<String>,
    pub last_input_tokens: i64,
    pub last_cached_input_tokens: i64,
    pub last_request_bytes: i64,
    pub last_provider_input_tokens: i64,
    pub last_provider_cached_input_tokens: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderRequestBlockReason {
    Cooldown,
    Lease,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderRequestLeaseDecision {
    Acquired(ProviderRequestLease),
    Blocked(ProviderRequestBlock),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderRequestResult {
    Success {
        input_tokens: Option<i64>,
        cached_input_tokens: Option<i64>,
    },
    Failed {
        status: Option<u16>,
        request_id: Option<String>,
        retry_after_ms: Option<i64>,
    },
}

impl StateRuntime {
    pub async fn check_provider_request_cooldown(
        &self,
        key: &ProviderRequestKey,
        preflight: &ProviderRequestPreflight,
        now_ms: i64,
    ) -> anyhow::Result<Option<ProviderRequestBlock>> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
INSERT INTO provider_request_state (
    provider_id,
    model,
    key_fingerprint,
    last_input_tokens,
    last_cached_input_tokens,
    last_request_bytes,
    last_thread_id,
    last_turn_id,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(provider_id, model, key_fingerprint) DO UPDATE SET
    last_input_tokens = excluded.last_input_tokens,
    last_cached_input_tokens = excluded.last_cached_input_tokens,
    last_request_bytes = excluded.last_request_bytes,
    last_thread_id = excluded.last_thread_id,
    last_turn_id = excluded.last_turn_id,
    updated_at_ms = excluded.updated_at_ms
            "#,
        )
        .bind(key.provider_id.as_str())
        .bind(key.model.as_str())
        .bind(key.key_fingerprint.as_str())
        .bind(preflight.input_tokens.max(0))
        .bind(preflight.cached_input_tokens.max(0))
        .bind(preflight.request_bytes.max(0))
        .bind(preflight.thread_id.as_deref())
        .bind(preflight.turn_id.as_deref())
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;

        let row = sqlx::query(
            r#"
SELECT
    cooldown_until_ms,
    lease_owner,
    lease_until_ms,
    last_status,
    last_request_id,
    last_input_tokens,
    last_cached_input_tokens,
    last_request_bytes,
    last_provider_input_tokens,
    last_provider_cached_input_tokens
FROM provider_request_state
WHERE provider_id = ? AND model = ? AND key_fingerprint = ?
            "#,
        )
        .bind(key.provider_id.as_str())
        .bind(key.model.as_str())
        .bind(key.key_fingerprint.as_str())
        .fetch_one(&mut *tx)
        .await?;

        let cooldown_until_ms: i64 = row.try_get("cooldown_until_ms")?;
        let block = if cooldown_until_ms > now_ms {
            Some(block_from_row(
                &row,
                ProviderRequestBlockReason::Cooldown,
                cooldown_until_ms,
                now_ms,
            )?)
        } else {
            None
        };

        tx.commit().await?;
        Ok(block)
    }

    pub async fn try_acquire_provider_request_lease(
        &self,
        key: &ProviderRequestKey,
        preflight: &ProviderRequestPreflight,
        owner: &str,
        lease_ttl_ms: i64,
        now_ms: i64,
    ) -> anyhow::Result<ProviderRequestLeaseDecision> {
        let lease_until_ms = now_ms.saturating_add(lease_ttl_ms.max(1));
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
INSERT INTO provider_request_state (
    provider_id,
    model,
    key_fingerprint,
    last_input_tokens,
    last_cached_input_tokens,
    last_request_bytes,
    last_thread_id,
    last_turn_id,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(provider_id, model, key_fingerprint) DO UPDATE SET
    last_input_tokens = excluded.last_input_tokens,
    last_cached_input_tokens = excluded.last_cached_input_tokens,
    last_request_bytes = excluded.last_request_bytes,
    last_thread_id = excluded.last_thread_id,
    last_turn_id = excluded.last_turn_id,
    updated_at_ms = excluded.updated_at_ms
            "#,
        )
        .bind(key.provider_id.as_str())
        .bind(key.model.as_str())
        .bind(key.key_fingerprint.as_str())
        .bind(preflight.input_tokens.max(0))
        .bind(preflight.cached_input_tokens.max(0))
        .bind(preflight.request_bytes.max(0))
        .bind(preflight.thread_id.as_deref())
        .bind(preflight.turn_id.as_deref())
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;

        let row = sqlx::query(
            r#"
SELECT
    cooldown_until_ms,
    lease_owner,
    lease_until_ms,
    last_status,
    last_request_id,
    last_input_tokens,
    last_cached_input_tokens,
    last_request_bytes,
    last_provider_input_tokens,
    last_provider_cached_input_tokens
FROM provider_request_state
WHERE provider_id = ? AND model = ? AND key_fingerprint = ?
            "#,
        )
        .bind(key.provider_id.as_str())
        .bind(key.model.as_str())
        .bind(key.key_fingerprint.as_str())
        .fetch_one(&mut *tx)
        .await?;

        let cooldown_until_ms: i64 = row.try_get("cooldown_until_ms")?;
        if cooldown_until_ms > now_ms {
            let block = block_from_row(
                &row,
                ProviderRequestBlockReason::Cooldown,
                cooldown_until_ms,
                now_ms,
            )?;
            tx.commit().await?;
            return Ok(ProviderRequestLeaseDecision::Blocked(block));
        }

        let lease_until_existing_ms: i64 = row.try_get("lease_until_ms")?;
        if lease_until_existing_ms > now_ms {
            let block = block_from_row(
                &row,
                ProviderRequestBlockReason::Lease,
                lease_until_existing_ms,
                now_ms,
            )?;
            tx.commit().await?;
            return Ok(ProviderRequestLeaseDecision::Blocked(block));
        }

        sqlx::query(
            r#"
UPDATE provider_request_state
SET lease_owner = ?,
    lease_until_ms = ?,
    last_status = NULL,
    last_request_id = NULL,
    last_provider_input_tokens = 0,
    last_provider_cached_input_tokens = 0,
    updated_at_ms = ?
WHERE provider_id = ? AND model = ? AND key_fingerprint = ?
            "#,
        )
        .bind(owner)
        .bind(lease_until_ms)
        .bind(now_ms)
        .bind(key.provider_id.as_str())
        .bind(key.model.as_str())
        .bind(key.key_fingerprint.as_str())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(ProviderRequestLeaseDecision::Acquired(
            ProviderRequestLease {
                key: key.clone(),
                owner: owner.to_string(),
                lease_until_ms,
            },
        ))
    }

    pub async fn record_provider_request_result(
        &self,
        lease: &ProviderRequestLease,
        result: ProviderRequestResult,
        now_ms: i64,
    ) -> anyhow::Result<u64> {
        let rows_affected = match result {
            ProviderRequestResult::Success {
                input_tokens,
                cached_input_tokens,
            } => sqlx::query(
                r#"
UPDATE provider_request_state
SET lease_owner = NULL,
    lease_until_ms = 0,
    cooldown_until_ms = 0,
    last_status = 200,
    last_request_id = NULL,
    last_provider_input_tokens = COALESCE(?, last_provider_input_tokens),
    last_provider_cached_input_tokens = COALESCE(?, last_provider_cached_input_tokens),
    consecutive_429_count = 0,
    updated_at_ms = ?
WHERE provider_id = ? AND model = ? AND key_fingerprint = ? AND lease_owner = ?
                    "#,
            )
            .bind(input_tokens.map(|tokens| tokens.max(0)))
            .bind(cached_input_tokens.map(|tokens| tokens.max(0)))
            .bind(now_ms)
            .bind(lease.key.provider_id.as_str())
            .bind(lease.key.model.as_str())
            .bind(lease.key.key_fingerprint.as_str())
            .bind(lease.owner.as_str())
            .execute(self.pool.as_ref())
            .await?
            .rows_affected(),
            ProviderRequestResult::Failed {
                status,
                request_id,
                retry_after_ms,
            } => {
                let status_i64 = status.map(i64::from);
                let mut tx = self.pool.begin().await?;
                let existing_count = sqlx::query_scalar::<_, i64>(
                    r#"
SELECT consecutive_429_count
FROM provider_request_state
WHERE provider_id = ? AND model = ? AND key_fingerprint = ?
                    "#,
                )
                .bind(lease.key.provider_id.as_str())
                .bind(lease.key.model.as_str())
                .bind(lease.key.key_fingerprint.as_str())
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(0);

                let is_rate_limit = status_i64 == Some(RATE_LIMIT_STATUS);
                let consecutive_429_count = if is_rate_limit {
                    existing_count.saturating_add(1)
                } else {
                    0
                };
                let cooldown_until_ms = if is_rate_limit {
                    now_ms.saturating_add(
                        retry_after_ms
                            .filter(|ms| *ms > 0)
                            .unwrap_or_else(|| default_cooldown_ms(consecutive_429_count)),
                    )
                } else {
                    0
                };

                let rows_affected = sqlx::query(
                    r#"
UPDATE provider_request_state
SET lease_owner = NULL,
    lease_until_ms = 0,
    cooldown_until_ms = ?,
    last_status = ?,
    last_request_id = ?,
    consecutive_429_count = ?,
    updated_at_ms = ?
WHERE provider_id = ? AND model = ? AND key_fingerprint = ? AND lease_owner = ?
                    "#,
                )
                .bind(cooldown_until_ms)
                .bind(status_i64)
                .bind(request_id.as_deref())
                .bind(consecutive_429_count)
                .bind(now_ms)
                .bind(lease.key.provider_id.as_str())
                .bind(lease.key.model.as_str())
                .bind(lease.key.key_fingerprint.as_str())
                .bind(lease.owner.as_str())
                .execute(&mut *tx)
                .await?
                .rows_affected();

                tx.commit().await?;
                rows_affected
            }
        };

        Ok(rows_affected)
    }

    pub async fn release_provider_request_lease(
        &self,
        lease: &ProviderRequestLease,
        now_ms: i64,
    ) -> anyhow::Result<u64> {
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_request_state
SET lease_owner = NULL,
    lease_until_ms = 0,
    updated_at_ms = ?
WHERE provider_id = ? AND model = ? AND key_fingerprint = ? AND lease_owner = ?
            "#,
        )
        .bind(now_ms)
        .bind(lease.key.provider_id.as_str())
        .bind(lease.key.model.as_str())
        .bind(lease.key.key_fingerprint.as_str())
        .bind(lease.owner.as_str())
        .execute(self.pool.as_ref())
        .await?
        .rows_affected();
        Ok(rows_affected)
    }
}

fn block_from_row(
    row: &sqlx::sqlite::SqliteRow,
    reason: ProviderRequestBlockReason,
    until_ms: i64,
    now_ms: i64,
) -> anyhow::Result<ProviderRequestBlock> {
    Ok(ProviderRequestBlock {
        reason,
        until_ms,
        remaining_ms: until_ms.saturating_sub(now_ms),
        lease_owner: row.try_get("lease_owner")?,
        last_status: row.try_get("last_status")?,
        last_request_id: row.try_get("last_request_id")?,
        last_input_tokens: row.try_get("last_input_tokens")?,
        last_cached_input_tokens: row.try_get("last_cached_input_tokens")?,
        last_request_bytes: row.try_get("last_request_bytes")?,
        last_provider_input_tokens: row.try_get("last_provider_input_tokens")?,
        last_provider_cached_input_tokens: row.try_get("last_provider_cached_input_tokens")?,
    })
}

fn default_cooldown_ms(consecutive_429_count: i64) -> i64 {
    match consecutive_429_count {
        count if count <= 1 => 30_000,
        2 => 60_000,
        3 => 120_000,
        _ => DEFAULT_COOLDOWN_CAP_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderRequestBlockReason;
    use super::ProviderRequestKey;
    use super::ProviderRequestLeaseDecision;
    use super::ProviderRequestPreflight;
    use super::ProviderRequestResult;
    use super::StateRuntime;
    use super::test_support::unique_temp_dir;

    fn key() -> ProviderRequestKey {
        ProviderRequestKey {
            provider_id: "ambient".to_string(),
            model: "zai-org/GLM-5.2-FP8".to_string(),
            key_fingerprint: "env:AMBIENT_API_KEY:test".to_string(),
        }
    }

    fn preflight() -> ProviderRequestPreflight {
        ProviderRequestPreflight {
            input_tokens: 37_492,
            cached_input_tokens: 20_000,
            request_bytes: 160_000,
            thread_id: Some("thread-a".to_string()),
            turn_id: Some("turn-a".to_string()),
        }
    }

    fn small_preflight() -> ProviderRequestPreflight {
        ProviderRequestPreflight {
            input_tokens: 2_426,
            cached_input_tokens: 576,
            request_bytes: 29_441,
            thread_id: Some("thread-b".to_string()),
            turn_id: Some("turn-b".to_string()),
        }
    }

    #[tokio::test]
    async fn lease_blocks_second_process_until_ttl() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let first = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire first lease");
        assert!(matches!(first, ProviderRequestLeaseDecision::Acquired(_)));

        let second = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 1_500)
            .await
            .expect("acquire second lease");
        match second {
            ProviderRequestLeaseDecision::Blocked(block) => {
                assert_eq!(block.reason, ProviderRequestBlockReason::Lease);
                assert_eq!(block.lease_owner.as_deref(), Some("worker-a"));
                assert_eq!(block.last_input_tokens, 37_492);
                assert_eq!(block.last_request_bytes, 160_000);
            }
            ProviderRequestLeaseDecision::Acquired(_) => panic!("expected lease block"),
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn releasing_lease_allows_next_process() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let ProviderRequestLeaseDecision::Acquired(lease) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire first lease")
        else {
            panic!("expected acquired lease");
        };
        let rows_affected = runtime
            .release_provider_request_lease(&lease, 2_000)
            .await
            .expect("release lease");
        assert_eq!(rows_affected, 1);

        let second = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 2_100)
            .await
            .expect("acquire second lease");
        assert!(matches!(second, ProviderRequestLeaseDecision::Acquired(_)));

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn cooldown_only_check_ignores_active_lease() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let first = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire first lease");
        assert!(matches!(first, ProviderRequestLeaseDecision::Acquired(_)));

        let block = runtime
            .check_provider_request_cooldown(&key(), &small_preflight(), 1_500)
            .await
            .expect("check cooldown");
        assert_eq!(block, None);

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn failed_non_rate_limit_result_clears_lease_without_cooldown() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let ProviderRequestLeaseDecision::Acquired(lease) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire lease")
        else {
            panic!("expected acquired lease");
        };
        let rows_affected = runtime
            .record_provider_request_result(
                &lease,
                ProviderRequestResult::Failed {
                    status: Some(500),
                    request_id: Some("req-500".to_string()),
                    retry_after_ms: Some(60_000),
                },
                2_000,
            )
            .await
            .expect("record non-rate-limit failure");
        assert_eq!(rows_affected, 1);

        let retry = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 2_500)
            .await
            .expect("retry lease");
        assert!(matches!(retry, ProviderRequestLeaseDecision::Acquired(_)));

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn stale_owner_result_or_release_does_not_clear_active_owner() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let ProviderRequestLeaseDecision::Acquired(first) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 1_000, 1_000)
            .await
            .expect("acquire first lease")
        else {
            panic!("expected first lease");
        };
        let ProviderRequestLeaseDecision::Acquired(_second) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 2_500)
            .await
            .expect("acquire second lease")
        else {
            panic!("expected second lease after first expires");
        };

        let result_rows = runtime
            .record_provider_request_result(
                &first,
                ProviderRequestResult::Success {
                    input_tokens: Some(1),
                    cached_input_tokens: Some(1),
                },
                3_000,
            )
            .await
            .expect("record stale result");
        assert_eq!(result_rows, 0);
        let release_rows = runtime
            .release_provider_request_lease(&first, 3_100)
            .await
            .expect("release stale lease");
        assert_eq!(release_rows, 0);

        let blocked = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-c", 10_000, 3_200)
            .await
            .expect("blocked by active second lease");
        match blocked {
            ProviderRequestLeaseDecision::Blocked(block) => {
                assert_eq!(block.reason, ProviderRequestBlockReason::Lease);
                assert_eq!(block.lease_owner.as_deref(), Some("worker-b"));
            }
            ProviderRequestLeaseDecision::Acquired(_) => panic!("expected active lease block"),
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn rate_limit_result_sets_cooldown_and_blocks_retry() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let ProviderRequestLeaseDecision::Acquired(lease) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire lease")
        else {
            panic!("expected acquired lease");
        };
        let rows_affected = runtime
            .record_provider_request_result(
                &lease,
                ProviderRequestResult::Failed {
                    status: Some(429),
                    request_id: Some("req-1".to_string()),
                    retry_after_ms: None,
                },
                2_000,
            )
            .await
            .expect("record 429");
        assert_eq!(rows_affected, 1);

        let retry = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 2_500)
            .await
            .expect("retry lease");
        match retry {
            ProviderRequestLeaseDecision::Blocked(block) => {
                assert_eq!(block.reason, ProviderRequestBlockReason::Cooldown);
                assert_eq!(block.remaining_ms, 29_500);
                assert_eq!(block.last_status, Some(429));
                assert_eq!(block.last_request_id.as_deref(), Some("req-1"));
            }
            ProviderRequestLeaseDecision::Acquired(_) => panic!("expected cooldown block"),
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn success_result_records_provider_reported_cache_usage() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let ProviderRequestLeaseDecision::Acquired(lease) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire lease")
        else {
            panic!("expected acquired lease");
        };
        let rows_affected = runtime
            .record_provider_request_result(
                &lease,
                ProviderRequestResult::Success {
                    input_tokens: Some(17_136),
                    cached_input_tokens: Some(17_088),
                },
                2_000,
            )
            .await
            .expect("record success");
        assert_eq!(rows_affected, 1);

        let ProviderRequestLeaseDecision::Acquired(second) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 2_500)
            .await
            .expect("acquire second lease")
        else {
            panic!("expected second lease");
        };
        runtime
            .record_provider_request_result(
                &second,
                ProviderRequestResult::Failed {
                    status: Some(429),
                    request_id: None,
                    retry_after_ms: None,
                },
                3_000,
            )
            .await
            .expect("record 429");

        let blocked = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-c", 10_000, 3_500)
            .await
            .expect("blocked after cooldown");
        match blocked {
            ProviderRequestLeaseDecision::Blocked(block) => {
                assert_eq!(block.reason, ProviderRequestBlockReason::Cooldown);
                assert_eq!(block.last_input_tokens, 37_492);
                assert_eq!(block.last_cached_input_tokens, 20_000);
                assert_eq!(block.last_provider_input_tokens, 0);
                assert_eq!(block.last_provider_cached_input_tokens, 0);
            }
            ProviderRequestLeaseDecision::Acquired(_) => panic!("expected cooldown block"),
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn acquiring_new_lease_clears_stale_result_fields() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let ProviderRequestLeaseDecision::Acquired(first) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire first")
        else {
            panic!("expected first lease");
        };
        runtime
            .record_provider_request_result(
                &first,
                ProviderRequestResult::Success {
                    input_tokens: Some(88_303),
                    cached_input_tokens: Some(0),
                },
                2_000,
            )
            .await
            .expect("record cold-cache success");

        let ProviderRequestLeaseDecision::Acquired(_second) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 2_500)
            .await
            .expect("acquire second")
        else {
            panic!("expected second lease");
        };

        let blocked = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-c", 10_000, 3_000)
            .await
            .expect("blocked by active second lease");
        match blocked {
            ProviderRequestLeaseDecision::Blocked(block) => {
                assert_eq!(block.reason, ProviderRequestBlockReason::Lease);
                assert_eq!(block.last_status, None);
                assert_eq!(block.last_request_id, None);
                assert_eq!(block.last_provider_input_tokens, 0);
                assert_eq!(block.last_provider_cached_input_tokens, 0);
            }
            ProviderRequestLeaseDecision::Acquired(_) => panic!("expected active lease block"),
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn repeated_rate_limits_back_off_to_sixty_seconds() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let ProviderRequestLeaseDecision::Acquired(first) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire first")
        else {
            panic!("expected first lease");
        };
        runtime
            .record_provider_request_result(
                &first,
                ProviderRequestResult::Failed {
                    status: Some(429),
                    request_id: None,
                    retry_after_ms: None,
                },
                2_000,
            )
            .await
            .expect("record first 429");

        let ProviderRequestLeaseDecision::Acquired(second) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 33_000)
            .await
            .expect("acquire after first cooldown")
        else {
            panic!("expected second lease");
        };
        runtime
            .record_provider_request_result(
                &second,
                ProviderRequestResult::Failed {
                    status: Some(429),
                    request_id: None,
                    retry_after_ms: None,
                },
                34_000,
            )
            .await
            .expect("record second 429");

        let blocked = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-c", 10_000, 35_000)
            .await
            .expect("blocked after second cooldown");
        match blocked {
            ProviderRequestLeaseDecision::Blocked(block) => {
                assert_eq!(block.reason, ProviderRequestBlockReason::Cooldown);
                assert_eq!(block.remaining_ms, 59_000);
            }
            ProviderRequestLeaseDecision::Acquired(_) => panic!("expected cooldown block"),
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn rate_limit_result_uses_retry_after_when_available() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        let ProviderRequestLeaseDecision::Acquired(lease) = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-a", 10_000, 1_000)
            .await
            .expect("acquire lease")
        else {
            panic!("expected acquired lease");
        };
        runtime
            .record_provider_request_result(
                &lease,
                ProviderRequestResult::Failed {
                    status: Some(429),
                    request_id: None,
                    retry_after_ms: Some(12_000),
                },
                2_000,
            )
            .await
            .expect("record 429");

        let retry = runtime
            .try_acquire_provider_request_lease(&key(), &preflight(), "worker-b", 10_000, 2_500)
            .await
            .expect("retry lease");
        match retry {
            ProviderRequestLeaseDecision::Blocked(block) => {
                assert_eq!(block.reason, ProviderRequestBlockReason::Cooldown);
                assert_eq!(block.remaining_ms, 11_500);
            }
            ProviderRequestLeaseDecision::Acquired(_) => panic!("expected cooldown block"),
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }
}
