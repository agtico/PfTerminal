use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ModelProviderCapabilitiesReadParams;
use codex_app_server_protocol::ModelProviderCapabilitiesReadResponse;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

async fn read_capabilities(codex_home: &Path) -> Result<ModelProviderCapabilitiesReadResponse> {
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home)
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_TIMEOUT)
        .await?;

    let request_id = mcp
        .send_model_provider_capabilities_read_request(ModelProviderCapabilitiesReadParams {})
        .await?;
    let received: ModelProviderCapabilitiesReadResponse =
        timeout(DEFAULT_TIMEOUT, mcp.read_response(request_id)).await??;

    Ok(received)
}

#[tokio::test]
async fn read_default_provider_capabilities() -> Result<()> {
    let codex_home = TempDir::new()?;
    let received = read_capabilities(codex_home.path()).await?;

    assert_eq!(
        received,
        ModelProviderCapabilitiesReadResponse {
            // PFTerminal's unconfigured provider is Ambient, whose capability
            // surface is intentionally conservative. Explicit OpenAI behavior
            // is pinned separately below.
            namespace_tools: false,
            image_generation: false,
            web_search: false,
        }
    );
    Ok(())
}

#[tokio::test]
async fn read_openai_provider_capabilities() -> Result<()> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"model_provider = "openai"
"#,
    )?;
    let received = read_capabilities(codex_home.path()).await?;

    // This test pins the explicitly-configured OpenAI provider's capability
    // contract. OpenAI uses the ProviderCapabilities::default() branch.
    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: true,
        image_generation: true,
        web_search: true,
    };
    assert_eq!(received, expected);
    Ok(())
}

#[tokio::test]
async fn read_default_provider_capabilities_profiles_cover_all_branches() -> Result<()> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"model_provider = "ambient"
"#,
    )?;
    let received = read_capabilities(codex_home.path()).await?;
    // Explicitly-configured Ambient intentionally exposes no namespace tools,
    // image generation, or web search.
    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: false,
        image_generation: false,
        web_search: false,
    };
    assert_eq!(received, expected);

    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"model_provider = "openrouter"
"#,
    )?;
    let received = read_capabilities(codex_home.path()).await?;
    // Explicitly-configured OpenRouter exposes hosted web search only.
    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: false,
        image_generation: false,
        web_search: true,
    };
    assert_eq!(received, expected);

    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"model_provider = "openai"
"#,
    )?;
    let received = read_capabilities(codex_home.path()).await?;
    // Explicitly-configured OpenAI uses ProviderCapabilities::default().
    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: true,
        image_generation: true,
        web_search: true,
    };
    assert_eq!(received, expected);

    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"model_provider = "claude-plan"
"#,
    )?;
    let received = read_capabilities(codex_home.path()).await?;
    // Anthropic Messages has plain function tools but no Responses namespace
    // container, so collaboration tools must be flattened at this boundary.
    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: false,
        image_generation: false,
        web_search: true,
    };
    assert_eq!(received, expected);

    // Bedrock has a provider-specific override; the existing test below pins
    // that separate capability contract.
    Ok(())
}

#[tokio::test]
async fn read_amazon_bedrock_provider_capabilities() -> Result<()> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"model_provider = "amazon-bedrock"
"#,
    )?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_TIMEOUT)
        .await?;

    let request_id = mcp
        .send_model_provider_capabilities_read_request(ModelProviderCapabilitiesReadParams {})
        .await?;
    let received: ModelProviderCapabilitiesReadResponse =
        timeout(DEFAULT_TIMEOUT, mcp.read_response(request_id)).await??;

    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: true,
        image_generation: false,
        web_search: true,
    };
    assert_eq!(received, expected);
    Ok(())
}
