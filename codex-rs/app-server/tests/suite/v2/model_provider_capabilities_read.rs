use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::ModelProviderCapabilitiesReadParams;
use codex_app_server_protocol::ModelProviderCapabilitiesReadResponse;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

async fn read_capabilities(
    codex_home: &std::path::Path,
) -> Result<ModelProviderCapabilitiesReadResponse> {
    let mut mcp = TestAppServer::new(codex_home).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_model_provider_capabilities_read_request(ModelProviderCapabilitiesReadParams {})
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    to_response(response)
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
    let received = read_capabilities(codex_home.path()).await?;
    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: true,
        image_generation: false,
        web_search: false,
    };
    assert_eq!(received, expected);
    Ok(())
}
