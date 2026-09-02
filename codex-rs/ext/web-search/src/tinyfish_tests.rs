use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
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

use crate::tinyfish::TinyFishSearchClient;
use crate::tinyfish::TinyFishSearchRequest;
use crate::tinyfish::TinyFishSearchResponse;
use crate::tinyfish::TinyFishSearchResult;

const TEST_API_KEY: &str = "test-tinyfish-key";

#[tokio::test]
async fn tinyfish_search_sends_documented_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("X-API-Key", "test-tinyfish-key"))
        .and(query_param("query", "rust async traits"))
        .and(query_param("include_domains", "doc.rust-lang.org,docs.rs"))
        .and(query_param("recency_minutes", "43200"))
        .and(query_param("location", "US"))
        .respond_with(ResponseTemplate::new(/*status*/ 200).set_body_raw(
            r#"{
                "query": "rust async traits",
                "results": [
                    {
                        "position": 1,
                        "site_name": "doc.rust-lang.org",
                        "title": "Async traits",
                        "snippet": "Native async functions in traits.",
                        "url": "https://doc.rust-lang.org/book/async-traits"
                    },
                    {
                        "position": 2,
                        "site_name": "docs.rs",
                        "title": "async-trait",
                        "snippet": "Type erasure for async trait methods.",
                        "url": "https://docs.rs/async-trait"
                    }
                ],
                "total_results": 2,
                "page": 0
            }"#,
            "application/json",
        ))
        .mount(&server)
        .await;

    let client = TinyFishSearchClient::new(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        Url::parse(&server.uri()).expect("mock endpoint should be valid"),
        RedactedString::from(TEST_API_KEY),
    )
    .expect("client should build");
    let response = client
        .search(&TinyFishSearchRequest {
            query: "rust async traits".to_string(),
            domains: Some(vec!["doc.rust-lang.org".to_string(), "docs.rs".to_string()]),
            recency_days: Some(30),
            location: Some("US".to_string()),
        })
        .await
        .expect("search should succeed");

    assert_eq!(
        response,
        TinyFishSearchResponse {
            query: "rust async traits".to_string(),
            results: vec![
                TinyFishSearchResult {
                    position: 1,
                    site_name: "doc.rust-lang.org".to_string(),
                    title: "Async traits".to_string(),
                    snippet: "Native async functions in traits.".to_string(),
                    url: "https://doc.rust-lang.org/book/async-traits".to_string(),
                },
                TinyFishSearchResult {
                    position: 2,
                    site_name: "docs.rs".to_string(),
                    title: "async-trait".to_string(),
                    snippet: "Type erasure for async trait methods.".to_string(),
                    url: "https://docs.rs/async-trait".to_string(),
                },
            ],
            total_results: 2,
            page: 0,
        }
    );
}

#[tokio::test]
async fn tinyfish_error_unauthorized_is_safe_and_actionable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 401)
                .set_body_string(format!("invalid key: {TEST_API_KEY}")),
        )
        .mount(&server)
        .await;

    let error = test_client(&server)
        .search(&test_request())
        .await
        .expect_err("401 should fail");

    assert_eq!(error.to_string(), "TinyFish rejected TINYFISH_API_KEY");
    assert!(!format!("{error:?}").contains(TEST_API_KEY));
}

#[tokio::test]
async fn tinyfish_error_rate_limit_is_safe_and_actionable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 429)
                .set_body_string(format!("rate limited key: {TEST_API_KEY}")),
        )
        .mount(&server)
        .await;

    let error = test_client(&server)
        .search(&test_request())
        .await
        .expect_err("429 should fail");

    assert_eq!(error.to_string(), "TinyFish web search rate limit exceeded");
    assert!(!format!("{error:?}").contains(TEST_API_KEY));
}

#[tokio::test]
async fn tinyfish_error_body_is_bounded_and_redacts_the_api_key() {
    let server = MockServer::start().await;
    let provider_body = format!(
        "provider failure containing {TEST_API_KEY}: {}",
        "x".repeat(2_048)
    );
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(/*status*/ 500).set_body_string(provider_body.clone()))
        .mount(&server)
        .await;

    let error = test_client(&server)
        .search(&test_request())
        .await
        .expect_err("500 should fail");
    let formatted = error.to_string();
    let debug = format!("{error:?}");

    assert!(formatted.contains("provider failure containing [REDACTED]"));
    assert!(debug.contains("provider failure containing [REDACTED]"));
    assert!(formatted.len() <= 1_200, "formatted error was unbounded");
    assert!(debug.len() <= 1_300, "debug error was unbounded");
    assert!(!formatted.contains(TEST_API_KEY));
    assert!(!debug.contains(TEST_API_KEY));
    assert!(provider_body.len() > 1_024);
}

#[tokio::test]
async fn tinyfish_error_recency_overflow_prevents_the_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(/*status*/ 200))
        .expect(/*n*/ 0)
        .mount(&server)
        .await;
    let mut request = test_request();
    request.recency_days = Some(u64::MAX);

    let error = test_client(&server)
        .search(&request)
        .await
        .expect_err("overflow should fail before sending");

    assert_eq!(
        error.to_string(),
        format!(
            "TinyFish recency_days value {max} is too large",
            max = u64::MAX
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

fn test_client(server: &MockServer) -> TinyFishSearchClient {
    TinyFishSearchClient::new(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        Url::parse(&server.uri()).expect("mock endpoint should be valid"),
        RedactedString::from(TEST_API_KEY),
    )
    .expect("client should build")
}

fn test_request() -> TinyFishSearchRequest {
    TinyFishSearchRequest {
        query: "bounded errors".to_string(),
        domains: None,
        recency_days: None,
        location: None,
    }
}
