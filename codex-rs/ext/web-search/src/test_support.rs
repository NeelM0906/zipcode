use std::sync::Arc;

use codex_core::config::Config;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_login::AuthManager;
use codex_utils_redacted_string::RedactedString;
use url::Url;

/// Installs web search with an injected TinyFish backend for integration tests.
pub fn install_tinyfish(
    registry: &mut ExtensionRegistryBuilder<Config>,
    auth_manager: Arc<AuthManager>,
    endpoint: Url,
    api_key: RedactedString,
) {
    crate::extension::install_tinyfish(registry, auth_manager, endpoint, api_key);
}
