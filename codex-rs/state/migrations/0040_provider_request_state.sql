CREATE TABLE provider_request_state (
    provider_id TEXT NOT NULL,
    model TEXT NOT NULL,
    key_fingerprint TEXT NOT NULL,
    cooldown_until_ms INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_until_ms INTEGER NOT NULL DEFAULT 0,
    last_status INTEGER,
    last_request_id TEXT,
    last_input_tokens INTEGER NOT NULL DEFAULT 0,
    last_cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    last_request_bytes INTEGER NOT NULL DEFAULT 0,
    last_thread_id TEXT,
    last_turn_id TEXT,
    consecutive_429_count INTEGER NOT NULL DEFAULT 0,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (provider_id, model, key_fingerprint)
);

CREATE INDEX provider_request_state_cooldown_idx
    ON provider_request_state(cooldown_until_ms);

CREATE INDEX provider_request_state_lease_idx
    ON provider_request_state(lease_until_ms);
