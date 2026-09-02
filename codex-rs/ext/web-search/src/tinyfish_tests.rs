use std::io::Write;

use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_utils_redacted_string::RedactedString;
use flate2::Compression;
use flate2::write::GzEncoder;
use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use url::Url;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

use crate::tinyfish::TinyFishError;
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
                    date: None,
                    publisher: None,
                    authors: None,
                    venue: None,
                    year: None,
                    cited_by_count: None,
                    pdf_url: None,
                },
                TinyFishSearchResult {
                    position: 2,
                    site_name: "docs.rs".to_string(),
                    title: "async-trait".to_string(),
                    snippet: "Type erasure for async trait methods.".to_string(),
                    url: "https://docs.rs/async-trait".to_string(),
                    date: None,
                    publisher: None,
                    authors: None,
                    venue: None,
                    year: None,
                    cited_by_count: None,
                    pdf_url: None,
                },
            ],
            total_results: 2,
            page: 0,
        }
    );
}

#[tokio::test]
async fn tinyfish_redirect_does_not_reach_cross_origin_destination() {
    let destination = MockServer::start().await;
    let source = MockServer::start().await;
    let redirect_url = format!("{}/redirected", destination.uri());
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("X-API-Key", TEST_API_KEY))
        .respond_with(
            ResponseTemplate::new(/*status*/ 302)
                .insert_header("Location", redirect_url.as_str())
                .set_body_string(format!("{TEST_API_KEY}{}", "x".repeat(2_048))),
        )
        .mount(&source)
        .await;

    let error = test_client(&source)
        .search(&test_request())
        .await
        .expect_err("redirect should be returned as a status error");
    let (status, body) = http_status(&error);

    assert_eq!(status, http::StatusCode::FOUND);
    assert_eq!(
        body,
        "[response body omitted because it exceeds 1024 bytes]"
    );
    assert!(!error.to_string().contains(TEST_API_KEY));
    assert!(
        destination
            .received_requests()
            .await
            .expect("destination requests should be available")
            .is_empty()
    );
}

#[tokio::test]
async fn tinyfish_success_body_rejects_known_length_over_one_mibibyte() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200)
                .set_body_raw(oversized_success_json(), "application/json"),
        )
        .mount(&server)
        .await;

    let error = test_client(&server)
        .search(&test_request())
        .await
        .expect_err("known oversized success body should fail");

    assert_success_body_too_large(&error);
}

#[tokio::test]
async fn tinyfish_success_body_rejects_chunked_unknown_length_over_one_mibibyte() {
    let (endpoint, server) = start_chunked_server(oversized_success_json()).await;
    let client = TinyFishSearchClient::new(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        endpoint,
        RedactedString::from(TEST_API_KEY),
    )
    .expect("client should build");

    let error = client
        .search(&test_request())
        .await
        .expect_err("chunked oversized success body should fail");
    server
        .await
        .expect("chunked server should finish")
        .expect("chunked server response should succeed");

    assert_success_body_too_large(&error);
}

#[tokio::test]
async fn tinyfish_success_body_rejects_decoded_gzip_over_one_mibibyte() {
    let server = MockServer::start().await;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(oversized_success_json().as_bytes())
        .expect("fixture should compress");
    let compressed = encoder.finish().expect("fixture should finish compressing");
    assert!(compressed.len() < 1_048_576);
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200)
                .insert_header("Content-Encoding", "gzip")
                .insert_header("Content-Type", "application/json")
                .set_body_bytes(compressed),
        )
        .mount(&server)
        .await;

    let error = test_client(&server)
        .search(&test_request())
        .await
        .expect_err("decoded oversized success body should fail");

    assert_success_body_too_large(&error);
}

#[tokio::test]
async fn tinyfish_result_dto_accepts_sparse_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200).set_body_json(serde_json::json!({
                "query": "sparse",
                "results": [{ "title": "Sparse title" }],
                "total_results": 1,
            })),
        )
        .mount(&server)
        .await;

    let response = test_client(&server)
        .search(&test_request())
        .await
        .expect("sparse result should decode");

    assert_eq!(
        response,
        TinyFishSearchResponse {
            query: "sparse".to_string(),
            results: vec![TinyFishSearchResult {
                position: 0,
                site_name: String::new(),
                title: "Sparse title".to_string(),
                snippet: String::new(),
                url: String::new(),
                date: None,
                publisher: None,
                authors: None,
                venue: None,
                year: None,
                cited_by_count: None,
                pdf_url: None,
            }],
            total_results: 1,
            page: 0,
        }
    );
}

#[tokio::test]
async fn tinyfish_result_dto_preserves_optional_metadata_and_order() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200).set_body_json(serde_json::json!({
                "query": "metadata",
                "results": [
                    {
                        "position": 7,
                        "site_name": "papers.example",
                        "title": "First provider result",
                        "snippet": "First snippet",
                        "url": "https://papers.example/first",
                        "date": "2025-05-01",
                        "publisher": "Example Press",
                        "authors": ["Ada Author", "Grace Writer"],
                        "venue": "RustConf",
                        "year": 2025,
                        "cited_by_count": 42,
                        "pdf_url": "https://papers.example/first.pdf"
                    },
                    {
                        "position": 3,
                        "title": "Second provider result",
                        "url": "https://papers.example/second",
                        "authors": ["Second Author"],
                        "year": 2024
                    }
                ],
                "total_results": 2,
                "page": 4,
            })),
        )
        .mount(&server)
        .await;

    let response = test_client(&server)
        .search(&test_request())
        .await
        .expect("metadata results should decode");

    assert_eq!(
        response,
        TinyFishSearchResponse {
            query: "metadata".to_string(),
            results: vec![
                TinyFishSearchResult {
                    position: 7,
                    site_name: "papers.example".to_string(),
                    title: "First provider result".to_string(),
                    snippet: "First snippet".to_string(),
                    url: "https://papers.example/first".to_string(),
                    date: Some("2025-05-01".to_string()),
                    publisher: Some("Example Press".to_string()),
                    authors: Some(vec!["Ada Author".to_string(), "Grace Writer".to_string(),]),
                    venue: Some("RustConf".to_string()),
                    year: Some(2025),
                    cited_by_count: Some(42),
                    pdf_url: Some("https://papers.example/first.pdf".to_string()),
                },
                TinyFishSearchResult {
                    position: 3,
                    site_name: String::new(),
                    title: "Second provider result".to_string(),
                    snippet: String::new(),
                    url: "https://papers.example/second".to_string(),
                    date: None,
                    publisher: None,
                    authors: Some(vec!["Second Author".to_string()]),
                    venue: None,
                    year: Some(2024),
                    cited_by_count: None,
                    pdf_url: None,
                },
            ],
            total_results: 2,
            page: 4,
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
async fn tinyfish_access_errors_are_safe_and_actionable() {
    for status in [402, 403] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_string(format!("account denied for {TEST_API_KEY}")),
            )
            .mount(&server)
            .await;

        let error = test_client(&server)
            .search(&test_request())
            .await
            .expect_err("account without Search API access should fail");

        assert_eq!(
            error.to_string(),
            "TinyFish account lacks Search API access"
        );
        assert!(!format!("{error:?}").contains(TEST_API_KEY));
    }
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
    let body = http_status_body(&error);

    assert_eq!(
        body,
        "[response body omitted because it exceeds 1024 bytes]"
    );
    assert!(formatted.len() <= 1_200, "formatted error was unbounded");
    assert!(debug.len() <= 1_300, "debug error was unbounded");
    assert!(!formatted.contains(TEST_API_KEY));
    assert!(!debug.contains(TEST_API_KEY));
    assert!(provider_body.len() > 1_024);
}

#[tokio::test]
async fn tinyfish_error_body_does_not_expose_a_secret_crossing_the_limit() {
    const BOUNDARY_API_KEY: &str = "BOUNDARY-SECRET-KEY";

    let server = MockServer::start().await;
    let provider_body = format!("{}{BOUNDARY_API_KEY}", "x".repeat(1_020));
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(/*status*/ 500).set_body_string(provider_body))
        .mount(&server)
        .await;

    let error = test_client_with_api_key(&server, BOUNDARY_API_KEY)
        .search(&test_request())
        .await
        .expect_err("500 should fail");
    let formatted = error.to_string();
    let debug = format!("{error:?}");
    let body = http_status_body(&error);

    assert_eq!(
        body,
        "[response body omitted because it exceeds 1024 bytes]"
    );
    assert!(!formatted.contains("BOUN"));
    assert!(!debug.contains("BOUN"));
}

#[tokio::test]
async fn tinyfish_error_body_at_exact_limit_is_not_marked_truncated() {
    let server = MockServer::start().await;
    let provider_body = "z".repeat(1_024);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(/*status*/ 500).set_body_string(provider_body.clone()))
        .mount(&server)
        .await;

    let error = test_client(&server)
        .search(&test_request())
        .await
        .expect_err("500 should fail");

    assert_eq!(http_status_body(&error), provider_body);
    assert!(!error.to_string().contains("truncated"));
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
    test_client_with_api_key(server, TEST_API_KEY)
}

fn test_client_with_api_key(server: &MockServer, api_key: &str) -> TinyFishSearchClient {
    TinyFishSearchClient::new(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        Url::parse(&server.uri()).expect("mock endpoint should be valid"),
        RedactedString::from(api_key),
    )
    .expect("client should build")
}

fn http_status_body(error: &TinyFishError) -> &str {
    http_status(error).1
}

fn http_status(error: &TinyFishError) -> (http::StatusCode, &str) {
    let TinyFishError::HttpStatus { status, body } = error else {
        panic!("expected an HTTP status error, got {error:?}");
    };
    (*status, body)
}

fn test_request() -> TinyFishSearchRequest {
    TinyFishSearchRequest {
        query: "bounded errors".to_string(),
        domains: None,
        recency_days: None,
        location: None,
    }
}

fn oversized_success_json() -> String {
    format!(
        r#"{{"query":"bounded success","results":[],"total_results":0,"page":0,"padding":"{TEST_API_KEY}-padding-marker{}"}}"#,
        "x".repeat(1_048_576)
    )
}

fn assert_success_body_too_large(error: &TinyFishError) {
    let formatted = error.to_string();
    let debug = format!("{error:?}");
    assert_eq!(
        formatted,
        "TinyFish web search response exceeded 1048576 bytes"
    );
    assert!(!debug.contains(TEST_API_KEY));
    assert!(!debug.contains("padding-marker"));
}

async fn start_chunked_server(body: String) -> (Url, tokio::task::JoinHandle<std::io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("chunked server should bind");
    let address = listener
        .local_addr()
        .expect("chunked server should have an address");
    let endpoint =
        Url::parse(&format!("http://{address}")).expect("chunked server endpoint should be valid");
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let bytes_read = socket.read(&mut buffer).await?;
            if bytes_read == 0 {
                return Ok(());
            }
            request.extend_from_slice(&buffer[..bytes_read]);
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await?;
        for chunk in body.as_bytes().chunks(64 * 1_024) {
            if socket
                .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
                .await
                .is_err()
            {
                return Ok(());
            }
            if socket.write_all(chunk).await.is_err() || socket.write_all(b"\r\n").await.is_err() {
                return Ok(());
            }
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
        Ok(())
    });
    (endpoint, task)
}
