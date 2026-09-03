use std::error::Error as StdError;
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

use super::TinyFishError;
use super::TinyFishSearchClient;
use crate::tinyfish_output::TinyFishSearchResponse;
use crate::tinyfish_output::TinyFishSearchResult;
use crate::tinyfish_request::TinyFishWireRequest;

const TEST_API_KEY: &str = "test-tinyfish-key";

#[tokio::test]
async fn sends_the_maximum_recency_and_decodes_the_typed_response() {
    let server = MockServer::start().await;
    let expected_response = TinyFishSearchResponse {
        query: "rust async traits".to_string(),
        results: vec![TinyFishSearchResult {
            position: 1,
            site_name: "doc.rust-lang.org".to_string(),
            title: "Async traits".to_string(),
            snippet: "Native async functions in traits.".to_string(),
            url: "https://doc.rust-lang.org/book/async-traits".to_string(),
            date: Some("2026-01-02".to_string()),
            publisher: Some("Rust Project".to_string()),
            authors: Some(vec!["Ferris".to_string()]),
            venue: Some("The Rust Book".to_string()),
            year: Some(2026),
            cited_by_count: Some(7),
            pdf_url: Some("https://doc.rust-lang.org/async-traits.pdf".to_string()),
        }],
        total_results: 1,
        page: 2,
    };
    let response_body = serde_json::to_vec(&expected_response).expect("fixture should serialize");
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("X-API-Key", TEST_API_KEY))
        .and(header("Accept-Encoding", "gzip"))
        .and(query_param("query", "rust async traits"))
        .and(query_param("include_domains", "doc.rust-lang.org,docs.rs"))
        .and(query_param("recency_minutes", "5256000"))
        .and(query_param("location", "US"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200)
                .insert_header("Content-Encoding", "gzip")
                .insert_header("Content-Type", "application/json")
                .set_body_bytes(gzip_bytes(&response_body)),
        )
        .mount(&server)
        .await;

    let response = test_client(&server)
        .search(&TinyFishWireRequest {
            query: "rust async traits".to_string(),
            include_domains: Some("doc.rust-lang.org,docs.rs".to_string()),
            recency_minutes: Some(5_256_000),
            location: Some("US".to_string()),
        })
        .await
        .expect("search should succeed");

    assert_eq!(response, expected_response);
}

#[tokio::test]
async fn does_not_forward_the_api_key_across_a_redirect() {
    let destination = MockServer::start().await;
    let source = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("X-API-Key", TEST_API_KEY))
        .respond_with(
            ResponseTemplate::new(/*status*/ 302)
                .insert_header(
                    "Location",
                    format!("{}/redirected", destination.uri()).as_str(),
                )
                .set_body_string(format!("{TEST_API_KEY}{}", "x".repeat(2_048))),
        )
        .mount(&source)
        .await;

    let error = test_client(&source)
        .search(&test_request())
        .await
        .expect_err("redirect should be surfaced as an HTTP status error");

    assert_eq!(http_status(&error).0, http::StatusCode::FOUND);
    assert!(!format!("{error:?}").contains(TEST_API_KEY));
    assert!(
        destination
            .received_requests()
            .await
            .expect("destination requests should be available")
            .is_empty()
    );
}

#[tokio::test]
async fn rejects_a_known_length_success_body_over_one_mibibyte() {
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
        .expect_err("known oversized response should fail");

    assert_response_too_large(&error);
}

#[tokio::test]
async fn rejects_a_chunked_success_body_over_one_mibibyte() {
    let (endpoint, server) =
        start_chunked_server(http::StatusCode::OK, oversized_success_json()).await;
    let error = test_client_at(endpoint)
        .search(&test_request())
        .await
        .expect_err("chunked oversized response should fail");
    server
        .await
        .expect("chunked server should finish")
        .expect("chunked response should succeed");

    assert_response_too_large(&error);
}

#[tokio::test]
async fn rejects_a_decoded_gzip_success_body_over_one_mibibyte() {
    let server = MockServer::start().await;
    let compressed = gzip_bytes(oversized_success_json().as_bytes());
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
        .expect_err("decoded oversized response should fail");

    assert_response_too_large(&error);
}

#[tokio::test]
async fn rejects_an_unsupported_content_encoding_without_reflecting_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200)
                .insert_header("Content-Encoding", format!("br-{TEST_API_KEY}").as_str())
                .set_body_json(serde_json::json!({
                    "query": "bounded transport",
                    "results": [],
                    "total_results": 0
                })),
        )
        .mount(&server)
        .await;

    let error = test_client(&server)
        .search(&test_request())
        .await
        .expect_err("unsupported response encoding should fail");

    assert_eq!(
        error.to_string(),
        "TinyFish web search returned an unsupported content encoding"
    );
    assert!(!format!("{error:?}").contains(TEST_API_KEY));
}

#[tokio::test]
async fn malformed_typed_response_does_not_expose_a_reflected_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200).set_body_json(serde_json::json!({
                "query": "bounded transport",
                "results": [],
                "total_results": TEST_API_KEY
            })),
        )
        .mount(&server)
        .await;

    let error = test_client(&server)
        .search(&test_request())
        .await
        .expect_err("malformed typed response should fail");
    assert_eq!(
        error.to_string(),
        "TinyFish web search returned invalid JSON"
    );

    let mut rendered_chain = format!("{error}\n{error:?}");
    let mut source = error.source();
    while let Some(next) = source {
        rendered_chain.push_str(&format!("\n{next}\n{next:?}"));
        source = next.source();
    }
    assert!(!rendered_chain.contains(TEST_API_KEY));
}

#[tokio::test]
async fn redacts_the_api_key_from_every_decoded_string_field() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 200).set_body_json(serde_json::json!({
                "query": format!("query {TEST_API_KEY}"),
                "results": [{
                    "site_name": format!("site {TEST_API_KEY}"),
                    "title": format!("title {TEST_API_KEY}"),
                    "snippet": format!("snippet {TEST_API_KEY}"),
                    "url": format!("https://example.test/{TEST_API_KEY}"),
                    "date": format!("date {TEST_API_KEY}"),
                    "publisher": format!("publisher {TEST_API_KEY}"),
                    "authors": [format!("author {TEST_API_KEY}")],
                    "venue": format!("venue {TEST_API_KEY}"),
                    "pdf_url": format!("https://example.test/{TEST_API_KEY}.pdf")
                }],
                "total_results": 1
            })),
        )
        .mount(&server)
        .await;

    let response = test_client(&server)
        .search(&test_request())
        .await
        .expect("response should decode");
    let serialized = serde_json::to_string(&response).expect("response should serialize");

    assert!(!serialized.contains(TEST_API_KEY));
    assert_eq!(serialized.matches("[REDACTED]").count(), 10);
}

#[tokio::test]
async fn maps_provider_statuses_to_safe_actionable_errors() {
    for (status, expected) in [
        (401, "TinyFish rejected TINYFISH_API_KEY"),
        (402, "TinyFish account lacks Search API access"),
        (
            403,
            "TinyFish search request was forbidden by the upstream service",
        ),
        (429, "TinyFish web search rate limit exceeded"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_string(format!("provider reflected {TEST_API_KEY}")),
            )
            .mount(&server)
            .await;

        let error = test_client(&server)
            .search(&test_request())
            .await
            .expect_err("provider status should fail");

        assert_eq!(error.to_string(), expected);
        assert!(!format!("{error:?}").contains(TEST_API_KEY));
    }
}

#[tokio::test]
async fn bounds_and_redacts_generic_provider_error_bodies() {
    let small_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 500)
                .set_body_string(format!("provider reflected {TEST_API_KEY}")),
        )
        .mount(&small_server)
        .await;
    let small_error = test_client(&small_server)
        .search(&test_request())
        .await
        .expect_err("provider error should fail");
    assert_eq!(http_status(&small_error).1, "provider reflected [REDACTED]");

    let large_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(/*status*/ 500)
                .set_body_string(format!("{TEST_API_KEY}{}", "x".repeat(2_048))),
        )
        .mount(&large_server)
        .await;
    let large_error = test_client(&large_server)
        .search(&test_request())
        .await
        .expect_err("oversized provider error should fail");

    assert_eq!(
        http_status(&large_error).1,
        "[response body omitted because it exceeds 1024 bytes]"
    );
    assert!(!format!("{large_error:?}").contains(TEST_API_KEY));
}

#[tokio::test]
async fn bounds_a_chunked_provider_error_body() {
    let (endpoint, server) = start_chunked_server(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        format!("{TEST_API_KEY}{}", "x".repeat(2_048)),
    )
    .await;
    let error = test_client_at(endpoint)
        .search(&test_request())
        .await
        .expect_err("oversized chunked provider error should fail");
    server
        .await
        .expect("chunked server should finish")
        .expect("chunked response should succeed");

    assert_eq!(
        http_status(&error).1,
        "[response body omitted because it exceeds 1024 bytes]"
    );
    assert!(!format!("{error:?}").contains(TEST_API_KEY));
}

fn test_client(server: &MockServer) -> TinyFishSearchClient {
    test_client_at(Url::parse(&server.uri()).expect("mock endpoint should be valid"))
}

fn test_client_at(endpoint: Url) -> TinyFishSearchClient {
    TinyFishSearchClient::from_endpoint(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        endpoint,
        RedactedString::from(TEST_API_KEY),
    )
    .expect("client should build")
}

fn http_status(error: &TinyFishError) -> (http::StatusCode, &str) {
    let TinyFishError::HttpStatus { status, body } = error else {
        panic!("expected an HTTP status error, got {error:?}");
    };
    (*status, body)
}

fn test_request() -> TinyFishWireRequest {
    TinyFishWireRequest {
        query: "bounded transport".to_string(),
        include_domains: None,
        recency_minutes: None,
        location: None,
    }
}

fn oversized_success_json() -> String {
    format!(
        r#"{{"query":"bounded success","results":[],"total_results":0,"padding":"{TEST_API_KEY}{}"}}"#,
        "x".repeat(1_048_576)
    )
}

fn gzip_bytes(body: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(body).expect("fixture should compress");
    encoder.finish().expect("fixture should finish compressing")
}

fn assert_response_too_large(error: &TinyFishError) {
    assert_eq!(
        error.to_string(),
        "TinyFish web search response exceeded 1048576 bytes"
    );
    let debug = format!("{error:?}");
    assert!(!debug.contains(TEST_API_KEY));
    assert!(!debug.contains("bounded success"));
}

async fn start_chunked_server(
    status: http::StatusCode,
    body: String,
) -> (Url, tokio::task::JoinHandle<std::io::Result<()>>) {
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
                format!(
                    "HTTP/1.1 {} TinyFish Test\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                    status.as_u16()
                )
                .as_bytes(),
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
