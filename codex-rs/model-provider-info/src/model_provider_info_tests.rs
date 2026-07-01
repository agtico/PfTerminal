use super::*;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::AbsolutePathBufGuard;
use pretty_assertions::assert_eq;
use std::num::NonZeroU64;
use tempfile::tempdir;

#[test]
fn test_deserialize_ollama_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Ollama"
base_url = "http://localhost:11434/v1"
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Ollama".into(),
        base_url: Some("http://localhost:11434/v1".into()),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_azure_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Azure"
base_url = "https://xxxxx.openai.azure.com/openai"
env_key = "AZURE_OPENAI_API_KEY"
query_params = { api-version = "2025-04-01-preview" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://xxxxx.openai.azure.com/openai".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: Some(maplit::hashmap! {
            "api-version".to_string() => "2025-04-01-preview".to_string(),
        }),
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_example_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Example"
base_url = "https://example.com"
env_key = "API_KEY"
http_headers = { "X-Example-Header" = "example-value" }
env_http_headers = { "X-Example-Env-Header" = "EXAMPLE_ENV_VAR" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: Some(maplit::hashmap! {
            "X-Example-Header".to_string() => "example-value".to_string(),
        }),
        env_http_headers: Some(maplit::hashmap! {
            "X-Example-Env-Header".to_string() => "EXAMPLE_ENV_VAR".to_string(),
        }),
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_chat_wire_api() {
    let provider_toml = r#"
name = "OpenAI using Chat Completions"
base_url = "https://api.openai.com/v1"
env_key = "OPENAI_API_KEY"
wire_api = "chat"
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.wire_api, WireApi::Chat);
}

#[test]
fn test_deserialize_anthropic_wire_api() {
    let provider_toml = r#"
name = "Anthropic-compatible"
base_url = "https://api.example.com/v1"
env_key = "ANTHROPIC_API_KEY"
wire_api = "anthropic"
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.wire_api, WireApi::Anthropic);
}

#[test]
fn test_deserialize_websocket_connect_timeout() {
    let provider_toml = r#"
name = "OpenAI"
base_url = "https://api.openai.com/v1"
websocket_connect_timeout_ms = 15000
supports_websockets = true
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.websocket_connect_timeout_ms, Some(15_000));
}

#[test]
fn test_supports_remote_compaction_for_openai() {
    let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);

    assert!(provider.supports_remote_compaction());
}

#[test]
fn openai_provider_uses_codex_compat_version_header() {
    let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);

    assert_eq!(
        provider
            .http_headers
            .as_ref()
            .and_then(|headers| headers.get("version"))
            .map(String::as_str),
        Some(OPENAI_CODEX_COMPAT_VERSION)
    );
}

#[test]
fn test_personal_access_token_uses_chatgpt_codex_base_url() {
    let api_provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        .to_api_provider(Some(AuthMode::PersonalAccessToken))
        .expect("OpenAI provider should build API provider");

    assert_eq!(api_provider.base_url, CHATGPT_CODEX_BASE_URL);
}

#[test]
fn test_supports_remote_compaction_for_azure_name() {
    let provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://example.com/openai".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    assert!(provider.supports_remote_compaction());
}

#[test]
fn test_supports_remote_compaction_for_non_openai_non_azure_provider() {
    let provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com/v1".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    assert!(!provider.supports_remote_compaction());
}

#[test]
fn test_deserialize_provider_auth_config_defaults() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
args = ["--format=text"]
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    assert_eq!(
        provider.auth,
        Some(ModelProviderAuthInfo {
            command: "./scripts/print-token".to_string(),
            args: vec!["--format=text".to_string()],
            timeout_ms: NonZeroU64::new(5_000).unwrap(),
            refresh_interval_ms: 300_000,
            cwd: AbsolutePathBuf::resolve_path_against_base(".", base_dir.path()),
        })
    );
}

#[test]
fn test_deserialize_provider_aws_config() {
    let provider_toml = r#"
name = "Amazon Bedrock"
base_url = "https://bedrock.example.com/v1"

[aws]
profile = "codex-bedrock"
region = "us-west-2"
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();

    assert_eq!(
        provider.aws,
        Some(ModelProviderAwsAuthInfo {
            profile: Some("codex-bedrock".to_string()),
            region: Some("us-west-2".to_string()),
        })
    );
}

#[test]
fn test_create_amazon_bedrock_provider() {
    assert_eq!(
        ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
        ModelProviderInfo {
            name: "Amazon Bedrock".to_string(),
            base_url: Some("https://bedrock-mantle.us-east-1.api.aws/openai/v1".to_string()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            }),
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(maplit::hashmap! {
                AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER.to_string() =>
                    AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE.to_string(),
            }),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    );
}

#[test]
fn test_create_ambient_provider() {
    assert_eq!(
        ModelProviderInfo::create_ambient_provider(),
        ModelProviderInfo {
            name: "Ambient".to_string(),
            base_url: Some(AMBIENT_BASE_URL.to_string()),
            env_key: Some(AMBIENT_API_KEY_ENV_VAR.to_string()),
            env_key_instructions: Some(provider_api_key_vault_instructions()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    );
    assert_eq!(AMBIENT_DEFAULT_MODEL, "z-ai/glm-5.2");
    assert_eq!(AMBIENT_LEGACY_GLM_5_2_FP8_MODEL, "zai-org/GLM-5.2-FP8");
    assert_eq!(AMBIENT_KIMI_K2_7_CODE_MODEL, "moonshotai/kimi-k2.7-code");
}

#[test]
fn test_create_zai_provider() {
    assert_eq!(
        ModelProviderInfo::create_zai_provider(),
        ModelProviderInfo {
            name: "Z.AI".to_string(),
            base_url: Some(ZAI_BASE_URL.to_string()),
            env_key: Some(ZAI_API_KEY_ENV_VAR.to_string()),
            env_key_instructions: Some(provider_api_key_vault_instructions()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    );
    assert_eq!(ZAI_DEFAULT_MODEL, "glm-5.2");
}

#[test]
fn test_create_anthropic_provider() {
    assert_eq!(
        ModelProviderInfo::create_anthropic_provider(),
        ModelProviderInfo {
            name: "Anthropic".to_string(),
            base_url: Some(ANTHROPIC_BASE_URL.to_string()),
            env_key: Some(ANTHROPIC_API_KEY_ENV_VAR.to_string()),
            env_key_instructions: Some(provider_api_key_vault_instructions()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Anthropic,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    );
    assert_eq!(ANTHROPIC_DEFAULT_MODEL, "claude-opus-4-8");
}

#[test]
fn test_create_claude_plan_provider() {
    assert_eq!(
        ModelProviderInfo::create_claude_plan_provider(),
        ModelProviderInfo {
            name: "Claude Plan".to_string(),
            base_url: Some(ANTHROPIC_BASE_URL.to_string()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: Some(ModelProviderAuthInfo {
                command: "pfterminal".to_string(),
                args: vec!["internal-claude-oauth-token".to_string()],
                timeout_ms: NonZeroU64::new(5_000).expect("timeout should be non-zero"),
                refresh_interval_ms: 60_000,
                cwd: AbsolutePathBuf::from_absolute_path_checked("/")
                    .expect("root path should be absolute"),
            }),
            aws: None,
            wire_api: WireApi::Anthropic,
            query_params: None,
            http_headers: Some(maplit::hashmap! {
                "anthropic-beta".to_string() => "claude-code-20250219,oauth-2025-04-20".to_string(),
            }),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    );
    assert_eq!(CLAUDE_PLAN_MODEL, "claude-opus-4-8-plan");
    assert_eq!(CLAUDE_PLAN_UPSTREAM_MODEL, ANTHROPIC_DEFAULT_MODEL);
    assert_eq!(CLAUDE_FABLE_PLAN_MODEL, "claude-fable-5-plan");
    assert_eq!(CLAUDE_FABLE_PLAN_UPSTREAM_MODEL, "claude-fable-5");
    assert_eq!(
        claude_plan_upstream_model(CLAUDE_PLAN_MODEL),
        Some(CLAUDE_PLAN_UPSTREAM_MODEL)
    );
    assert_eq!(
        claude_plan_upstream_model(CLAUDE_FABLE_PLAN_MODEL),
        Some(CLAUDE_FABLE_PLAN_UPSTREAM_MODEL)
    );
    assert_eq!(claude_plan_upstream_model("claude-fable-5"), None);
}

#[test]
fn test_amazon_bedrock_provider_adds_mantle_client_agent_header() {
    let api_provider = ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None)
        .to_api_provider(/*auth_mode*/ None)
        .expect("Amazon Bedrock provider should build API provider");

    assert_eq!(
        api_provider
            .headers
            .get(AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE)
    );
}

#[test]
fn test_built_in_model_providers_include_amazon_bedrock() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers
            .get(AMAZON_BEDROCK_PROVIDER_ID)
            .map(ModelProviderInfo::is_amazon_bedrock),
        Some(true)
    );
}

#[test]
fn test_built_in_model_providers_include_ambient() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers
            .get(AMBIENT_PROVIDER_ID)
            .map(ModelProviderInfo::is_ambient),
        Some(true)
    );
}

#[test]
fn test_built_in_model_providers_include_anthropic() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let anthropic = providers
        .get(ANTHROPIC_PROVIDER_ID)
        .expect("Anthropic provider should be built in");
    assert!(anthropic.is_anthropic());
    assert_eq!(anthropic.base_url.as_deref(), Some(ANTHROPIC_BASE_URL));
    assert_eq!(
        anthropic.env_key.as_deref(),
        Some(ANTHROPIC_API_KEY_ENV_VAR)
    );
    assert_eq!(anthropic.wire_api, WireApi::Anthropic);
    assert_eq!(anthropic.api_key_header_name(), Some("x-api-key"));
    assert!(!anthropic.requires_openai_auth);
}

#[test]
fn test_built_in_model_providers_include_claude_plan() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let claude_plan = providers
        .get(CLAUDE_PLAN_PROVIDER_ID)
        .expect("Claude Plan provider should be built in");
    assert!(claude_plan.is_claude_plan());
    assert_eq!(claude_plan.base_url.as_deref(), Some(ANTHROPIC_BASE_URL));
    assert!(claude_plan.env_key.is_none());
    assert!(claude_plan.auth.is_some());
    assert_eq!(claude_plan.wire_api, WireApi::Anthropic);
    assert_eq!(
        claude_plan
            .http_headers
            .as_ref()
            .and_then(|headers| headers.get("anthropic-beta"))
            .map(String::as_str),
        Some("claude-code-20250219,oauth-2025-04-20")
    );
    assert!(!claude_plan.requires_openai_auth);
}

#[test]
fn test_built_in_model_providers_include_zai() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers
            .get(ZAI_PROVIDER_ID)
            .map(ModelProviderInfo::is_zai),
        Some(true)
    );
}

#[test]
fn test_built_in_model_providers_include_zai_anthropic() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let zai_anthropic = providers
        .get(ZAI_ANTHROPIC_PROVIDER_ID)
        .expect("Z.AI Anthropic provider should be built in");
    assert_eq!(
        zai_anthropic.base_url.as_deref(),
        Some(ZAI_ANTHROPIC_BASE_URL)
    );
    assert_eq!(zai_anthropic.env_key.as_deref(), Some(ZAI_API_KEY_ENV_VAR));
    assert_eq!(zai_anthropic.wire_api, WireApi::Anthropic);
    assert!(!zai_anthropic.requires_openai_auth);
}

#[test]
fn test_built_in_model_providers_include_openrouter() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let openrouter = providers
        .get(OPENROUTER_PROVIDER_ID)
        .expect("OpenRouter provider should be built in");
    assert!(openrouter.is_openrouter());
    assert_eq!(openrouter.base_url.as_deref(), Some(OPENROUTER_BASE_URL));
    assert_eq!(
        openrouter.env_key.as_deref(),
        Some(OPENROUTER_API_KEY_ENV_VAR)
    );
    assert_eq!(openrouter.wire_api, WireApi::Chat);
    assert!(!openrouter.requires_openai_auth);
}

#[test]
fn test_built_in_model_providers_include_openrouter_anthropic() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let openrouter_anthropic = providers
        .get(OPENROUTER_ANTHROPIC_PROVIDER_ID)
        .expect("OpenRouter Anthropic provider should be built in");
    assert_eq!(
        openrouter_anthropic.base_url.as_deref(),
        Some(OPENROUTER_BASE_URL)
    );
    assert_eq!(
        openrouter_anthropic.env_key.as_deref(),
        Some(OPENROUTER_API_KEY_ENV_VAR)
    );
    assert_eq!(openrouter_anthropic.wire_api, WireApi::Anthropic);
    assert!(!openrouter_anthropic.requires_openai_auth);
}

#[test]
fn test_built_in_model_providers_include_baseten() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let baseten = providers
        .get(BASETEN_PROVIDER_ID)
        .expect("Baseten provider should be built in");
    assert!(baseten.is_baseten());
    assert_eq!(baseten.base_url.as_deref(), Some(BASETEN_BASE_URL));
    assert_eq!(baseten.env_key.as_deref(), Some(BASETEN_API_KEY_ENV_VAR));
    assert_eq!(baseten.wire_api, WireApi::Chat);
    assert!(!baseten.requires_openai_auth);
}

#[test]
fn test_built_in_model_providers_include_baseten_anthropic() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let baseten_anthropic = providers
        .get(BASETEN_ANTHROPIC_PROVIDER_ID)
        .expect("Baseten Anthropic provider should be built in");
    assert_eq!(
        baseten_anthropic.base_url.as_deref(),
        Some(BASETEN_BASE_URL)
    );
    assert_eq!(
        baseten_anthropic.env_key.as_deref(),
        Some(BASETEN_API_KEY_ENV_VAR)
    );
    assert_eq!(baseten_anthropic.wire_api, WireApi::Anthropic);
    assert!(!baseten_anthropic.requires_openai_auth);
}

#[test]
fn test_built_in_model_providers_include_vercel() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let vercel = providers
        .get(VERCEL_PROVIDER_ID)
        .expect("Vercel provider should be built in");
    assert!(vercel.is_vercel());
    assert_eq!(vercel.base_url.as_deref(), Some(VERCEL_BASE_URL));
    assert_eq!(vercel.env_key.as_deref(), Some(VERCEL_API_KEY_ENV_VAR));
    assert_eq!(vercel.wire_api, WireApi::Responses);
    assert!(!vercel.requires_openai_auth);
    assert_eq!(VERCEL_DEFAULT_MODEL, "zai/glm-5.2");
    assert_eq!(VERCEL_GLM_5_2_FAST_MODEL, "zai/glm-5.2-fast");
}

#[test]
fn test_built_in_model_providers_include_vercel_anthropic() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    for provider_id in [
        VERCEL_ANTHROPIC_PROVIDER_ID,
        VERCEL_ANTHROPIC_FAST_PROVIDER_ID,
    ] {
        let vercel_anthropic = providers
            .get(provider_id)
            .expect("Vercel Anthropic provider should be built in");
        assert_eq!(vercel_anthropic.base_url.as_deref(), Some(VERCEL_BASE_URL));
        assert_eq!(
            vercel_anthropic.env_key.as_deref(),
            Some(VERCEL_API_KEY_ENV_VAR)
        );
        assert_eq!(vercel_anthropic.wire_api, WireApi::Anthropic);
        assert!(!vercel_anthropic.requires_openai_auth);
    }
}

#[test]
fn test_merge_configured_model_providers_adds_custom_provider() {
    let custom_provider = ModelProviderInfo {
        name: "Custom".to_string(),
        base_url: Some("https://example.com/v1".to_string()),
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([("custom".to_string(), custom_provider.clone())]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected.insert("custom".to_string(), custom_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_applies_amazon_bedrock_profile_override() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("codex-bedrock".to_string()),
                region: Some("us-west-2".to_string()),
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected
        .get_mut(AMAZON_BEDROCK_PROVIDER_ID)
        .expect("Amazon Bedrock provider should be built in")
        .aws = Some(ModelProviderAwsAuthInfo {
        profile: Some("codex-bedrock".to_string()),
        region: Some("us-west-2".to_string()),
    });

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_rejects_amazon_bedrock_non_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            name: "Custom Bedrock".to_string(),
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("codex-bedrock".to_string()),
                region: None,
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Err(
            "model_providers.amazon-bedrock only supports changing `aws.profile` and `aws.region`; other non-default provider fields are not supported"
                .to_string()
        )
    );
}

#[test]
fn test_merge_configured_model_providers_allows_amazon_bedrock_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            }),
            wire_api: WireApi::Responses,
            ..ModelProviderInfo::default()
        },
    )]);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(built_in_model_providers(/*openai_base_url*/ None))
    );
}

#[test]
fn test_validate_provider_aws_rejects_conflicting_auth() {
    let provider = ModelProviderInfo {
        aws: Some(ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
        }),
        env_key: Some("AWS_BEARER_TOKEN_BEDROCK".to_string()),
        supports_websockets: false,
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with env_key, requires_openai_auth".to_string())
    );
}

#[test]
fn test_validate_provider_aws_rejects_websockets() {
    let provider = ModelProviderInfo {
        aws: Some(ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
        }),
        requires_openai_auth: false,
        supports_websockets: true,
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with supports_websockets".to_string())
    );
}

#[test]
fn test_deserialize_provider_auth_config_allows_zero_refresh_interval() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
refresh_interval_ms = 0
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    let auth = provider.auth.expect("auth config should deserialize");
    assert_eq!(auth.refresh_interval_ms, 0);
    assert_eq!(auth.refresh_interval(), None);
}
