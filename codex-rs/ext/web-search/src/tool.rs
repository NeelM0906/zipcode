use std::sync::Arc;

use codex_api::ReqwestTransport;
use codex_api::SearchClient;
use codex_api::SearchCommands;
use codex_api::SearchQuery;
use codex_api::SearchRequest;
use codex_api::SearchResponseLength;
use codex_api::SearchSettings;
use codex_core::X_CODEX_TURN_METADATA_HEADER;
use codex_core::web_search_action_detail;
use codex_extension_api::ExtensionTurnItem;
use codex_extension_api::FunctionCallError;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema_without_compaction;
use codex_extension_items::ExtensionItem;
use codex_extension_items::web_search::WebSearchAction;
use codex_extension_items::web_search::WebSearchItem;
use codex_login::AuthManager;
use codex_login::default_client::add_originator_header;
use codex_login::default_client::create_client;
use codex_model_provider::create_model_provider;
use codex_protocol::models::WebSearchAction as CoreWebSearchAction;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WebSearchBeginEvent;
use codex_protocol::protocol::WebSearchEndEvent;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolExposure;
use codex_tools::default_namespace_description;
use http::HeaderMap;
use http::HeaderValue;
use url::Url;

use crate::extension::WebSearchBackend;
use crate::history::recent_input;
use crate::output::SearchOutput;
use crate::schema::TinyFishCommands;
use crate::schema::commands_schema;
use crate::schema::tinyfish_commands_schema;
use crate::tinyfish::TinyFishSearchClient;
use crate::tinyfish::TinyFishSearchRequest;
use crate::tinyfish::TinyFishSearchResponse;

pub(crate) const WEB_NAMESPACE: &str = "web";
pub(crate) const RUN_TOOL_NAME: &str = "run";
const WEB_RUN_DESCRIPTION: &str = include_str!("../web_run_description.md");
const TINYFISH_WEB_RUN_DESCRIPTION: &str = include_str!("../tinyfish_web_run_description.md");
const RESULTS_PAYLOAD_BYTES_METRIC: &str = "codex.web_search.results.payload_bytes";

pub(crate) struct WebSearchTool {
    pub(crate) session_id: String,
    pub(crate) backend: WebSearchBackend,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) settings: SearchSettings,
    pub(crate) originator: Option<String>,
}

impl<'call> ToolExecutor<ToolCall<'call>> for WebSearchTool {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(WEB_NAMESPACE, RUN_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        // parse schema without compaction that removes field metadata/descriptions to match hosted tool definition
        let (schema, description) = match self.backend {
            WebSearchBackend::Model { .. } => (commands_schema(), WEB_RUN_DESCRIPTION),
            WebSearchBackend::Tinyfish { .. } => {
                (tinyfish_commands_schema(), TINYFISH_WEB_RUN_DESCRIPTION)
            }
        };
        let parameters = match parse_tool_input_schema_without_compaction(&schema) {
            Ok(parameters) => parameters,
            Err(err) => panic!("search command schema should parse: {err}"),
        };

        ToolSpec::Namespace(ResponsesApiNamespace {
            name: WEB_NAMESPACE.to_string(),
            description: default_namespace_description(WEB_NAMESPACE),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: RUN_TOOL_NAME.to_string(),
                description: description.to_string(),
                strict: false,
                parameters,
                output_schema: None,
                defer_loading: None,
            })],
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle<'a>(&'a self, call: ToolCall<'call>) -> codex_extension_api::ToolExecutorFuture<'a>
    where
        'call: 'a,
    {
        Box::pin(self.handle_call(call))
    }
}

impl WebSearchTool {
    async fn handle_call(
        &self,
        call: ToolCall<'_>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        match &self.backend {
            WebSearchBackend::Model { provider } => self.handle_model_call(call, provider).await,
            WebSearchBackend::Tinyfish {
                http_client_factory,
                endpoint,
                api_key,
            } => {
                self.handle_tinyfish_call(call, http_client_factory, endpoint, api_key.as_ref())
                    .await
            }
        }
    }

    async fn handle_model_call(
        &self,
        call: ToolCall<'_>,
        model_provider: &codex_model_provider_info::ModelProviderInfo,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let commands = parse_commands(&call)?;
        let command_action = command_action(&commands);
        let provider =
            create_model_provider(model_provider.clone(), Some(self.auth_manager.clone()));
        let api_provider = provider
            .api_provider()
            .await
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        let auth = provider
            .api_auth()
            .await
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        let client = SearchClient::new(
            ReqwestTransport::from_http_client(create_client()),
            api_provider,
            auth,
        );
        let request = SearchRequest {
            id: self.session_id.clone(),
            model: call.model.clone(),
            reasoning: None,
            input: recent_input(call.conversation_history.items()),
            commands: Some(commands),
            settings: Some(self.settings.clone()),
            max_output_tokens: Some(
                u64::try_from(call.truncation_policy.token_budget()).unwrap_or(u64::MAX),
            ),
        };
        let extra_headers = search_request_headers(
            self.originator.as_deref(),
            call.codex_turn_metadata.as_deref(),
        );
        call.turn_item_emitter
            .emit_started(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: String::new(),
                    action: None,
                    results: None,
                },
                EventMsg::WebSearchBegin(WebSearchBeginEvent {
                    call_id: call.call_id.clone(),
                }),
            ))
            .await;
        let response = client
            .search(&request, extra_headers)
            .await
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        let output = response.output;
        let results = response.results;
        if let Some(results) = results.as_deref() {
            record_results_payload_bytes(results);
        }
        let legacy_action = match &command_action {
            WebSearchAction::Search { query, queries } => CoreWebSearchAction::Search {
                query: query.clone(),
                queries: queries.clone(),
            },
            WebSearchAction::OpenPage { url } => CoreWebSearchAction::OpenPage { url: url.clone() },
            WebSearchAction::FindInPage { url, pattern } => CoreWebSearchAction::FindInPage {
                url: url.clone(),
                pattern: pattern.clone(),
            },
            WebSearchAction::Other => CoreWebSearchAction::Other,
        };
        let query = web_search_action_detail(&legacy_action);
        call.turn_item_emitter
            .emit_completed(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: query.clone(),
                    action: Some(command_action),
                    results: results.clone(),
                },
                EventMsg::WebSearchEnd(WebSearchEndEvent {
                    call_id: call.call_id.clone(),
                    query,
                    action: legacy_action,
                    results,
                }),
            ))
            .await;

        Ok(Box::new(SearchOutput::new(output)))
    }

    async fn handle_tinyfish_call(
        &self,
        call: ToolCall<'_>,
        http_client_factory: &codex_http_client::HttpClientFactory,
        endpoint: &Url,
        api_key: Option<&codex_utils_redacted_string::RedactedString>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let commands = parse_tinyfish_commands(&call)?;
        let requests = prepare_tinyfish_requests(&commands, &self.settings)?;
        let api_key = api_key.ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "TinyFish web search requires TINYFISH_API_KEY".to_string(),
            )
        })?;
        let client = TinyFishSearchClient::new(
            http_client_factory.clone(),
            endpoint.clone(),
            api_key.clone(),
        )
        .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;

        call.turn_item_emitter
            .emit_started(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: String::new(),
                    action: None,
                    results: None,
                },
                EventMsg::WebSearchBegin(WebSearchBeginEvent {
                    call_id: call.call_id.clone(),
                }),
            ))
            .await;

        let mut responses = Vec::with_capacity(requests.len());
        for request in &requests {
            let mut response = client
                .search(request)
                .await
                .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
            response.query = request.query.clone();
            responses.push(response);
        }
        let formatted = format_tinyfish_output(responses, commands.response_length)?;
        record_results_payload_bytes(&formatted.results);

        let (query, queries) = match requests.as_slice() {
            [request] => (Some(request.query.clone()), None),
            requests => (
                None,
                Some(
                    requests
                        .iter()
                        .map(|request| request.query.clone())
                        .collect(),
                ),
            ),
        };
        let command_action = WebSearchAction::Search {
            query: query.clone(),
            queries: queries.clone(),
        };
        let legacy_action = CoreWebSearchAction::Search { query, queries };
        let query = web_search_action_detail(&legacy_action);
        call.turn_item_emitter
            .emit_completed(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: query.clone(),
                    action: Some(command_action),
                    results: Some(formatted.results.clone()),
                },
                EventMsg::WebSearchEnd(WebSearchEndEvent {
                    call_id: call.call_id.clone(),
                    query,
                    action: legacy_action,
                    results: Some(formatted.results),
                }),
            ))
            .await;

        Ok(Box::new(SearchOutput::new(formatted.output)))
    }
}

fn record_results_payload_bytes(results: &[serde_json::Value]) {
    if let Some(metrics) = codex_otel::global()
        && let Ok(payload) = serde_json::to_vec(results)
    {
        let payload_bytes = i64::try_from(payload.len()).unwrap_or(i64::MAX);
        let _ = metrics.histogram(RESULTS_PAYLOAD_BYTES_METRIC, payload_bytes, &[]);
    }
}

fn parse_tinyfish_commands(call: &ToolCall<'_>) -> Result<TinyFishCommands, FunctionCallError> {
    serde_json::from_str(call.function_arguments()?)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn prepare_tinyfish_requests(
    commands: &TinyFishCommands,
    settings: &SearchSettings,
) -> Result<Vec<TinyFishSearchRequest>, FunctionCallError> {
    if !(1..=4).contains(&commands.search_query.len()) {
        return Err(FunctionCallError::RespondToModel(
            "TinyFish web search accepts one to four queries".to_string(),
        ));
    }

    let configured_domains = settings
        .filters
        .as_ref()
        .and_then(|filters| filters.allowed_domains.as_deref());
    let location = settings.user_location.as_ref().and_then(|location| {
        let parts = [
            location.city.as_deref(),
            location.region.as_deref(),
            location.country.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
        (!parts.is_empty()).then(|| parts.join(", "))
    });

    commands
        .search_query
        .iter()
        .map(|query| {
            let query_text = query.q.trim();
            if query_text.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "TinyFish web search queries must not be empty".to_string(),
                ));
            }
            let domains = effective_domains(configured_domains, query.domains.as_deref())?;
            Ok(TinyFishSearchRequest {
                query: query_text.to_string(),
                domains,
                recency_days: query.recency,
                location: location.clone(),
            })
        })
        .collect()
}

fn effective_domains(
    configured_domains: Option<&[String]>,
    requested_domains: Option<&[String]>,
) -> Result<Option<Vec<String>>, FunctionCallError> {
    match (configured_domains, requested_domains) {
        (Some(configured), Some(requested)) => {
            let domains = configured
                .iter()
                .filter(|configured_domain| {
                    requested.iter().any(|requested_domain| {
                        configured_domain.eq_ignore_ascii_case(requested_domain)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if domains.is_empty() {
                Err(FunctionCallError::RespondToModel(
                    "requested domains do not overlap the configured TinyFish allowlist"
                        .to_string(),
                ))
            } else {
                Ok(Some(domains))
            }
        }
        (Some(configured), None) => Ok(Some(configured.to_vec())),
        (None, Some(requested)) => Ok(Some(requested.to_vec())),
        (None, None) => Ok(None),
    }
}

struct TinyFishFormattedOutput {
    output: String,
    results: Vec<serde_json::Value>,
}

fn format_tinyfish_output(
    mut responses: Vec<TinyFishSearchResponse>,
    response_length: Option<SearchResponseLength>,
) -> Result<TinyFishFormattedOutput, FunctionCallError> {
    let result_limit = match response_length {
        Some(SearchResponseLength::Short) => 5,
        Some(SearchResponseLength::Medium | SearchResponseLength::Long) | None => 10,
    };
    for response in &mut responses {
        response.results.truncate(result_limit);
    }
    let results = responses
        .iter()
        .flat_map(|response| response.results.iter())
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
    let searches = responses
        .into_iter()
        .map(|response| {
            serde_json::json!({
                "query": response.query,
                "results": response.results,
            })
        })
        .collect::<Vec<_>>();
    let output = serde_json::to_string_pretty(&serde_json::json!({
        "provider": "tinyfish",
        "searches": searches,
    }))
    .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
    Ok(TinyFishFormattedOutput { output, results })
}

fn search_request_headers(originator: Option<&str>, turn_metadata: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(turn_metadata) = turn_metadata
        && let Ok(header_value) = HeaderValue::from_str(turn_metadata)
    {
        headers.insert(X_CODEX_TURN_METADATA_HEADER, header_value);
    }

    if let Some(originator) = originator {
        add_originator_header(&mut headers, originator);
    }
    headers
}

fn parse_commands(call: &ToolCall<'_>) -> Result<SearchCommands, FunctionCallError> {
    let arguments = call.function_arguments()?;
    if arguments.trim().is_empty() {
        return Ok(SearchCommands::default());
    }

    serde_json::from_str(arguments)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn command_action(commands: &SearchCommands) -> WebSearchAction {
    commands
        .search_query
        .as_deref()
        .and_then(query_action)
        .or_else(|| commands.image_query.as_deref().and_then(query_action))
        .or_else(|| {
            commands
                .open
                .as_deref()
                .and_then(|operations| operations.first())
                .and_then(|operation| {
                    literal_url(&operation.ref_id)
                        .map(|url| WebSearchAction::OpenPage { url: Some(url) })
                })
        })
        .or_else(|| {
            commands
                .find
                .as_deref()
                .and_then(|operations| operations.first())
                .map(|operation| WebSearchAction::FindInPage {
                    url: literal_url(&operation.ref_id),
                    pattern: Some(operation.pattern.clone()),
                })
        })
        .unwrap_or(WebSearchAction::Other)
}

fn query_action(queries: &[SearchQuery]) -> Option<WebSearchAction> {
    match queries {
        [] => None,
        [query] => Some(WebSearchAction::Search {
            query: Some(query.q.clone()),
            queries: None,
        }),
        queries => Some(WebSearchAction::Search {
            query: None,
            queries: Some(queries.iter().map(|query| query.q.clone()).collect()),
        }),
    }
}

fn literal_url(ref_id: &str) -> Option<String> {
    Url::parse(ref_id).is_ok().then(|| ref_id.to_string())
}

fn extension_turn_item(item: WebSearchItem, legacy_event: EventMsg) -> ExtensionTurnItem {
    ExtensionTurnItem {
        item: ExtensionItem::WebSearch(item),
        legacy_events: vec![legacy_event],
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use codex_api::ApproximateLocation;
    use codex_api::LocationType;
    use codex_api::SearchCommands;
    use codex_api::SearchFilters;
    use codex_api::SearchQuery;
    use codex_api::SearchResponseLength;
    use codex_api::SearchSettings;
    use codex_extension_api::ConversationHistory;
    use codex_extension_api::ExtensionTurnItem;
    use codex_extension_api::FunctionCallError;
    use codex_extension_api::ToolCall;
    use codex_extension_api::ToolCallSource;
    use codex_extension_api::ToolName;
    use codex_extension_api::ToolOutput;
    use codex_extension_api::ToolPayload;
    use codex_extension_api::TurnItemEmissionFuture;
    use codex_extension_api::TurnItemEmitter;
    use codex_extension_items::ExtensionItem;
    use codex_extension_items::web_search::WebSearchAction;
    use codex_extension_items::web_search::WebSearchItem;
    use codex_http_client::HttpClientFactory;
    use codex_http_client::OutboundProxyPolicy;
    use codex_login::AuthManager;
    use codex_login::CodexAuth;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::FunctionCallOutputContentItem;
    use codex_protocol::models::ResponseInputItem;
    use codex_protocol::models::ResponseItem;
    use codex_protocol::protocol::EventMsg;
    use codex_protocol::protocol::TruncationPolicy;
    use codex_utils_redacted_string::RedactedString;
    use pretty_assertions::assert_eq;
    use url::Url;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    use super::WebSearchTool;
    use super::command_action;
    use super::format_tinyfish_output;
    use super::prepare_tinyfish_requests;
    use super::search_request_headers;
    use crate::extension::WebSearchBackend;
    use crate::schema::TinyFishCommands;
    use crate::tinyfish::TinyFishSearchResponse;
    use crate::tinyfish::TinyFishSearchResult;
    use codex_core::X_CODEX_TURN_METADATA_HEADER;

    #[test]
    fn search_request_headers_forward_thread_originator_and_turn_metadata() {
        let headers = search_request_headers(Some("chatgpt_cca"), Some("turn-metadata"));
        assert_eq!(
            headers
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some("chatgpt_cca")
        );
        assert_eq!(
            headers
                .get(X_CODEX_TURN_METADATA_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("turn-metadata")
        );
    }

    #[test]
    fn command_action_reports_queries_and_navigation_detail() {
        let cases = [
            (
                r#"{"image_query":[{"q":"waterfalls"},{"q":"mountains"}]}"#,
                WebSearchAction::Search {
                    query: None,
                    queries: Some(vec!["waterfalls".to_string(), "mountains".to_string()]),
                },
            ),
            (
                r#"{"open":[{"ref_id":"https://example.com/docs"}]}"#,
                WebSearchAction::OpenPage {
                    url: Some("https://example.com/docs".to_string()),
                },
            ),
            (
                r#"{"find":[{"ref_id":"https://example.com/docs","pattern":"install"}]}"#,
                WebSearchAction::FindInPage {
                    url: Some("https://example.com/docs".to_string()),
                    pattern: Some("install".to_string()),
                },
            ),
            (
                r#"{"find":[{"ref_id":"turn0search0","pattern":"install"}]}"#,
                WebSearchAction::FindInPage {
                    url: None,
                    pattern: Some("install".to_string()),
                },
            ),
            (
                r#"{"open":[{"ref_id":"turn0search0"}]}"#,
                WebSearchAction::Other,
            ),
        ];

        for (arguments, expected) in cases {
            let commands: SearchCommands =
                serde_json::from_str(arguments).expect("valid search command arguments");
            assert_eq!(command_action(&commands), expected);
        }
    }

    #[test]
    fn tinyfish_tool_rejects_empty_or_excessive_query_batches() {
        for search_query in [
            Vec::new(),
            (0..5)
                .map(|index| search_query(&format!("query-{index}"), None))
                .collect(),
        ] {
            let error = prepare_tinyfish_requests(
                &TinyFishCommands {
                    search_query,
                    response_length: None,
                },
                &SearchSettings::default(),
            )
            .expect_err("invalid batch should be rejected");
            assert!(matches!(error, FunctionCallError::RespondToModel(_)));
        }

        let error = prepare_tinyfish_requests(
            &TinyFishCommands {
                search_query: vec![search_query("   ", None)],
                response_length: None,
            },
            &SearchSettings::default(),
        )
        .expect_err("blank query should be rejected");
        assert!(matches!(error, FunctionCallError::RespondToModel(_)));
    }

    #[test]
    fn tinyfish_tool_rejects_unsupported_commands() {
        let error = serde_json::from_str::<TinyFishCommands>(
            r#"{"search_query":[{"q":"rust"}],"open":[{"ref_id":"https://example.com"}]}"#,
        )
        .expect_err("TinyFish should reject commands outside search_query");

        assert!(error.to_string().contains("unknown field `open`"));
    }

    #[test]
    fn tinyfish_tool_intersects_domains_case_insensitively_in_configured_order() {
        let settings = SearchSettings {
            filters: Some(SearchFilters {
                allowed_domains: Some(vec![
                    "docs.rs".to_string(),
                    "DOC.RUST-LANG.ORG".to_string(),
                    "example.com".to_string(),
                ]),
                blocked_domains: None,
            }),
            ..Default::default()
        };
        let requests = prepare_tinyfish_requests(
            &TinyFishCommands {
                search_query: vec![search_query(
                    "rust async traits",
                    Some(&["doc.rust-lang.org", "attacker.example", "DOCS.RS"]),
                )],
                response_length: None,
            },
            &settings,
        )
        .expect("overlapping domains should be accepted");

        assert_eq!(
            requests[0].domains,
            Some(vec!["docs.rs".to_string(), "DOC.RUST-LANG.ORG".to_string(),])
        );
    }

    #[test]
    fn tinyfish_tool_rejects_empty_domain_intersection() {
        let settings = SearchSettings {
            filters: Some(SearchFilters {
                allowed_domains: Some(vec!["docs.rs".to_string()]),
                blocked_domains: None,
            }),
            ..Default::default()
        };

        let error = prepare_tinyfish_requests(
            &TinyFishCommands {
                search_query: vec![search_query(
                    "rust async traits",
                    Some(&["attacker.example"]),
                )],
                response_length: None,
            },
            &settings,
        )
        .expect_err("non-overlapping domains should be rejected before execution");

        assert!(matches!(error, FunctionCallError::RespondToModel(_)));
    }

    #[test]
    fn tinyfish_tool_short_returns_five_results_and_other_lengths_return_ten() {
        let response = tinyfish_response("rust", 12);
        for (response_length, expected_count) in [
            (Some(SearchResponseLength::Short), 5),
            (Some(SearchResponseLength::Medium), 10),
            (Some(SearchResponseLength::Long), 10),
            (None, 10),
        ] {
            let formatted = format_tinyfish_output(vec![response.clone()], response_length)
                .expect("response should format");
            let output: serde_json::Value =
                serde_json::from_str(&formatted.output).expect("valid output JSON");
            assert_eq!(
                output["searches"][0]["results"].as_array().map(Vec::len),
                Some(expected_count)
            );
            assert_eq!(formatted.results.len(), expected_count);
        }
    }

    #[test]
    fn tinyfish_tool_preserves_query_groups_ranking_and_privacy() {
        const API_KEY: &str = "tf-secret-key";
        const HISTORY_FIXTURE: &str = "private conversation history";
        let formatted = format_tinyfish_output(
            vec![
                tinyfish_response("first query", 2),
                tinyfish_response("second query", 2),
            ],
            None,
        )
        .expect("responses should format");
        let output: serde_json::Value =
            serde_json::from_str(&formatted.output).expect("valid output JSON");

        assert_eq!(
            output,
            serde_json::json!({
                "provider": "tinyfish",
                "searches": [
                    {
                        "query": "first query",
                        "results": [
                            result_json("first query", 1),
                            result_json("first query", 2),
                        ],
                    },
                    {
                        "query": "second query",
                        "results": [
                            result_json("second query", 1),
                            result_json("second query", 2),
                        ],
                    },
                ],
            })
        );
        assert_eq!(
            formatted.results,
            vec![
                result_json("first query", 1),
                result_json("first query", 2),
                result_json("second query", 1),
                result_json("second query", 2),
            ]
        );
        assert!(!formatted.output.contains(API_KEY));
        assert!(!formatted.output.contains(HISTORY_FIXTURE));
    }

    #[tokio::test]
    async fn tinyfish_tool_missing_key_responds_to_model_without_http() {
        let server = MockServer::start().await;
        let tool = tinyfish_tool(&server, None, SearchSettings::default());

        let result = tool
            .handle_call(tool_call(
                serde_json::json!({"search_query": [{"q": "rust"}]}),
                ConversationHistory::default(),
                Arc::new(RecordingTurnItemEmitter::default()),
            ))
            .await;
        let Err(error) = result else {
            panic!("missing API key should be model-visible");
        };

        assert_eq!(
            error,
            FunctionCallError::RespondToModel(
                "TinyFish web search requires TINYFISH_API_KEY".to_string()
            )
        );
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded requests should be available")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn tinyfish_tool_empty_domain_intersection_responds_without_http() {
        let server = MockServer::start().await;
        let settings = SearchSettings {
            filters: Some(SearchFilters {
                allowed_domains: Some(vec!["docs.rs".to_string()]),
                blocked_domains: None,
            }),
            ..Default::default()
        };
        let tool = tinyfish_tool(&server, Some("test-key"), settings);

        let result = tool
            .handle_call(tool_call(
                serde_json::json!({
                    "search_query": [{
                        "q": "rust",
                        "domains": ["attacker.example"],
                    }],
                }),
                ConversationHistory::default(),
                Arc::new(RecordingTurnItemEmitter::default()),
            ))
            .await;
        let Err(error) = result else {
            panic!("empty domain intersection should be model-visible");
        };

        assert!(matches!(error, FunctionCallError::RespondToModel(_)));
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded requests should be available")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn tinyfish_tool_sends_only_search_parameters_and_emits_persistable_lifecycle() {
        const API_KEY: &str = "tf-private-api-key";
        const HISTORY_FIXTURE: &str = "private conversation history";
        const TURN_METADATA: &str = "private turn metadata";
        const ORIGINATOR: &str = "private originator";
        let server = MockServer::start().await;
        mount_tinyfish_response(&server, "rust async traits", "provider changed query", 1).await;
        mount_tinyfish_response(&server, "tokio tasks", "provider changed query again", 2).await;
        let settings = SearchSettings {
            filters: Some(SearchFilters {
                allowed_domains: Some(vec!["docs.rs".to_string(), "example.com".to_string()]),
                blocked_domains: None,
            }),
            user_location: Some(ApproximateLocation {
                r#type: LocationType::Approximate,
                country: Some("US".to_string()),
                region: Some("NY".to_string()),
                city: Some("New York".to_string()),
                timezone: Some("America/New_York".to_string()),
            }),
            ..Default::default()
        };
        let mut tool = tinyfish_tool(&server, Some(API_KEY), settings);
        tool.originator = Some(ORIGINATOR.to_string());
        let emitter = Arc::new(RecordingTurnItemEmitter::default());
        let history = ConversationHistory::new(vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: HISTORY_FIXTURE.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }]);
        let payload = ToolPayload::Function {
            arguments: serde_json::json!({
                "search_query": [
                    {
                        "q": "rust async traits",
                        "domains": ["DOCS.RS", "attacker.example"],
                        "recency": 2,
                    },
                    {
                        "q": "tokio tasks",
                        "domains": ["docs.rs"],
                        "recency": 2,
                    },
                ],
                "response_length": "short",
            })
            .to_string(),
        };
        let mut call = tool_call(
            serde_json::Value::Null,
            history,
            Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
        );
        call.codex_turn_metadata = Some(TURN_METADATA.to_string());
        call.payload = payload.clone();

        let output = tool
            .handle_call(call)
            .await
            .expect("TinyFish search should succeed");

        assert!(output.contains_external_context());
        let response_item = output.to_response_item("call-1", &payload);
        let ResponseInputItem::FunctionCallOutput { output, .. } = response_item else {
            panic!("web search should return function call output");
        };
        let [FunctionCallOutputContentItem::InputText { text }] =
            output.content_items().expect("output should contain text")
        else {
            panic!("web search should return one text item");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(text).expect("valid JSON output"),
            serde_json::json!({
                "provider": "tinyfish",
                "searches": [
                    {
                        "query": "rust async traits",
                        "results": [result_json("rust async traits", 1)],
                    },
                    {
                        "query": "tokio tasks",
                        "results": [result_json("tokio tasks", 2)],
                    },
                ],
            })
        );
        for private in [API_KEY, HISTORY_FIXTURE, TURN_METADATA, ORIGINATOR] {
            assert!(!text.contains(private));
        }

        let requests = server
            .received_requests()
            .await
            .expect("recorded requests should be available");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .iter()
                .map(|request| {
                    request
                        .url
                        .query_pairs()
                        .map(|(key, value)| (key.into_owned(), value.into_owned()))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
            vec![
                vec![
                    ("query".to_string(), "rust async traits".to_string()),
                    ("include_domains".to_string(), "docs.rs".to_string()),
                    ("recency_minutes".to_string(), "2880".to_string()),
                    ("location".to_string(), "New York, NY, US".to_string()),
                ],
                vec![
                    ("query".to_string(), "tokio tasks".to_string()),
                    ("include_domains".to_string(), "docs.rs".to_string()),
                    ("recency_minutes".to_string(), "2880".to_string()),
                    ("location".to_string(), "New York, NY, US".to_string()),
                ],
            ]
        );
        for request in &requests {
            assert_eq!(
                request
                    .headers
                    .get("X-API-Key")
                    .and_then(|value| value.to_str().ok()),
                Some(API_KEY)
            );
            assert!(!request.url.as_str().contains(API_KEY));
            assert!(!String::from_utf8_lossy(&request.body).contains(API_KEY));
            for private in [HISTORY_FIXTURE, TURN_METADATA, ORIGINATOR] {
                assert!(!request.url.as_str().contains(private));
                assert!(!String::from_utf8_lossy(&request.body).contains(private));
            }
            assert!(request.headers.get("x-codex-turn-metadata").is_none());
            assert!(request.headers.get("originator").is_none());
        }

        let started = emitter.started.lock().expect("started lock");
        assert_eq!(started.len(), 1);
        assert!(matches!(
            started[0].legacy_events.as_slice(),
            [EventMsg::WebSearchBegin(event)] if event.call_id == "call-1"
        ));
        let completed = emitter.completed.lock().expect("completed lock");
        assert_eq!(completed.len(), 1);
        let ExtensionItem::WebSearch(item) = &completed[0].item else {
            panic!("completed item should be persistable web search data");
        };
        assert_eq!(
            item,
            &WebSearchItem {
                id: "call-1".to_string(),
                query: "rust async traits ...".to_string(),
                action: Some(WebSearchAction::Search {
                    query: None,
                    queries: Some(vec![
                        "rust async traits".to_string(),
                        "tokio tasks".to_string(),
                    ]),
                }),
                results: Some(vec![
                    result_json("rust async traits", 1),
                    result_json("tokio tasks", 2),
                ]),
            }
        );
        assert!(matches!(
            completed[0].legacy_events.as_slice(),
            [EventMsg::WebSearchEnd(event)]
                if event.call_id == "call-1"
                    && event.results.as_ref().is_some_and(|results| results.len() == 2)
        ));
    }

    fn search_query(q: &str, domains: Option<&[&str]>) -> SearchQuery {
        SearchQuery {
            q: q.to_string(),
            recency: None,
            domains: domains
                .map(|domains| domains.iter().map(|domain| (*domain).to_string()).collect()),
        }
    }

    fn tinyfish_response(query: &str, result_count: usize) -> TinyFishSearchResponse {
        TinyFishSearchResponse {
            query: query.to_string(),
            results: (1..=result_count)
                .map(|position| TinyFishSearchResult {
                    position: position as u64,
                    site_name: format!("site-{position}"),
                    title: format!("{query} title {position}"),
                    snippet: format!("{query} snippet {position}"),
                    url: format!("https://example.com/{query}/{position}"),
                })
                .collect(),
            total_results: result_count as u64,
            page: 1,
        }
    }

    fn result_json(query: &str, position: u64) -> serde_json::Value {
        serde_json::json!({
            "position": position,
            "site_name": format!("site-{position}"),
            "title": format!("{query} title {position}"),
            "snippet": format!("{query} snippet {position}"),
            "url": format!("https://example.com/{query}/{position}"),
        })
    }

    fn tinyfish_tool(
        server: &MockServer,
        api_key: Option<&str>,
        settings: SearchSettings,
    ) -> WebSearchTool {
        WebSearchTool {
            session_id: "session-1".to_string(),
            backend: WebSearchBackend::Tinyfish {
                http_client_factory: HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
                endpoint: Url::parse(&server.uri()).expect("valid mock endpoint"),
                api_key: api_key.map(RedactedString::from),
            },
            auth_manager: AuthManager::from_auth_for_testing(CodexAuth::from_api_key("dummy")),
            settings,
            originator: None,
        }
    }

    fn tool_call(
        arguments: serde_json::Value,
        conversation_history: ConversationHistory,
        turn_item_emitter: Arc<dyn TurnItemEmitter>,
    ) -> ToolCall<'static> {
        ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: ToolName::namespaced("web", "run"),
            model: "gpt-test".to_string(),
            codex_turn_metadata: None,
            truncation_policy: TruncationPolicy::Bytes(10_000),
            source: ToolCallSource::Direct,
            conversation_history,
            turn_item_emitter,
            environments: Vec::new(),
            payload: ToolPayload::Function {
                arguments: arguments.to_string(),
            },
        }
    }

    async fn mount_tinyfish_response(
        server: &MockServer,
        query: &str,
        response_query: &str,
        position: u64,
    ) {
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("X-API-Key", "tf-private-api-key"))
            .and(query_param("query", query))
            .and(query_param("include_domains", "docs.rs"))
            .and(query_param("recency_minutes", "2880"))
            .and(query_param("location", "New York, NY, US"))
            .respond_with(
                ResponseTemplate::new(/*status*/ 200).set_body_json(serde_json::json!({
                    "query": response_query,
                    "results": [result_json(query, position)],
                    "total_results": 1,
                    "page": 0,
                })),
            )
            .mount(server)
            .await;
    }

    #[derive(Default)]
    struct RecordingTurnItemEmitter {
        started: Mutex<Vec<ExtensionTurnItem>>,
        completed: Mutex<Vec<ExtensionTurnItem>>,
    }

    impl TurnItemEmitter for RecordingTurnItemEmitter {
        fn emit_started<'a>(&'a self, item: ExtensionTurnItem) -> TurnItemEmissionFuture<'a> {
            self.started.lock().expect("started lock").push(item);
            Box::pin(std::future::ready(()))
        }

        fn emit_completed<'a>(&'a self, item: ExtensionTurnItem) -> TurnItemEmissionFuture<'a> {
            self.completed.lock().expect("completed lock").push(item);
            Box::pin(std::future::ready(()))
        }
    }
}
