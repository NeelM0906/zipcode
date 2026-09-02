use std::time::Duration;

use codex_http_client::BuildRouteAwareHttpClientError;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClient;
use codex_http_client::HttpClientFactory;
use codex_http_client::HttpError;
use codex_utils_redacted_string::RedactedString;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use url::Url;

pub(crate) const TINYFISH_SEARCH_ENDPOINT: &str = "https://api.search.tinyfish.ai";
pub(crate) const TINYFISH_API_KEY_ENV: &str = "TINYFISH_API_KEY";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ERROR_BODY_BYTES: usize = 1024;

#[derive(Clone)]
pub(crate) struct TinyFishSearchClient {
    client: HttpClient,
    endpoint: Url,
    api_key: RedactedString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TinyFishSearchRequest {
    pub query: String,
    pub domains: Option<Vec<String>>,
    pub recency_days: Option<u64>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TinyFishSearchResponse {
    pub query: String,
    pub results: Vec<TinyFishSearchResult>,
    pub total_results: u64,
    #[serde(default)]
    pub page: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TinyFishSearchResult {
    pub position: u64,
    pub site_name: String,
    pub title: String,
    pub snippet: String,
    pub url: String,
}

#[derive(Debug, Error)]
pub(crate) enum TinyFishError {
    #[error("failed to configure TinyFish web search")]
    Configuration {
        #[source]
        source: BuildRouteAwareHttpClientError,
    },
    #[error("TinyFish web search request failed")]
    Request {
        #[source]
        source: HttpError,
    },
    #[error("TinyFish rejected TINYFISH_API_KEY")]
    ApiKeyRejected,
    #[error("TinyFish web search rate limit exceeded")]
    RateLimited,
    #[error("TinyFish web search returned HTTP {status}: {body}")]
    HttpStatus {
        status: http::StatusCode,
        body: String,
    },
    #[error("TinyFish web search returned invalid JSON")]
    ResponseDecode {
        #[source]
        source: HttpError,
    },
    #[error("TinyFish recency_days value {days} is too large")]
    RecencyOverflow { days: u64 },
}

#[derive(Serialize)]
struct TinyFishWireRequest<'a> {
    query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_domains: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recency_minutes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'a str>,
}

impl TinyFishSearchClient {
    pub(crate) fn new(
        http_client_factory: HttpClientFactory,
        endpoint: Url,
        api_key: RedactedString,
    ) -> Result<Self, TinyFishError> {
        let client = http_client_factory
            .build_client_without_request_logging(endpoint.as_str(), ClientRouteClass::Other)
            .map_err(|source| TinyFishError::Configuration { source })?;
        Ok(Self {
            client,
            endpoint,
            api_key,
        })
    }

    pub(crate) async fn search(
        &self,
        request: &TinyFishSearchRequest,
    ) -> Result<TinyFishSearchResponse, TinyFishError> {
        let recency_minutes = request
            .recency_days
            .map(|days| {
                days.checked_mul(24 * 60)
                    .ok_or(TinyFishError::RecencyOverflow { days })
            })
            .transpose()?;
        let query = TinyFishWireRequest {
            query: &request.query,
            include_domains: request.domains.as_ref().map(|domains| domains.join(",")),
            recency_minutes,
            location: request.location.as_deref(),
        };
        let response = self
            .client
            .get(self.endpoint.clone())
            .header("X-API-Key", self.api_key.as_str())
            .query(&query)
            .timeout(SEARCH_TIMEOUT)
            .send()
            .await
            .map_err(|source| TinyFishError::Request { source })?;
        let status = response.status();
        if !status.is_success() {
            return match status {
                http::StatusCode::UNAUTHORIZED => Err(TinyFishError::ApiKeyRejected),
                http::StatusCode::TOO_MANY_REQUESTS => Err(TinyFishError::RateLimited),
                _ => {
                    let body = match response.content_length() {
                        Some(length) if length > MAX_ERROR_BODY_BYTES as u64 => format!(
                            "[response body omitted because it exceeds {MAX_ERROR_BODY_BYTES} bytes]"
                        ),
                        Some(_) => match response.bytes().await {
                            Ok(bytes) if bytes.len() <= MAX_ERROR_BODY_BYTES => {
                                let mut body = String::from_utf8_lossy(&bytes).into_owned();
                                if !self.api_key.as_str().is_empty() {
                                    body = body.replace(self.api_key.as_str(), "[REDACTED]");
                                }
                                if body.len() <= MAX_ERROR_BODY_BYTES {
                                    body
                                } else {
                                    format!(
                                        "[response body omitted because it exceeds {MAX_ERROR_BODY_BYTES} bytes after redaction]"
                                    )
                                }
                            }
                            Ok(_) => format!(
                                "[response body omitted because it exceeds {MAX_ERROR_BODY_BYTES} bytes]"
                            ),
                            Err(_) => "[failed to read response body]".to_string(),
                        },
                        None => "[response body omitted because its length is unknown]".to_string(),
                    };
                    Err(TinyFishError::HttpStatus { status, body })
                }
            };
        }

        response
            .json()
            .await
            .map_err(|source| TinyFishError::ResponseDecode { source })
    }
}
