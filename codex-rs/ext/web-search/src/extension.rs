use std::sync::Arc;

use codex_api::AllowedCaller;
use codex_api::ApproximateLocation;
use codex_api::ExternalWebAccess;
use codex_api::ExternalWebAccessMode;
use codex_api::LocationType;
use codex_api::SearchContextSize;
use codex_api::SearchFilters;
use codex_api::SearchSettings;
use codex_core::config::Config;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadOriginator;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolContributor;
use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::config_types::WebSearchProvider;
use codex_protocol::shell_environment::TINYFISH_API_KEY_ENV_VAR;
use codex_utils_redacted_string::RedactedString;
use url::Url;

use crate::tinyfish::TINYFISH_SEARCH_ENDPOINT;
use crate::tool::WebSearchTool;

#[derive(Clone)]
struct WebSearchExtension {
    auth_manager: Arc<AuthManager>,
    #[cfg(feature = "test-support")]
    tinyfish_test_backend: Option<TinyfishTestBackend>,
}

#[cfg(feature = "test-support")]
#[derive(Clone)]
struct TinyfishTestBackend {
    endpoint: Url,
    api_key: RedactedString,
}

#[derive(Clone)]
struct WebSearchExtensionConfig {
    available: bool,
    backend: WebSearchBackend,
    settings: SearchSettings,
}

#[derive(Clone)]
pub(crate) enum WebSearchBackend {
    Model {
        provider: Box<ModelProviderInfo>,
    },
    Tinyfish {
        http_client_factory: HttpClientFactory,
        endpoint: Url,
        api_key: Option<RedactedString>,
    },
}

impl From<&Config> for WebSearchExtensionConfig {
    fn from(config: &Config) -> Self {
        let web_search_mode = config.web_search_mode.value();
        let web_search_provider = config
            .web_search_config
            .as_ref()
            .map(|config| config.provider)
            .unwrap_or_default();
        let backend = match web_search_provider {
            WebSearchProvider::Model => WebSearchBackend::Model {
                provider: Box::new(config.model_provider.clone()),
            },
            WebSearchProvider::Tinyfish => WebSearchBackend::Tinyfish {
                http_client_factory: config.http_client_factory(),
                endpoint: match Url::parse(TINYFISH_SEARCH_ENDPOINT) {
                    Ok(endpoint) => endpoint,
                    Err(err) => panic!("TinyFish search endpoint should be a valid URL: {err}"),
                },
                api_key: std::env::var(TINYFISH_API_KEY_ENV_VAR)
                    .ok()
                    .filter(|api_key| !api_key.trim().is_empty())
                    .map(RedactedString::from),
            },
        };
        Self {
            available: provider_available(
                web_search_provider,
                web_search_mode,
                &config.model_provider,
            ),
            backend,
            settings: search_settings(config, web_search_mode),
        }
    }
}

impl WebSearchExtension {
    fn config(&self, config: &Config) -> WebSearchExtensionConfig {
        #[cfg(feature = "test-support")]
        let extension_config = {
            let mut extension_config = WebSearchExtensionConfig::from(config);
            if let Some(test_backend) = self.tinyfish_test_backend.as_ref()
                && matches!(&extension_config.backend, WebSearchBackend::Tinyfish { .. })
            {
                extension_config.backend = WebSearchBackend::Tinyfish {
                    http_client_factory: config.http_client_factory(),
                    endpoint: test_backend.endpoint.clone(),
                    api_key: Some(test_backend.api_key.clone()),
                };
            }
            extension_config
        };
        #[cfg(not(feature = "test-support"))]
        let extension_config = WebSearchExtensionConfig::from(config);
        extension_config
    }
}

fn provider_available(
    web_search_provider: WebSearchProvider,
    web_search_mode: WebSearchMode,
    model_provider: &ModelProviderInfo,
) -> bool {
    match web_search_provider {
        WebSearchProvider::Model => {
            (model_provider.is_openai()
                || model_provider.uses_openai_actor_authorization()
                || model_provider.supports_standalone_web_search)
                && web_search_mode != WebSearchMode::Disabled
        }
        WebSearchProvider::Tinyfish => web_search_mode == WebSearchMode::Live,
    }
}

fn search_settings(config: &Config, web_search_mode: WebSearchMode) -> SearchSettings {
    let web_search_config = config.web_search_config.as_ref();
    SearchSettings {
        user_location: web_search_config
            .and_then(|config| config.user_location.as_ref())
            .map(|location| ApproximateLocation {
                r#type: LocationType::Approximate,
                country: location.country.clone(),
                region: location.region.clone(),
                city: location.city.clone(),
                timezone: location.timezone.clone(),
            }),
        search_context_size: web_search_config
            .and_then(|config| config.search_context_size)
            .map(|size| match size {
                WebSearchContextSize::Low => SearchContextSize::Low,
                WebSearchContextSize::Medium => SearchContextSize::Medium,
                WebSearchContextSize::High => SearchContextSize::High,
            }),
        filters: web_search_config
            .and_then(|config| config.filters.as_ref())
            .map(|filters| SearchFilters {
                allowed_domains: filters.allowed_domains.clone(),
                blocked_domains: None,
            }),
        allowed_callers: Some(vec![AllowedCaller::Direct]),
        external_web_access: Some(external_web_access_for_mode(web_search_mode)),
        ..Default::default()
    }
}

fn external_web_access_for_mode(web_search_mode: WebSearchMode) -> ExternalWebAccess {
    match web_search_mode {
        WebSearchMode::Disabled | WebSearchMode::Cached => ExternalWebAccess::Boolean(false),
        WebSearchMode::Indexed => ExternalWebAccess::Mode(ExternalWebAccessMode::Indexed),
        WebSearchMode::Live => ExternalWebAccess::Boolean(true),
    }
}

impl ThreadLifecycleContributor<Config> for WebSearchExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            input.thread_store.insert(self.config(input.config));
        })
    }
}

impl ConfigContributor<Config> for WebSearchExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        thread_store.insert(self.config(new_config));
    }
}

impl ToolContributor for WebSearchExtension {
    fn tools(
        &self,
        session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<
        Arc<dyn for<'call> codex_extension_api::ToolExecutor<codex_extension_api::ToolCall<'call>>>,
    > {
        let Some(config) = thread_store.get::<WebSearchExtensionConfig>() else {
            return Vec::new();
        };
        if !config.available {
            return Vec::new();
        }

        vec![Arc::new(WebSearchTool {
            session_id: session_store.level_id().to_string(),
            backend: config.backend.clone(),
            auth_manager: self.auth_manager.clone(),
            settings: config.settings.clone(),
            originator: thread_store
                .get::<ThreadOriginator>()
                .map(|originator| originator.0.clone()),
        })]
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>, auth_manager: Arc<AuthManager>) {
    let extension = Arc::new(WebSearchExtension {
        auth_manager,
        #[cfg(feature = "test-support")]
        tinyfish_test_backend: None,
    });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(feature = "test-support")]
/// Installs web search with an injected TinyFish backend for integration tests.
pub fn install_tinyfish_for_test(
    registry: &mut ExtensionRegistryBuilder<Config>,
    auth_manager: Arc<AuthManager>,
    endpoint: Url,
    api_key: RedactedString,
) {
    let extension = Arc::new(WebSearchExtension {
        auth_manager,
        tinyfish_test_backend: Some(TinyfishTestBackend { endpoint, api_key }),
    });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
mod tests {
    use codex_extension_api::ExtensionData;
    use codex_extension_api::ExtensionRegistryBuilder;
    use codex_extension_api::ToolName;
    use codex_extension_api::ToolSpec;
    use codex_http_client::HttpClientFactory;
    use codex_http_client::OutboundProxyPolicy;
    use codex_login::CodexAuth;
    use codex_model_provider_info::ModelProviderInfo;
    use codex_protocol::config_types::WebSearchProvider;
    use codex_tools::ResponsesApiNamespaceTool;
    use pretty_assertions::assert_eq;
    use serde_json::Value;
    use url::Url;

    use super::AuthManager;
    use super::Config;
    use super::WebSearchBackend;
    use super::WebSearchExtensionConfig;
    use super::external_web_access_for_mode;
    use super::install;
    use super::provider_available;
    use crate::tinyfish::TINYFISH_SEARCH_ENDPOINT;
    use crate::tool::RUN_TOOL_NAME;
    use crate::tool::WEB_NAMESPACE;
    use codex_api::ExternalWebAccess;
    use codex_api::ExternalWebAccessMode;
    use codex_protocol::config_types::WebSearchMode;

    #[test]
    fn external_web_access_preserves_legacy_values_until_indexed() {
        assert_eq!(
            [
                WebSearchMode::Disabled,
                WebSearchMode::Cached,
                WebSearchMode::Indexed,
                WebSearchMode::Live,
            ]
            .map(external_web_access_for_mode),
            [
                ExternalWebAccess::Boolean(false),
                ExternalWebAccess::Boolean(false),
                ExternalWebAccess::Mode(ExternalWebAccessMode::Indexed),
                ExternalWebAccess::Boolean(true),
            ]
        );
    }

    #[test]
    fn installed_extension_contributes_web_run_when_enabled() {
        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        install(
            &mut builder,
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("dummy")),
        );
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("11111111-1111-4111-8111-111111111111");
        thread_store.insert(WebSearchExtensionConfig {
            available: true,
            backend: WebSearchBackend::Model {
                provider: Box::new(ModelProviderInfo::create_openai_provider(
                    /*base_url*/ None,
                )),
            },
            settings: Default::default(),
        });

        let tool_names = registry
            .tool_contributors()
            .iter()
            .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
            .map(|tool| (tool.tool_name(), tool.supports_parallel_tool_calls()))
            .collect::<Vec<_>>();

        assert_eq!(
            tool_names,
            vec![(ToolName::namespaced(WEB_NAMESPACE, RUN_TOOL_NAME), true)]
        );
    }

    #[test]
    fn tinyfish_provider_contributes_web_run_without_model_provider_support() {
        let model_provider = ModelProviderInfo {
            supports_standalone_web_search: false,
            ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        };
        assert!(provider_available(
            WebSearchProvider::Tinyfish,
            WebSearchMode::Live,
            &model_provider,
        ));

        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        install(
            &mut builder,
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("dummy")),
        );
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("11111111-1111-4111-8111-111111111111");
        thread_store.insert(tinyfish_extension_config(/*available*/ true));

        let tool_names = registry
            .tool_contributors()
            .iter()
            .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
            .map(|tool| tool.tool_name())
            .collect::<Vec<_>>();

        assert_eq!(
            tool_names,
            vec![ToolName::namespaced(WEB_NAMESPACE, RUN_TOOL_NAME)]
        );
    }

    #[test]
    fn tinyfish_provider_is_available_only_in_live_mode() {
        let model_provider = ModelProviderInfo {
            supports_standalone_web_search: false,
            ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        };
        assert_eq!(
            [
                WebSearchMode::Disabled,
                WebSearchMode::Cached,
                WebSearchMode::Indexed,
                WebSearchMode::Live,
            ]
            .map(|mode| provider_available(
                WebSearchProvider::Tinyfish,
                mode,
                &model_provider
            )),
            [false, false, false, true]
        );
    }

    #[test]
    fn tinyfish_provider_exposes_search_only_schema() {
        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        install(
            &mut builder,
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("dummy")),
        );
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("11111111-1111-4111-8111-111111111111");
        thread_store.insert(tinyfish_extension_config(/*available*/ true));

        let tools = registry.tool_contributors()[0].tools(&session_store, &thread_store);
        let ToolSpec::Namespace(namespace) = tools[0].spec() else {
            panic!("web.run should advertise a namespace tool");
        };
        let ResponsesApiNamespaceTool::Function(function) = &namespace.tools[0] else {
            panic!("web.run should advertise a function tool");
        };
        let schema = serde_json::to_value(&function.parameters).expect("schema should serialize");
        let properties = schema["properties"]
            .as_object()
            .expect("schema properties should be an object");

        assert_eq!(
            properties.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["response_length", "search_query"]
        );
        assert_eq!(schema["required"], serde_json::json!(["search_query"]));
        assert_eq!(schema["properties"]["search_query"]["minItems"], 1);
        assert_eq!(schema["properties"]["search_query"]["maxItems"], 4);
        for unsupported in [
            "open",
            "click",
            "find",
            "screenshot",
            "image_query",
            "finance",
            "weather",
            "sports",
            "time",
        ] {
            assert_eq!(properties.get(unsupported), None::<&Value>);
        }
    }

    fn tinyfish_extension_config(available: bool) -> WebSearchExtensionConfig {
        WebSearchExtensionConfig {
            available,
            backend: WebSearchBackend::Tinyfish {
                http_client_factory: HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
                endpoint: Url::parse(TINYFISH_SEARCH_ENDPOINT).expect("valid TinyFish endpoint"),
                api_key: None,
            },
            settings: Default::default(),
        }
    }
}
