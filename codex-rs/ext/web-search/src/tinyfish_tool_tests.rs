use std::sync::Arc;
use std::sync::Mutex;

use codex_api::ApproximateLocation;
use codex_api::LocationType;
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

use super::format_tinyfish_output;
use super::prepare_tinyfish_requests;
use crate::extension::WebSearchBackend;
use crate::schema::TinyFishCommands;
use crate::tinyfish::TinyFishSearchResponse;
use crate::tinyfish::TinyFishSearchResult;
use crate::tool::WebSearchTool;
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
        let formatted = format_tinyfish_output(
            vec![response.clone()],
            response_length,
            /*response_byte_budget*/ usize::MAX,
        )
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
        /*response_byte_budget*/ usize::MAX,
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
async fn tinyfish_tool_blank_key_responds_to_model_without_http() {
    let server = MockServer::start().await;
    let tool = tinyfish_tool(&server, Some(" \t\n "), SearchSettings::default());

    let result = tool
        .handle_call(tool_call(
            serde_json::json!({"search_query": [{"q": "rust"}]}),
            ConversationHistory::default(),
            Arc::new(RecordingTurnItemEmitter::default()),
        ))
        .await;
    let Err(error) = result else {
        panic!("blank API key should be model-visible");
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
async fn tinyfish_tool_recency_overflow_responds_to_model_without_http_or_events() {
    let server = MockServer::start().await;
    let tool = tinyfish_tool(&server, Some("test-key"), SearchSettings::default());
    let emitter = Arc::new(RecordingTurnItemEmitter::default());

    let result = tool
        .handle_call(tool_call(
            serde_json::json!({
                "search_query": [{"q": "rust", "recency": u64::MAX}],
            }),
            ConversationHistory::default(),
            Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
        ))
        .await;
    let Err(error) = result else {
        panic!("overflowing recency should be model-visible");
    };

    assert!(matches!(error, FunctionCallError::RespondToModel(_)));
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded requests should be available")
            .is_empty()
    );
    assert!(emitter.started.lock().expect("started lock").is_empty());
    assert!(emitter.completed.lock().expect("completed lock").is_empty());
}

#[tokio::test]
async fn tinyfish_tool_bounds_oversized_results_for_output_and_events() {
    const MAX_MODEL_CONTEXT_TOKENS: usize = 10_000;
    let server = MockServer::start().await;
    let oversized = "provider-data".repeat(10_000);
    for query in ["first query", "second query"] {
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("query", query))
            .respond_with(
                ResponseTemplate::new(/*status*/ 200).set_body_json(serde_json::json!({
                    "query": "provider query must be ignored",
                    "results": [
                        {
                            "position": 1,
                            "site_name": oversized,
                            "title": oversized,
                            "snippet": oversized,
                            "url": oversized,
                        },
                        {
                            "position": 2,
                            "site_name": oversized,
                            "title": oversized,
                            "snippet": oversized,
                            "url": oversized,
                        },
                    ],
                    "total_results": 2,
                    "page": 0,
                })),
            )
            .mount(&server)
            .await;
    }
    let tool = tinyfish_tool(&server, Some("test-key"), SearchSettings::default());
    let emitter = Arc::new(RecordingTurnItemEmitter::default());
    let mut call = tool_call(
        serde_json::json!({
            "search_query": [{"q": "first query"}, {"q": "second query"}],
        }),
        ConversationHistory::default(),
        Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
    );
    call.truncation_policy = TruncationPolicy::Tokens(50_000);

    let output = tool
        .handle_call(call)
        .await
        .expect("oversized TinyFish response should be safely bounded");
    let response_item = output.to_response_item(
        "call-1",
        &ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    );
    let ResponseInputItem::FunctionCallOutput { output, .. } = response_item else {
        panic!("web search should return function call output");
    };
    let [FunctionCallOutputContentItem::InputText { text }] =
        output.content_items().expect("output should contain text")
    else {
        panic!("web search should return one text item");
    };
    let response_budget = TruncationPolicy::Tokens(MAX_MODEL_CONTEXT_TOKENS).byte_budget();
    assert!(text.len() <= response_budget);
    let parsed: serde_json::Value =
        serde_json::from_str(text).expect("bounded output should remain valid JSON");
    assert_eq!(
        parsed["searches"]
            .as_array()
            .expect("search groups")
            .iter()
            .map(|search| {
                (
                    search["query"].as_str().expect("query"),
                    search["results"]
                        .as_array()
                        .expect("results")
                        .iter()
                        .map(|result| result["position"].as_u64().expect("position"))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
        vec![("first query", vec![1, 2]), ("second query", vec![1, 2]),]
    );
    let flattened_results = parsed["searches"]
        .as_array()
        .expect("search groups")
        .iter()
        .flat_map(|search| search["results"].as_array().expect("results"))
        .cloned()
        .collect::<Vec<_>>();

    let completed = emitter.completed.lock().expect("completed lock");
    let ExtensionItem::WebSearch(item) = &completed[0].item else {
        panic!("completed item should be web search data");
    };
    assert_eq!(item.results.as_ref(), Some(&flattened_results));
    assert!(
        serde_json::to_vec(item.results.as_ref().expect("event results"))
            .expect("event results should serialize")
            .len()
            <= response_budget
    );
    assert!(matches!(
        completed[0].legacy_events.as_slice(),
        [EventMsg::WebSearchEnd(event)] if event.results.as_ref() == Some(&flattened_results)
    ));
}

#[test]
fn tinyfish_tool_bounds_optional_metadata_without_dropping_the_result() {
    let oversized = "provider-metadata".repeat(1_000);
    let mut response = tinyfish_response("metadata", 1);
    let result = &mut response.results[0];
    result.date = Some(oversized.clone());
    result.publisher = Some(oversized.clone());
    result.authors = Some(vec![oversized.clone(), oversized.clone()]);
    result.venue = Some(oversized.clone());
    result.year = Some(2025);
    result.cited_by_count = Some(42);
    result.pdf_url = Some(oversized.clone());

    let formatted = format_tinyfish_output(
        vec![response],
        Some(SearchResponseLength::Medium),
        /*response_byte_budget*/ 1_000,
    )
    .expect("metadata-bearing result should fit the output budget");

    assert!(formatted.output.len() <= 1_000);
    let [result] = formatted.results.as_slice() else {
        panic!("metadata-bearing result should be retained");
    };
    assert_eq!(result["year"], 2025);
    assert_eq!(result["cited_by_count"], 42);
    for field in ["date", "publisher", "venue", "pdf_url"] {
        let value = result[field].as_str().expect("metadata should be a string");
        assert!(!value.is_empty());
        assert!(value.len() < oversized.len());
    }
    for author in result["authors"]
        .as_array()
        .expect("authors should remain an array")
    {
        let author = author.as_str().expect("author should be a string");
        assert!(!author.is_empty());
        assert!(author.len() < oversized.len());
    }
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
                date: None,
                publisher: None,
                authors: None,
                venue: None,
                year: None,
                cited_by_count: None,
                pdf_url: None,
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
