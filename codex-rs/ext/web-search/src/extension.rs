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
use codex_login::AuthManager;
use codex_model_provider::SharedModelProvider;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::config_types::WebSearchProvider;
use codex_protocol::shell_environment::TINYFISH_API_KEY_ENV_VAR;
use codex_utils_redacted_string::RedactedString;
#[cfg(feature = "test-support")]
use url::Url;

use crate::tinyfish_tool::TinyFishRuntime;
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
    api_key_env_value: RedactedString,
}

#[derive(Clone)]
struct WebSearchExtensionConfig {
    available: bool,
    backend: WebSearchBackendConfig,
    settings: SearchSettings,
}

#[derive(Clone)]
enum WebSearchBackendConfig {
    Model { provider: Box<ModelProviderInfo> },
    Tinyfish { runtime: TinyFishRuntime },
}

#[derive(Clone)]
pub(crate) enum WebSearchBackend {
    Model { provider: SharedModelProvider },
    Tinyfish { runtime: TinyFishRuntime },
}

impl From<&Config> for WebSearchExtensionConfig {
    fn from(config: &Config) -> Self {
        Self::from_with_tinyfish_runtime(config, || {
            TinyFishRuntime::new(
                config.http_client_factory(),
                tinyfish_api_key_from(std::env::var),
            )
        })
    }
}

impl WebSearchExtensionConfig {
    fn from_with_tinyfish_runtime(
        config: &Config,
        tinyfish_runtime: impl FnOnce() -> TinyFishRuntime,
    ) -> Self {
        let web_search_mode = config.web_search_mode.value();
        let web_search_provider = config
            .web_search_config
            .as_ref()
            .map(|config| config.provider)
            .unwrap_or_default();
        let backend = match web_search_provider {
            WebSearchProvider::Model => WebSearchBackendConfig::Model {
                provider: Box::new(config.model_provider.clone()),
            },
            WebSearchProvider::Tinyfish => WebSearchBackendConfig::Tinyfish {
                runtime: tinyfish_runtime(),
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
        if let Some(test_backend) = self.tinyfish_test_backend.as_ref() {
            return WebSearchExtensionConfig::from_with_tinyfish_runtime(config, || {
                crate::tinyfish_tool::test_support::runtime(
                    config.http_client_factory(),
                    test_backend.endpoint.clone(),
                    tinyfish_api_key_from(|name| {
                        if name == TINYFISH_API_KEY_ENV_VAR {
                            Ok(test_backend.api_key_env_value.as_str().to_owned())
                        } else {
                            Err(std::env::VarError::NotPresent)
                        }
                    }),
                )
            });
        }
        WebSearchExtensionConfig::from(config)
    }
}

fn tinyfish_api_key_from(
    get_env: impl FnOnce(&'static str) -> Result<String, std::env::VarError>,
) -> Option<RedactedString> {
    get_env(TINYFISH_API_KEY_ENV_VAR)
        .ok()
        .map(|api_key| api_key.trim().to_string())
        .filter(|api_key| !api_key.is_empty())
        .map(RedactedString::from)
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

        let backend = match &config.backend {
            WebSearchBackendConfig::Model { provider } => WebSearchBackend::Model {
                provider: create_model_provider(
                    provider.as_ref().clone(),
                    Some(self.auth_manager.clone()),
                ),
            },
            WebSearchBackendConfig::Tinyfish { runtime } => WebSearchBackend::Tinyfish {
                runtime: runtime.clone(),
            },
        };

        vec![Arc::new(WebSearchTool {
            session_id: session_store.level_id().to_string(),
            backend,
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
pub(crate) fn install_tinyfish(
    registry: &mut ExtensionRegistryBuilder<Config>,
    auth_manager: Arc<AuthManager>,
    endpoint: Url,
    api_key: RedactedString,
) {
    let extension = Arc::new(WebSearchExtension {
        auth_manager,
        tinyfish_test_backend: Some(TinyfishTestBackend {
            endpoint,
            api_key_env_value: api_key,
        }),
    });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    use codex_core::config::ConfigBuilder;
    use codex_extension_api::ExtensionData;
    use codex_extension_api::ExtensionRegistryBuilder;
    use codex_extension_api::ToolName;
    use codex_extension_api::ToolSpec;
    use codex_login::CodexAuth;
    use codex_model_provider_info::ModelProviderInfo;
    use codex_protocol::config_types::WebSearchProvider;
    use codex_tools::ResponsesApiNamespaceTool;
    use codex_tools::ToolExposure;
    use pretty_assertions::assert_eq;

    use super::AuthManager;
    use super::Config;
    use super::WebSearchBackendConfig;
    use super::WebSearchExtensionConfig;
    use super::external_web_access_for_mode;
    use super::install;
    use super::provider_available;
    use super::tinyfish_api_key_from;
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
    fn installed_model_backend_preserves_web_run_availability() {
        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        let auth_manager = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("dummy"));
        install(&mut builder, auth_manager);
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("11111111-1111-4111-8111-111111111111");
        thread_store.insert(WebSearchExtensionConfig {
            available: true,
            backend: WebSearchBackendConfig::Model {
                provider: Box::new(ModelProviderInfo::create_openai_provider(
                    /*base_url*/ None,
                )),
            },
            settings: Default::default(),
        });

        let tools = registry
            .tool_contributors()
            .iter()
            .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
            .collect::<Vec<_>>();
        let [tool] = tools.as_slice() else {
            panic!("enabled model web search should contribute one tool");
        };

        assert_eq!(
            (
                tool.tool_name(),
                tool.exposure(),
                tool.supports_parallel_tool_calls(),
            ),
            (
                ToolName::namespaced(WEB_NAMESPACE, RUN_TOOL_NAME),
                ToolExposure::Direct,
                true,
            )
        );

        let ToolSpec::Namespace(namespace) = tool.spec() else {
            panic!("model web search should retain its namespace tool spec");
        };
        let [ResponsesApiNamespaceTool::Function(function)] = namespace.tools.as_slice() else {
            panic!("web namespace should retain one function tool");
        };
        let schema = serde_json::to_value(&function.parameters).expect("schema should serialize");
        let properties = schema["properties"]
            .as_object()
            .expect("schema properties should be an object");
        for command in [
            "search_query",
            "image_query",
            "open",
            "click",
            "find",
            "screenshot",
            "finance",
            "weather",
            "sports",
            "time",
            "response_length",
        ] {
            assert!(
                properties.contains_key(command),
                "model web search should retain the {command} command"
            );
        }
    }

    #[test]
    fn tinyfish_provider_is_available_only_in_live_mode() {
        let unsupported_model_provider = ModelProviderInfo {
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
            .map(|mode| {
                provider_available(
                    WebSearchProvider::Tinyfish,
                    mode,
                    &unsupported_model_provider,
                )
            }),
            [false, false, false, true]
        );
    }

    #[test]
    fn tinyfish_api_key_loading_uses_the_named_nonblank_value() {
        assert_eq!(
            tinyfish_api_key_from(|name| {
                assert_eq!(name, super::TINYFISH_API_KEY_ENV_VAR);
                Err(std::env::VarError::NotPresent)
            }),
            None
        );
        assert_eq!(
            tinyfish_api_key_from(|name| {
                assert_eq!(name, super::TINYFISH_API_KEY_ENV_VAR);
                Ok(" \t\n ".to_string())
            }),
            None
        );
        assert_eq!(
            tinyfish_api_key_from(|name| {
                assert_eq!(name, super::TINYFISH_API_KEY_ENV_VAR);
                Ok(" private-key ".to_string())
            }),
            Some("private-key".into())
        );
    }

    #[tokio::test]
    async fn live_tinyfish_config_activates_tinyfish_backend() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test time should follow the Unix epoch")
            .as_nanos();
        let test_root = std::env::temp_dir().join(format!(
            "codex-web-search-dispatch-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&test_root).expect("test root should be created");
        let config_result = ConfigBuilder::default()
            .codex_home(test_root.clone())
            .fallback_cwd(Some(test_root.clone()))
            .cli_overrides(vec![
                ("web_search".to_string(), "live".into()),
                ("tools.web_search.provider".to_string(), "tinyfish".into()),
            ])
            .build()
            .await;
        fs::remove_dir_all(&test_root).expect("test root should be removed");
        let config = config_result.expect("TinyFish config should load");
        assert_eq!(
            config
                .web_search_config
                .as_ref()
                .map(|config| config.provider),
            Some(WebSearchProvider::Tinyfish)
        );

        let extension_config = WebSearchExtensionConfig::from(&config);
        assert!(extension_config.available);
        assert!(matches!(
            extension_config.backend,
            WebSearchBackendConfig::Tinyfish { .. }
        ));

        let mut model_config = config.clone();
        model_config
            .web_search_config
            .get_or_insert_with(Default::default)
            .provider = WebSearchProvider::Model;
        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        let auth_manager = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("dummy"));
        install(&mut builder, auth_manager);
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("11111111-1111-4111-8111-111111111111");
        thread_store.insert(WebSearchExtensionConfig::from(&model_config));
        let web_run_spec = || {
            let tools = registry
                .tool_contributors()
                .iter()
                .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
                .collect::<Vec<_>>();
            match tools.as_slice() {
                [] => None,
                [tool] => Some(tool.spec()),
                _ => panic!("web search should contribute at most one tool"),
            }
        };

        let Some(ToolSpec::Namespace(model_namespace)) = web_run_spec() else {
            panic!("model-backed web search should contribute web.run");
        };
        let [ResponsesApiNamespaceTool::Function(model_function)] =
            model_namespace.tools.as_slice()
        else {
            panic!("model-backed web search should expose one function");
        };

        registry.config_contributors()[0].on_config_changed(
            &session_store,
            &thread_store,
            &model_config,
            &config,
        );
        let Some(ToolSpec::Namespace(tinyfish_namespace)) = web_run_spec() else {
            panic!("TinyFish web search should contribute web.run");
        };
        let [ResponsesApiNamespaceTool::Function(tinyfish_function)] =
            tinyfish_namespace.tools.as_slice()
        else {
            panic!("TinyFish web search should expose one function");
        };
        let tinyfish_properties = serde_json::to_value(&tinyfish_function.parameters)
            .expect("schema should serialize")["properties"]
            .as_object()
            .expect("schema properties should be an object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            (
                tinyfish_properties,
                tinyfish_function.description.len() < 1_024,
                tinyfish_function.description != model_function.description,
            ),
            (
                vec!["response_length".to_string(), "search_query".to_string()],
                true,
                true,
            )
        );

        let mut disabled_config = config.clone();
        disabled_config
            .web_search_mode
            .set(WebSearchMode::Disabled)
            .expect("web search mode should be mutable in tests");
        registry.config_contributors()[0].on_config_changed(
            &session_store,
            &thread_store,
            &config,
            &disabled_config,
        );
        assert!(web_run_spec().is_none());
    }
}
