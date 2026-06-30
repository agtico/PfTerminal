pub(crate) mod cache;
pub mod collaboration_mode_presets;
pub(crate) mod config;
pub mod manager;
pub mod model_info;
pub mod model_presets;
pub mod test_support;

pub use codex_app_server_protocol::AuthMode;
pub use config::ModelsManagerConfig;

/// OpenAI backend compatibility version for `/models` catalog requests.
///
/// Keep this in sync with `codex_model_provider_info::OPENAI_CODEX_COMPAT_VERSION`.
/// It is separate from PFTerminal's package version because OpenAI gates some
/// model metadata by upstream Codex client compatibility, not fork release
/// numbering.
pub const OPENAI_CODEX_COMPAT_VERSION: &str = "0.124.0";

/// Load the bundled model catalog shipped with `codex-models-manager`.
pub fn bundled_models_response()
-> std::result::Result<codex_protocol::openai_models::ModelsResponse, serde_json::Error> {
    serde_json::from_str(include_str!("../models.json"))
}

/// Convert the client version string to a whole version string (e.g. "1.2.3-alpha.4" -> "1.2.3").
pub fn client_version_to_whole() -> String {
    OPENAI_CODEX_COMPAT_VERSION.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_version_uses_openai_compat_version() {
        assert_eq!(client_version_to_whole(), OPENAI_CODEX_COMPAT_VERSION);
    }
}
