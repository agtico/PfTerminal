use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;

use codex_api::Provider;
use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::AMBIENT_DEFAULT_MODEL;
use codex_model_provider_info::BASETEN_DEFAULT_MODEL;
use codex_model_provider_info::CLAUDE_PLAN_MODEL;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OPENROUTER_DEFAULT_MODEL;
use codex_model_provider_info::VERCEL_DEFAULT_MODEL;
use codex_model_provider_info::ZAI_DEFAULT_MODEL;
use codex_models_manager::bundled_models_response;
use codex_models_manager::manager::OpenAiModelsManager;
use codex_models_manager::manager::SharedModelsManager;
use codex_models_manager::manager::StaticModelsManager;
use codex_protocol::account::ProviderAccount;
use codex_protocol::openai_models::ModelsResponse;

use crate::amazon_bedrock::AmazonBedrockModelProvider;
use crate::auth::auth_manager_for_provider;
use crate::auth::resolve_provider_auth;
use crate::models_endpoint::OpenAiModelsEndpoint;

/// Optional provider-backed features that Codex may expose at runtime.
///
/// These capabilities are a provider-owned upper bound. Callers can disable
/// more functionality through normal config, but should not expose a feature
/// that the active provider marks unsupported here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub namespace_tools: bool,
    pub image_generation: bool,
    pub web_search: bool,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            namespace_tools: true,
            image_generation: true,
            web_search: true,
        }
    }
}

/// Current app-visible account state for a model provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAccountState {
    pub account: Option<ProviderAccount>,
    pub requires_openai_auth: bool,
}

/// Error returned when a provider cannot construct its app-visible account state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAccountError {
    MissingChatgptAccountDetails,
    UnsupportedBedrockApiKeyAuth,
}

impl fmt::Display for ProviderAccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingChatgptAccountDetails => {
                write!(
                    f,
                    "email and plan type are required for chatgpt authentication"
                )
            }
            Self::UnsupportedBedrockApiKeyAuth => {
                write!(
                    f,
                    "Bedrock API key auth is only supported by the Amazon Bedrock model provider"
                )
            }
        }
    }
}

impl std::error::Error for ProviderAccountError {}

pub type ProviderAccountResult = std::result::Result<ProviderAccountState, ProviderAccountError>;

/// Default model used for automatic approval review when a provider does not
/// require a backend-specific model ID.
pub const DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL: &str = "codex-auto-review";

/// Default model used for memory extraction when a provider does not require a
/// backend-specific model ID.
pub const DEFAULT_MEMORY_EXTRACTION_PREFERRED_MODEL: &str = "gpt-5.4-mini";

/// Default model used for memory consolidation when a provider does not require
/// a backend-specific model ID.
pub const DEFAULT_MEMORY_CONSOLIDATION_PREFERRED_MODEL: &str = "gpt-5.4";

fn bundled_static_model_catalog() -> ModelsResponse {
    bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"))
}

/// Runtime provider abstraction used by model execution.
///
/// Implementations own provider-specific behavior for a model backend. The
/// `ModelProviderInfo` returned by `info` is the serialized/configured provider
/// metadata used by the default OpenAI-compatible implementation.
pub trait ModelProvider: fmt::Debug + Send + Sync {
    /// Returns the configured provider metadata.
    fn info(&self) -> &ModelProviderInfo;

    /// Returns the provider-owned capability upper bounds.
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    /// Returns the preferred model used for automatic approval review.
    ///
    /// Providers that require backend-specific model IDs should override this.
    fn approval_review_preferred_model(&self) -> &'static str {
        DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL
    }

    /// Returns the preferred model used for memory extraction.
    ///
    /// Providers that require backend-specific model IDs should override this.
    fn memory_extraction_preferred_model(&self) -> &'static str {
        DEFAULT_MEMORY_EXTRACTION_PREFERRED_MODEL
    }

    /// Returns the preferred model used for memory consolidation.
    ///
    /// Providers that require backend-specific model IDs should override this.
    fn memory_consolidation_preferred_model(&self) -> &'static str {
        DEFAULT_MEMORY_CONSOLIDATION_PREFERRED_MODEL
    }

    /// Returns whether requests made through this provider should include attestation.
    fn supports_attestation(&self) -> bool {
        false
    }

    /// Returns the provider-scoped auth manager, when this provider uses one.
    ///
    /// TODO(celia-oai): Make auth manager access internal to this crate so callers
    /// resolve provider-specific auth only through `ModelProvider`. We first need
    /// to think through whether Codex should have a unified provider-specific auth
    /// manager throughout the codebase; that is a larger refactor than this change.
    fn auth_manager(&self) -> Option<Arc<AuthManager>>;

    /// Returns the current provider-scoped auth value, if one is configured.
    fn auth(&self) -> ModelProviderFuture<'_, Option<CodexAuth>>;

    /// Returns the current app-visible account state for this provider.
    fn account_state(&self) -> ProviderAccountResult;

    /// Returns provider configuration adapted for the API client.
    fn api_provider(&self) -> ModelProviderFuture<'_, codex_protocol::error::Result<Provider>> {
        Box::pin(async move {
            let auth = self.auth().await;
            self.info()
                .to_api_provider(auth.as_ref().map(CodexAuth::auth_mode))
        })
    }

    /// Returns the provider base URL that will be used at request time.
    fn runtime_base_url(
        &self,
    ) -> ModelProviderFuture<'_, codex_protocol::error::Result<Option<String>>> {
        Box::pin(async { Ok(self.info().base_url.clone()) })
    }

    /// Returns the auth provider used to attach request credentials.
    fn api_auth(
        &self,
    ) -> ModelProviderFuture<'_, codex_protocol::error::Result<SharedAuthProvider>> {
        Box::pin(async move {
            let auth = self.auth().await;
            resolve_provider_auth(auth.as_ref(), self.info())
        })
    }

    /// Creates the model manager implementation appropriate for this provider.
    fn models_manager(
        &self,
        codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager;
}

pub type ModelProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Shared runtime model provider handle.
pub type SharedModelProvider = Arc<dyn ModelProvider>;

/// Creates the default runtime model provider for configured provider metadata.
pub fn create_model_provider(
    provider_info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
) -> SharedModelProvider {
    if provider_info.is_amazon_bedrock() {
        Arc::new(AmazonBedrockModelProvider::new(provider_info, auth_manager))
    } else {
        Arc::new(ConfiguredModelProvider::new(provider_info, auth_manager))
    }
}

/// Runtime model provider backed by configured `ModelProviderInfo`.
#[derive(Clone, Debug)]
struct ConfiguredModelProvider {
    info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
    cached_provider_env_auth: Arc<OnceLock<Option<CodexAuth>>>,
}

impl ConfiguredModelProvider {
    fn new(provider_info: ModelProviderInfo, auth_manager: Option<Arc<AuthManager>>) -> Self {
        let auth_manager = auth_manager_for_provider(auth_manager, &provider_info);
        Self {
            info: provider_info,
            auth_manager,
            cached_provider_env_auth: Arc::new(OnceLock::new()),
        }
    }

    fn provider_env_auth(&self, provider_key_id: &str) -> Option<CodexAuth> {
        if let Ok(api_key) = std::env::var(provider_key_id)
            && !api_key.trim().is_empty()
        {
            return Some(CodexAuth::from_api_key(&api_key));
        }

        self.cached_provider_env_auth
            .get_or_init(|| {
                self.auth_manager
                    .as_ref()
                    .and_then(|auth_manager| auth_manager.provider_api_key(provider_key_id).ok())
                    .flatten()
                    .map(|api_key| CodexAuth::from_api_key(&api_key))
            })
            .clone()
    }
}

impl ModelProvider for ConfiguredModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        if self.info.is_ambient() || self.info.is_baseten() || self.info.is_vercel() {
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
            }
        } else if self.info.is_zai() || self.info.is_openrouter() {
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: true,
            }
        } else {
            ProviderCapabilities::default()
        }
    }

    fn approval_review_preferred_model(&self) -> &'static str {
        if self.info.is_ambient() {
            AMBIENT_DEFAULT_MODEL
        } else if self.info.is_claude_plan() {
            CLAUDE_PLAN_MODEL
        } else if self.info.is_zai() {
            ZAI_DEFAULT_MODEL
        } else if self.info.is_openrouter() {
            OPENROUTER_DEFAULT_MODEL
        } else if self.info.is_baseten() {
            BASETEN_DEFAULT_MODEL
        } else if self.info.is_vercel() {
            VERCEL_DEFAULT_MODEL
        } else {
            DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL
        }
    }

    fn memory_extraction_preferred_model(&self) -> &'static str {
        if self.info.is_ambient() {
            AMBIENT_DEFAULT_MODEL
        } else if self.info.is_claude_plan() {
            CLAUDE_PLAN_MODEL
        } else if self.info.is_zai() {
            ZAI_DEFAULT_MODEL
        } else if self.info.is_openrouter() {
            OPENROUTER_DEFAULT_MODEL
        } else if self.info.is_baseten() {
            BASETEN_DEFAULT_MODEL
        } else if self.info.is_vercel() {
            VERCEL_DEFAULT_MODEL
        } else {
            DEFAULT_MEMORY_EXTRACTION_PREFERRED_MODEL
        }
    }

    fn memory_consolidation_preferred_model(&self) -> &'static str {
        if self.info.is_ambient() {
            AMBIENT_DEFAULT_MODEL
        } else if self.info.is_claude_plan() {
            CLAUDE_PLAN_MODEL
        } else if self.info.is_zai() {
            ZAI_DEFAULT_MODEL
        } else if self.info.is_openrouter() {
            OPENROUTER_DEFAULT_MODEL
        } else if self.info.is_baseten() {
            BASETEN_DEFAULT_MODEL
        } else if self.info.is_vercel() {
            VERCEL_DEFAULT_MODEL
        } else {
            DEFAULT_MEMORY_CONSOLIDATION_PREFERRED_MODEL
        }
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.auth_manager.clone()
    }

    fn supports_attestation(&self) -> bool {
        self.auth_manager
            .as_ref()
            .and_then(|auth_manager| auth_manager.auth_cached())
            .is_some_and(|auth| auth.is_chatgpt_auth())
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<CodexAuth>> {
        Box::pin(async move {
            let auth_manager = self.auth_manager.as_ref()?;

            if let Some(provider_key_id) = self.info.env_key.as_deref() {
                return self.provider_env_auth(provider_key_id);
            }

            auth_manager.auth().await
        })
    }

    fn account_state(&self) -> ProviderAccountResult {
        let account = if let Some(provider_key_id) = self.info.env_key.as_deref() {
            let stored_key = self
                .auth_manager
                .as_ref()
                .and_then(|auth_manager| auth_manager.provider_api_key(provider_key_id).ok())
                .flatten();
            let env_key = self.info.api_key().ok().flatten();
            if stored_key.is_some() || env_key.is_some() {
                Some(ProviderAccount::ApiKey)
            } else {
                None
            }
        } else if self.info.requires_openai_auth {
            self.auth_manager
                .as_ref()
                .and_then(|auth_manager| {
                    let auth = auth_manager.auth_cached()?;
                    if auth_manager.refresh_failure_for_auth(&auth).is_some() {
                        return None;
                    }
                    Some(auth)
                })
                .map(|auth| match &auth {
                    CodexAuth::ApiKey(_) => Ok(ProviderAccount::ApiKey),
                    CodexAuth::BedrockApiKey(_) => {
                        Err(ProviderAccountError::UnsupportedBedrockApiKeyAuth)
                    }
                    CodexAuth::Chatgpt(_)
                    | CodexAuth::ChatgptAuthTokens(_)
                    | CodexAuth::AgentIdentity(_)
                    | CodexAuth::PersonalAccessToken(_) => {
                        let email = auth.get_account_email();
                        let plan_type = auth.account_plan_type();

                        match (email, plan_type) {
                            (Some(email), Some(plan_type)) => {
                                Ok(ProviderAccount::Chatgpt { email, plan_type })
                            }
                            _ => Err(ProviderAccountError::MissingChatgptAccountDetails),
                        }
                    }
                })
                .transpose()?
        } else {
            None
        };

        Ok(ProviderAccountState {
            account,
            requires_openai_auth: self.info.requires_openai_auth,
        })
    }

    fn models_manager(
        &self,
        codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        let static_catalog = if self.info.is_anthropic() || self.info.is_claude_plan() {
            Some(config_model_catalog.unwrap_or_else(bundled_static_model_catalog))
        } else {
            config_model_catalog
        };

        match static_catalog {
            Some(model_catalog) => Arc::new(StaticModelsManager::new(
                self.auth_manager.clone(),
                model_catalog,
            )),
            None => {
                let endpoint = Arc::new(OpenAiModelsEndpoint::new(
                    self.info.clone(),
                    self.auth_manager.clone(),
                ));
                Arc::new(OpenAiModelsManager::new(
                    codex_home,
                    endpoint,
                    self.auth_manager.clone(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use codex_login::AuthCredentialsStoreMode;
    use codex_login::AuthKeyringBackendKind;
    use codex_login::auth::BedrockApiKeyAuth;
    use codex_login::login_with_provider_api_key;
    use codex_model_provider_info::AMBIENT_DEFAULT_MODEL;
    use codex_model_provider_info::ANTHROPIC_API_KEY_ENV_VAR;
    use codex_model_provider_info::BASETEN_DEFAULT_MODEL;
    use codex_model_provider_info::ModelProviderAwsAuthInfo;
    use codex_model_provider_info::OPENROUTER_DEFAULT_MODEL;
    use codex_model_provider_info::VERCEL_DEFAULT_MODEL;
    use codex_model_provider_info::WireApi;
    use codex_models_manager::manager::RefreshStrategy;
    use codex_protocol::config_types::ModelProviderAuthInfo;
    use codex_protocol::openai_models::ModelInfo;
    use codex_protocol::openai_models::ModelsResponse;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header_regex;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    use super::*;

    struct EnvVarGuard {
        key: String,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: impl Into<String>, value: &str) -> Self {
            let key = key.into();
            let previous = std::env::var(&key).ok();
            // Tests use a unique variable name and restore it on drop.
            unsafe { std::env::set_var(&key, value) };
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(&self.key, value) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn provider_info_with_command_auth() -> ModelProviderInfo {
        ModelProviderInfo {
            auth: Some(ModelProviderAuthInfo {
                command: "print-token".to_string(),
                args: Vec::new(),
                timeout_ms: NonZeroU64::new(5_000).expect("timeout should be non-zero"),
                refresh_interval_ms: 300_000,
                cwd: std::env::current_dir()
                    .expect("current dir should be available")
                    .try_into()
                    .expect("current dir should be absolute"),
            }),
            requires_openai_auth: false,
            ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        }
    }

    fn test_codex_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("codex-model-provider-test-{}", std::process::id()))
    }

    fn provider_for(base_url: String) -> ModelProviderInfo {
        ModelProviderInfo {
            name: "mock".into(),
            base_url: Some(base_url),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(0),
            stream_max_retries: Some(0),
            stream_idle_timeout_ms: Some(5_000),
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    fn remote_model(slug: &str) -> ModelInfo {
        serde_json::from_value(json!({
            "slug": slug,
            "display_name": slug,
            "description": null,
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [],
            "shell_type": "shell_command",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 0,
            "upgrade": null,
            "base_instructions": "base instructions",
            "supports_reasoning_summaries": false,
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode": "bytes", "limit": 10_000},
            "supports_parallel_tool_calls": false,
            "supports_image_detail_original": false,
            "context_window": 272_000,
            "max_context_window": 272_000,
            "experimental_supported_tools": [],
        }))
        .expect("valid model")
    }

    fn bedrock_api_key_auth() -> CodexAuth {
        CodexAuth::BedrockApiKey(BedrockApiKeyAuth {
            api_key: "bedrock-api-key-test".to_string(),
            region: "us-east-1".to_string(),
        })
    }

    #[test]
    fn configured_provider_uses_default_capabilities() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(provider.capabilities(), ProviderCapabilities::default());
    }

    #[test]
    fn ambient_provider_disables_responses_only_capabilities() {
        let provider = create_model_provider(
            ModelProviderInfo::create_ambient_provider(),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.capabilities(),
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
            }
        );
        assert_eq!(
            provider.approval_review_preferred_model(),
            AMBIENT_DEFAULT_MODEL
        );
        assert_eq!(
            provider.memory_extraction_preferred_model(),
            AMBIENT_DEFAULT_MODEL
        );
        assert_eq!(
            provider.memory_consolidation_preferred_model(),
            AMBIENT_DEFAULT_MODEL
        );
    }

    #[test]
    fn claude_plan_provider_uses_plan_model_for_helper_tasks() {
        let provider = create_model_provider(
            ModelProviderInfo::create_claude_plan_provider(),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.approval_review_preferred_model(),
            CLAUDE_PLAN_MODEL
        );
        assert_eq!(
            provider.memory_extraction_preferred_model(),
            CLAUDE_PLAN_MODEL
        );
        assert_eq!(
            provider.memory_consolidation_preferred_model(),
            CLAUDE_PLAN_MODEL
        );
    }

    #[tokio::test]
    async fn claude_plan_provider_uses_static_model_catalog() {
        let provider = create_model_provider(
            ModelProviderInfo::create_claude_plan_provider(),
            /*auth_manager*/ None,
        );

        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);
        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert!(
            catalog
                .models
                .iter()
                .any(|model| model.slug == CLAUDE_PLAN_MODEL),
            "Claude Plan should not poll Anthropic /models during startup"
        );
    }

    #[tokio::test]
    async fn anthropic_provider_uses_static_model_catalog() {
        let provider = create_model_provider(
            ModelProviderInfo::create_anthropic_provider(),
            /*auth_manager*/ None,
        );

        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);
        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert!(
            catalog
                .models
                .iter()
                .any(|model| model.slug == "claude-opus-4-8"),
            "Anthropic API-key models should not be refreshed through OpenAI-compatible /models"
        );
    }

    #[test]
    fn zai_provider_exposes_native_web_search_only() {
        let provider = create_model_provider(
            ModelProviderInfo::create_zai_provider(),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.capabilities(),
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: true,
            }
        );
    }

    #[test]
    fn openrouter_provider_enables_server_web_search_and_uses_openrouter_defaults() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openrouter_provider(),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.capabilities(),
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: true,
            }
        );
        assert_eq!(
            provider.approval_review_preferred_model(),
            OPENROUTER_DEFAULT_MODEL
        );
        assert_eq!(
            provider.memory_extraction_preferred_model(),
            OPENROUTER_DEFAULT_MODEL
        );
        assert_eq!(
            provider.memory_consolidation_preferred_model(),
            OPENROUTER_DEFAULT_MODEL
        );
    }

    #[test]
    fn baseten_provider_disables_hosted_tools_and_uses_baseten_defaults() {
        let provider = create_model_provider(
            ModelProviderInfo::create_baseten_provider(),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.capabilities(),
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
            }
        );
        assert_eq!(
            provider.approval_review_preferred_model(),
            BASETEN_DEFAULT_MODEL
        );
        assert_eq!(
            provider.memory_extraction_preferred_model(),
            BASETEN_DEFAULT_MODEL
        );
        assert_eq!(
            provider.memory_consolidation_preferred_model(),
            BASETEN_DEFAULT_MODEL
        );
    }

    #[test]
    fn vercel_provider_disables_hosted_tools_and_uses_vercel_defaults() {
        let provider = create_model_provider(
            ModelProviderInfo::create_vercel_provider(),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.capabilities(),
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
            }
        );
        assert_eq!(
            provider.approval_review_preferred_model(),
            VERCEL_DEFAULT_MODEL
        );
        assert_eq!(
            provider.memory_extraction_preferred_model(),
            VERCEL_DEFAULT_MODEL
        );
        assert_eq!(
            provider.memory_consolidation_preferred_model(),
            VERCEL_DEFAULT_MODEL
        );
    }

    #[test]
    fn configured_provider_uses_default_approval_review_preferred_model() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.approval_review_preferred_model(),
            DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL
        );
    }

    #[tokio::test]
    async fn configured_provider_runtime_base_url_uses_configured_base_url() {
        let provider = create_model_provider(
            provider_for("https://example.test/v1".to_string()),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider
                .runtime_base_url()
                .await
                .expect("runtime base URL should resolve"),
            Some("https://example.test/v1".to_string())
        );
    }

    #[test]
    fn create_model_provider_builds_command_auth_manager_without_base_manager() {
        let provider = create_model_provider(
            provider_info_with_command_auth(),
            /*auth_manager*/ None,
        );

        let auth_manager = provider
            .auth_manager()
            .expect("command auth provider should have an auth manager");

        assert!(auth_manager.has_external_auth());
    }

    #[test]
    fn create_model_provider_does_not_use_openai_auth_manager_for_amazon_bedrock_provider() {
        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(Some(ModelProviderAwsAuthInfo {
                profile: Some("codex-bedrock".to_string()),
                region: None,
            })),
            Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
                "openai-api-key",
            ))),
        );

        assert!(provider.auth_manager().is_none());
    }

    #[tokio::test]
    async fn create_model_provider_uses_managed_auth_for_amazon_bedrock_provider() {
        let auth = bedrock_api_key_auth();
        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            Some(AuthManager::from_auth_for_testing(auth.clone())),
        );

        assert_eq!(provider.auth().await, Some(auth));
    }

    #[tokio::test]
    async fn configured_provider_prefers_env_key_over_stored_provider_key() {
        let env_key = format!("PFT_PROVIDER_ENV_PRECEDENCE_{}", std::process::id());
        let codex_home = test_codex_home().join(&env_key);
        std::fs::create_dir_all(&codex_home).expect("temp codex home should be created");
        let _guard = EnvVarGuard::set(env_key.clone(), "env-provider-key");
        login_with_provider_api_key(
            &codex_home,
            &env_key,
            "stored-provider-key",
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("stored provider key should be written");

        let provider = create_model_provider(
            ModelProviderInfo {
                env_key: Some(env_key),
                ..provider_for("https://example.test/v1".to_string())
            },
            Some(AuthManager::from_auth_for_testing_with_home(
                CodexAuth::create_dummy_chatgpt_auth_for_testing(),
                codex_home,
            )),
        );

        let auth = provider
            .api_auth()
            .await
            .expect("provider auth should resolve");

        assert_eq!(
            auth.to_auth_headers()
                .get(http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer env-provider-key")
        );
    }

    #[tokio::test]
    async fn anthropic_provider_env_key_uses_x_api_key_header() {
        let _guard = EnvVarGuard::set(ANTHROPIC_API_KEY_ENV_VAR, "env-anthropic-key");
        let provider = create_model_provider(
            ModelProviderInfo::create_anthropic_provider(),
            Some(AuthManager::from_auth_for_testing(
                CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            )),
        );

        let auth = provider
            .api_auth()
            .await
            .expect("provider auth should resolve");
        let headers = auth.to_auth_headers();

        assert_eq!(headers.get(http::header::AUTHORIZATION), None);
        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("env-anthropic-key")
        );
    }

    #[test]
    fn openai_provider_returns_unauthenticated_openai_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: None,
                requires_openai_auth: true,
            })
        );
    }

    #[test]
    fn openai_provider_returns_api_key_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
                "openai-api-key",
            ))),
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: Some(ProviderAccount::ApiKey),
                requires_openai_auth: true,
            })
        );
    }

    #[test]
    fn openai_provider_rejects_chatgpt_account_state_without_email() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            Some(AuthManager::from_auth_for_testing(
                CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            )),
        );

        assert_eq!(
            provider.account_state(),
            Err(ProviderAccountError::MissingChatgptAccountDetails)
        );
    }

    #[test]
    fn openai_provider_rejects_bedrock_api_key_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            Some(AuthManager::from_auth_for_testing(bedrock_api_key_auth())),
        );

        assert_eq!(
            provider.account_state(),
            Err(ProviderAccountError::UnsupportedBedrockApiKeyAuth)
        );
    }

    #[test]
    fn custom_non_openai_provider_returns_no_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo {
                name: "Custom".to_string(),
                base_url: Some("http://localhost:1234/v1".to_string()),
                wire_api: WireApi::Responses,
                requires_openai_auth: false,
                ..Default::default()
            },
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: None,
                requires_openai_auth: false,
            })
        );
    }

    #[test]
    fn amazon_bedrock_provider_returns_bedrock_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: Some(ProviderAccount::AmazonBedrock {
                    credential_source:
                        codex_protocol::account::AmazonBedrockCredentialSource::AwsManaged,
                }),
                requires_openai_auth: false,
            })
        );
    }

    #[tokio::test]
    async fn amazon_bedrock_provider_creates_static_models_manager() {
        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            /*auth_manager*/ None,
        );
        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);

        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;
        let model_ids = catalog
            .models
            .iter()
            .map(|model| model.slug.as_str())
            .collect::<Vec<_>>();

        assert_eq!(model_ids, vec!["openai.gpt-5.5", "openai.gpt-5.4"]);

        let default_model = manager
            .list_models(RefreshStrategy::Online)
            .await
            .into_iter()
            .find(|preset| preset.is_default)
            .expect("Bedrock catalog should have a default model");

        assert_eq!(default_model.model, "openai.gpt-5.5");
    }

    #[tokio::test]
    async fn configured_bedrock_catalog_only_allows_default_service_tier() {
        let configured_model = codex_models_manager::bundled_models_response()
            .expect("bundled models should parse")
            .models
            .into_iter()
            .find(|model| model.slug == "gpt-5.5")
            .expect("bundled models should include GPT-5.5");
        assert!(!configured_model.additional_speed_tiers.is_empty());
        assert!(!configured_model.service_tiers.is_empty());

        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            /*auth_manager*/ None,
        );
        let manager = provider.models_manager(
            test_codex_home(),
            Some(ModelsResponse {
                models: vec![configured_model],
            }),
        );

        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].slug, "gpt-5.5");
        assert_eq!(
            catalog.models[0].additional_speed_tiers,
            Vec::<String>::new()
        );
        assert_eq!(catalog.models[0].service_tiers, Vec::new());
        assert_eq!(catalog.models[0].default_service_tier, None);
    }

    #[tokio::test]
    async fn configured_provider_models_manager_uses_provider_bearer_token() {
        let server = MockServer::start().await;
        let remote_models = vec![remote_model("provider-model")];

        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header_regex("Authorization", "Bearer provider-token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(ModelsResponse {
                        models: remote_models.clone(),
                    }),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut provider_info = provider_for(server.uri());
        provider_info.experimental_bearer_token = Some("provider-token".to_string());
        let provider = create_model_provider(
            provider_info,
            Some(AuthManager::from_auth_for_testing(
                CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            )),
        );

        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);
        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert!(
            catalog
                .models
                .iter()
                .any(|model| model.slug == "provider-model")
        );
    }
}
