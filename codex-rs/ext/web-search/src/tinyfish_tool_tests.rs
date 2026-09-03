use std::sync::Arc;
use std::sync::Mutex;

use codex_api::ApproximateLocation;
use codex_api::LocationType;
use codex_api::SearchFilters;
use codex_api::SearchSettings;
use codex_extension_api::ConversationHistory;
use codex_extension_api::ExtensionTurnItem;
use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolCallSource;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolNetworkEgress;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolPayload;
use codex_extension_api::ToolSpec;
use codex_extension_api::TurnItemEmissionFuture;
use codex_extension_api::TurnItemEmitter;
use codex_extension_items::ExtensionItem;
use codex_extension_items::web_search::WebSearchAction;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TruncationPolicy;
use codex_tools::ResponsesApiNamespaceTool;
use pretty_assertions::assert_eq;
use url::Url;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

use crate::extension::WebSearchBackend;
use crate::tool::WebSearchTool;

const API_KEY: &str = "tf-private-api-key";
const MAX_SERIALIZED_ITEM_BYTES: usize = 10_000;

const UNRECOGNIZED_API_KEY: &str = "x7Qp4mN9vL2sR8tW";

#[test]
fn tinyfish_declares_the_exact_reviewable_test_egress() {
    let endpoint = Url::parse("http://127.0.0.1:43127/search").expect("valid test endpoint");
    let settings = SearchSettings {
        filters: Some(SearchFilters {
            allowed_domains: Some(vec!["docs.rs".to_string()]),
            blocked_domains: None,
        }),
        user_location: Some(ApproximateLocation {
            r#type: LocationType::Approximate,
            country: Some("US".to_string()),
            region: None,
            city: None,
            timezone: None,
        }),
        ..Default::default()
    };
    let tool = tinyfish_tool(endpoint, Some(API_KEY), settings);
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({
            "search_query": [{
                "q": " rust async traits ",
                "domains": ["docs.rs", "blocked.example"],
                "recency": 2
            }]
        })
        .to_string(),
    };

    let egress = tool
        .network_egress(&payload)
        .expect("TinyFish egress declaration should be valid")
        .expect("TinyFish should declare network egress");

    assert_eq!(
        egress,
        ToolNetworkEgress {
            protocol: NetworkApprovalProtocol::Http,
            host: "127.0.0.1".to_string(),
            port: 43_127,
            review_command: vec![
                "web.run".to_string(),
                r#"[{"query":"rust async traits","include_domains":"docs.rs","recency_minutes":2880,"location":"US"}]"#
                    .to_string(),
            ],
        }
    );
    assert!(!egress.review_command.join(" ").contains(API_KEY));
}

#[test]
fn tinyfish_requires_an_api_key_before_declaring_egress() {
    let endpoint = Url::parse("http://127.0.0.1:43127/search").expect("valid test endpoint");
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({"search_query": [{"q": "rust"}]}).to_string(),
    };

    for api_key in [None, Some("   ")] {
        let tool = tinyfish_tool(endpoint.clone(), api_key, SearchSettings::default());
        assert!(matches!(
            tool.network_egress(&payload),
            Err(FunctionCallError::RespondToModel(message))
                if message == "TinyFish web search requires TINYFISH_API_KEY"
        ));
    }
}

#[test]
fn tinyfish_rejects_unreviewable_egress_payloads() {
    let tool = tinyfish_tool(
        Url::parse("http://127.0.0.1:1").expect("valid test endpoint"),
        Some(API_KEY),
        SearchSettings::default(),
    );
    let oversized = ToolPayload::Function {
        arguments: serde_json::json!({
            "search_query": [{"q": "x".repeat(9_000)}]
        })
        .to_string(),
    };

    assert!(matches!(
        tool.network_egress(&oversized),
        Err(FunctionCallError::RespondToModel(message))
            if message == "TinyFish web search request is too large for security review"
    ));

    let secret_domain = ToolPayload::Function {
        arguments: serde_json::json!({
            "search_query": [{
                "q": "rust",
                "domains": ["token=abcdefghijklmnop"]
            }]
        })
        .to_string(),
    };
    assert!(matches!(
        tool.network_egress(&secret_domain),
        Err(FunctionCallError::RespondToModel(message))
            if message == "TinyFish web search queries must not contain credentials or secrets"
    ));
}

#[test]
fn tinyfish_backend_exposes_only_the_search_query_contract() {
    let tool = tinyfish_tool(
        Url::parse("http://127.0.0.1:1").expect("valid test endpoint"),
        Some(API_KEY),
        SearchSettings::default(),
    );

    let ToolSpec::Namespace(namespace) = tool.spec() else {
        panic!("TinyFish should expose the web namespace");
    };
    let [ResponsesApiNamespaceTool::Function(function)] = namespace.tools.as_slice() else {
        panic!("TinyFish should expose one function");
    };
    let properties =
        serde_json::to_value(&function.parameters).expect("schema should serialize")["properties"]
            .as_object()
            .expect("schema properties should be an object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();

    assert_eq!(properties, vec!["response_length", "search_query"]);
    assert!(function.description.len() < 1_024);
    for unsupported_operation in [
        "image_query",
        "open",
        "click",
        "find",
        "screenshot",
        "finance",
        "weather",
        "sports",
        "time",
    ] {
        assert!(
            !function.description.contains(unsupported_operation),
            "TinyFish description should not advertise {unsupported_operation}"
        );
    }
}

#[tokio::test]
async fn invalid_tinyfish_inputs_fail_before_http_or_lifecycle_events() {
    let server = MockServer::start().await;
    let tool = tinyfish_tool(
        Url::parse(&server.uri()).expect("valid mock endpoint"),
        Some(API_KEY),
        SearchSettings::default(),
    );
    let emitter = Arc::new(RecordingTurnItemEmitter::default());
    let cases = [
        serde_json::json!({"open": [{"ref_id": "https://example.com"}]}),
        serde_json::json!({"search_query": []}),
        serde_json::json!({"search_query": [
            {"q": "one"}, {"q": "two"}, {"q": "three"}, {"q": "four"}, {"q": "five"}
        ]}),
        serde_json::json!({"search_query": [{"q": "  "}]}),
        serde_json::json!({"search_query": [{"q": "rust", "recency": 0}]}),
        serde_json::json!({"search_query": [{"q": "rust", "recency": 3_651}]}),
        serde_json::json!({"search_query": [{"q": "rust", "recency": u64::MAX}]}),
    ];

    for arguments in cases {
        let result = tool
            .handle(tool_call(
                arguments,
                ConversationHistory::default(),
                Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
            ))
            .await;
        assert!(matches!(result, Err(FunctionCallError::RespondToModel(_))));
    }

    assert!(received_requests(&server).await.is_empty());
    assert!(emitter.started.lock().expect("started lock").is_empty());
    assert!(emitter.completed.lock().expect("completed lock").is_empty());
}

#[tokio::test]
async fn configured_tinyfish_key_is_rejected_from_every_outbound_field_before_side_effects() {
    let endpoint = Url::parse("http://127.0.0.1:1").expect("valid unreachable test endpoint");
    let cases = [
        (
            SearchSettings::default(),
            serde_json::json!({
                "search_query": [{"q": format!("find {UNRECOGNIZED_API_KEY}")}]
            }),
        ),
        (
            SearchSettings::default(),
            serde_json::json!({
                "search_query": [{
                    "q": "rust",
                    "domains": [format!("docs.{UNRECOGNIZED_API_KEY}.example")]
                }]
            }),
        ),
        (
            SearchSettings {
                user_location: Some(ApproximateLocation {
                    r#type: LocationType::Approximate,
                    country: Some(format!("US-{UNRECOGNIZED_API_KEY}")),
                    region: None,
                    city: None,
                    timezone: None,
                }),
                ..Default::default()
            },
            serde_json::json!({"search_query": [{"q": "rust"}]}),
        ),
    ];

    for (settings, arguments) in cases {
        let tool = tinyfish_tool(endpoint.clone(), Some(UNRECOGNIZED_API_KEY), settings);
        let payload = ToolPayload::Function {
            arguments: arguments.to_string(),
        };
        let egress_error = tool
            .network_egress(&payload)
            .expect_err("the configured API key must not enter the review command");
        assert_eq!(
            egress_error,
            FunctionCallError::RespondToModel(
                "TinyFish web search queries must not contain credentials or secrets".to_string()
            )
        );
        assert!(!egress_error.to_string().contains(UNRECOGNIZED_API_KEY));

        let emitter = Arc::new(RecordingTurnItemEmitter::default());
        let result = tool
            .handle(tool_call(
                arguments,
                ConversationHistory::default(),
                Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
            ))
            .await;
        let Err(error) = result else {
            panic!("the configured API key must not be sent upstream");
        };
        assert_eq!(
            error,
            FunctionCallError::RespondToModel(
                "TinyFish web search queries must not contain credentials or secrets".to_string()
            )
        );
        assert!(!error.to_string().contains(UNRECOGNIZED_API_KEY));
        assert!(emitter.started.lock().expect("started lock").is_empty());
        assert!(emitter.completed.lock().expect("completed lock").is_empty());
    }
}

#[tokio::test]
async fn configured_tinyfish_key_is_rejected_after_outbound_transformations_before_side_effects() {
    let server = MockServer::start().await;
    let cases = [
        (
            "foo,bar",
            serde_json::json!({
                "search_query": [{"q": "rust", "domains": ["foo", "bar"]}]
            }),
        ),
        (
            r#"foo","bar"#,
            serde_json::json!({
                "search_query": [{"q": "rust", "domains": ["foo", "bar"]}]
            }),
        ),
        (
            "2880",
            serde_json::json!({
                "search_query": [{"q": "rust", "recency": 2}]
            }),
        ),
    ];

    for (api_key, arguments) in cases {
        let tool = tinyfish_tool(
            Url::parse(&server.uri()).expect("valid mock endpoint"),
            Some(api_key),
            SearchSettings::default(),
        );
        let payload = ToolPayload::Function {
            arguments: arguments.to_string(),
        };
        let egress_error = tool
            .network_egress(&payload)
            .expect_err("the transformed API key must not enter the review command");
        assert_eq!(
            egress_error,
            FunctionCallError::RespondToModel(
                "TinyFish web search queries must not contain credentials or secrets".to_string()
            )
        );

        let emitter = Arc::new(RecordingTurnItemEmitter::default());
        let result = tool
            .handle(tool_call(
                arguments,
                ConversationHistory::default(),
                Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
            ))
            .await;
        let Err(error) = result else {
            panic!("the transformed API key must not be sent upstream");
        };
        assert_eq!(
            error,
            FunctionCallError::RespondToModel(
                "TinyFish web search queries must not contain credentials or secrets".to_string()
            )
        );
        assert!(emitter.started.lock().expect("started lock").is_empty());
        assert!(emitter.completed.lock().expect("completed lock").is_empty());
    }

    assert!(received_requests(&server).await.is_empty());
}

#[tokio::test]
async fn missing_or_blank_tinyfish_key_fails_before_http_or_events() {
    let server = MockServer::start().await;

    for api_key in [None, Some(" \t\n ")] {
        let tool = tinyfish_tool(
            Url::parse(&server.uri()).expect("valid mock endpoint"),
            api_key,
            SearchSettings::default(),
        );
        let emitter = Arc::new(RecordingTurnItemEmitter::default());
        let result = tool
            .handle(tool_call(
                serde_json::json!({"search_query": [{"q": "rust"}]}),
                ConversationHistory::default(),
                Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
            ))
            .await;

        assert!(matches!(
            result,
            Err(FunctionCallError::RespondToModel(message))
                if message == "TinyFish web search requires TINYFISH_API_KEY"
        ));
        assert!(emitter.started.lock().expect("started lock").is_empty());
        assert!(emitter.completed.lock().expect("completed lock").is_empty());
    }

    assert!(received_requests(&server).await.is_empty());
}

#[tokio::test]
async fn tinyfish_executor_sends_minimal_requests_and_emits_bounded_persistable_results() {
    const HISTORY: &str = "private conversation history";
    let server = MockServer::start().await;
    mount_response(&server, "rust async traits", 1).await;
    mount_response(&server, "tokio tasks", 2).await;
    let settings = SearchSettings {
        filters: Some(SearchFilters {
            allowed_domains: Some(vec!["docs.rs".to_string(), "example.com".to_string()]),
            blocked_domains: None,
        }),
        user_location: Some(ApproximateLocation {
            r#type: LocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("private-region".to_string()),
            city: Some("private-city".to_string()),
            timezone: Some("private/timezone".to_string()),
        }),
        ..Default::default()
    };
    let tool = tinyfish_tool(
        Url::parse(&server.uri()).expect("valid mock endpoint"),
        Some(API_KEY),
        settings,
    );
    let emitter = Arc::new(RecordingTurnItemEmitter::default());
    let history = ConversationHistory::new(vec![ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: HISTORY.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }]);
    let arguments = serde_json::json!({
        "search_query": [
            {"q": "rust async traits", "domains": ["DOCS.RS", "attacker.example"], "recency": 2},
            {"q": "tokio tasks", "domains": ["docs.rs"], "recency": 2}
        ],
        "response_length": "short"
    });

    let output = tool
        .handle(tool_call(
            arguments,
            history,
            Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
        ))
        .await
        .expect("TinyFish search should succeed");

    assert!(output.contains_external_context());
    let response_item = output.to_response_item(
        "call-1",
        &ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    );
    let ResponseInputItem::FunctionCallOutput { output, .. } = &response_item else {
        panic!("TinyFish should return function output");
    };
    let [FunctionCallOutputContentItem::InputText { text }] =
        output.content_items().expect("output should contain text")
    else {
        panic!("TinyFish should return one text item");
    };
    let parsed = serde_json::from_str::<serde_json::Value>(text).expect("valid output JSON");
    assert_eq!(
        parsed,
        serde_json::json!({
            "provider": "tinyfish",
            "searches": [
                {"query": "rust async traits", "results": [result_json("rust async traits", 1)]},
                {"query": "tokio tasks", "results": [result_json("tokio tasks", 2)]}
            ]
        })
    );
    for private in [
        API_KEY,
        HISTORY,
        "private-region",
        "private-city",
        "private/timezone",
    ] {
        assert!(!text.contains(private));
    }

    let requests = received_requests(&server).await;
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(
            request
                .headers
                .get("X-API-Key")
                .and_then(|value| value.to_str().ok()),
            Some(API_KEY)
        );
        let serialized = format!("{}{}", request.url, String::from_utf8_lossy(&request.body));
        assert!(!serialized.contains(API_KEY));
        assert!(!serialized.contains(HISTORY));
        assert!(!serialized.contains("private-city"));
        assert!(!serialized.contains("private-region"));
        assert!(!serialized.contains("private/timezone"));
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
        item.action,
        Some(WebSearchAction::Search {
            query: None,
            queries: Some(vec![
                "rust async traits".to_string(),
                "tokio tasks".to_string()
            ]),
        })
    );
    for serialized in [
        serde_json::to_vec(&response_item).expect("response item should serialize"),
        serde_json::to_vec(&completed[0].item).expect("extension item should serialize"),
        serde_json::to_vec(&completed[0].legacy_events[0]).expect("legacy event should serialize"),
    ] {
        assert!(serialized.len() <= MAX_SERIALIZED_ITEM_BYTES);
    }
}

#[tokio::test]
async fn tinyfish_provider_errors_are_safe_and_do_not_complete_the_lifecycle() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 500)
                .set_body_string(format!("provider reflected {API_KEY}")),
        )
        .mount(&server)
        .await;
    let tool = tinyfish_tool(
        Url::parse(&server.uri()).expect("valid mock endpoint"),
        Some(API_KEY),
        SearchSettings::default(),
    );
    let emitter = Arc::new(RecordingTurnItemEmitter::default());

    let result = tool
        .handle(tool_call(
            serde_json::json!({"search_query": [{"q": "rust"}]}),
            ConversationHistory::default(),
            Arc::clone(&emitter) as Arc<dyn TurnItemEmitter>,
        ))
        .await;
    let Err(error) = result else {
        panic!("provider error should fail safely");
    };

    assert!(!error.to_string().contains(API_KEY));
    assert_eq!(emitter.started.lock().expect("started lock").len(), 1);
    assert!(emitter.completed.lock().expect("completed lock").is_empty());
}

fn tinyfish_tool(endpoint: Url, api_key: Option<&str>, settings: SearchSettings) -> WebSearchTool {
    WebSearchTool {
        session_id: "session-1".to_string(),
        backend: WebSearchBackend::Tinyfish {
            runtime: crate::tinyfish_tool::test_support::runtime(
                HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
                endpoint,
                api_key.map(Into::into),
            ),
        },
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
        model: "test-model".to_string(),
        codex_turn_metadata: Some("private-turn-metadata".to_string()),
        truncation_policy: TruncationPolicy::Bytes(MAX_SERIALIZED_ITEM_BYTES),
        source: ToolCallSource::Direct,
        conversation_history,
        turn_item_emitter,
        environments: Vec::new(),
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}

async fn mount_response(server: &MockServer, query: &str, position: u64) {
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("X-API-Key", API_KEY))
        .and(query_param("query", query))
        .and(query_param("include_domains", "docs.rs"))
        .and(query_param("recency_minutes", "2880"))
        .and(query_param("location", "US"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200).set_body_json(serde_json::json!({
                "query": "provider supplied query must be ignored",
                "results": [result_json(query, position)],
                "total_results": 1,
                "page": 0
            })),
        )
        .mount(server)
        .await;
}

fn result_json(query: &str, position: u64) -> serde_json::Value {
    serde_json::json!({
        "position": position,
        "site_name": format!("site-{position}"),
        "title": format!("{query} title {position}"),
        "snippet": format!("{query} snippet {position}"),
        "url": format!("https://example.com/{query}/{position}")
    })
}

async fn received_requests(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("recorded requests should be available")
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
