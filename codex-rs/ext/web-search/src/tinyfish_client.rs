use std::io::Read;
use std::time::Duration;

use codex_http_client::BuildRouteAwareHttpClientError;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClient;
use codex_http_client::HttpClientBuilder;
use codex_http_client::HttpClientFactory;
use codex_http_client::HttpError;
use codex_utils_redacted_string::RedactedString;
use flate2::read::MultiGzDecoder;
use http::header::ACCEPT_ENCODING;
use http::header::CONTENT_ENCODING;
use thiserror::Error;
use url::Url;

use crate::tinyfish_output::TinyFishSearchResponse;
use crate::tinyfish_request::TinyFishWireRequest;

pub(crate) const TINYFISH_SEARCH_ENDPOINT: &str = "https://api.search.tinyfish.ai";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ERROR_BODY_BYTES: usize = 1024;
const MAX_SUCCESS_BODY_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct TinyFishSearchClient {
    client: HttpClient,
    endpoint: Url,
    api_key: RedactedString,
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
    #[error("TinyFish account lacks Search API access")]
    AccessDenied,
    #[error("TinyFish search request was forbidden by the upstream service")]
    SearchForbidden,
    #[error("TinyFish web search returned HTTP {status}: {body}")]
    HttpStatus {
        status: http::StatusCode,
        body: String,
    },
    #[error("TinyFish web search response exceeded 1048576 bytes")]
    ResponseTooLarge,
    #[error("TinyFish web search returned an unsupported content encoding")]
    UnsupportedContentEncoding,
    #[error("TinyFish web search returned invalid gzip data")]
    ResponseDecompression,
    #[error("failed to read TinyFish web search response")]
    ResponseRead {
        #[source]
        source: HttpError,
    },
    #[error("TinyFish web search returned invalid JSON")]
    ResponseDecode,
}

impl TinyFishSearchClient {
    pub(crate) fn from_endpoint(
        http_client_factory: HttpClientFactory,
        endpoint: Url,
        api_key: RedactedString,
    ) -> Result<Self, TinyFishError> {
        let client = HttpClientBuilder::new()
            .without_redirects()
            .without_request_logging()
            .build_respecting_outbound_proxy_policy(
                &http_client_factory,
                endpoint.as_str(),
                ClientRouteClass::Other,
            )
            .map_err(|source| TinyFishError::Configuration { source })?;
        Ok(Self {
            client,
            endpoint,
            api_key,
        })
    }

    pub(crate) async fn search(
        &self,
        request: &TinyFishWireRequest,
    ) -> Result<TinyFishSearchResponse, TinyFishError> {
        let response = self
            .client
            .get(request.request_url(self.endpoint.clone()))
            .header("X-API-Key", self.api_key.as_str())
            .header(ACCEPT_ENCODING, "gzip")
            .timeout(SEARCH_TIMEOUT)
            .send()
            .await
            .map_err(|source| TinyFishError::Request { source })?;
        let status = response.status();
        if !status.is_success() {
            return match status {
                http::StatusCode::UNAUTHORIZED => Err(TinyFishError::ApiKeyRejected),
                http::StatusCode::PAYMENT_REQUIRED => Err(TinyFishError::AccessDenied),
                http::StatusCode::FORBIDDEN => Err(TinyFishError::SearchForbidden),
                http::StatusCode::TOO_MANY_REQUESTS => Err(TinyFishError::RateLimited),
                _ => Err(TinyFishError::HttpStatus {
                    status,
                    body: bounded_error_body(response, self.api_key.as_str()).await,
                }),
            };
        }

        let encoding = response_encoding(&response)?;
        let body = read_success_body(response).await?;
        let body = match encoding {
            ResponseEncoding::Identity => body,
            ResponseEncoding::Gzip => decode_gzip_body(&body)?,
        };

        let mut response: TinyFishSearchResponse =
            serde_json::from_slice(&body).map_err(|_| TinyFishError::ResponseDecode)?;
        redact_api_key(&mut response, self.api_key.as_str());
        Ok(response)
    }
}

#[cfg(test)]
#[path = "tinyfish_client_tests.rs"]
mod tests;

#[derive(Clone, Copy)]
enum ResponseEncoding {
    Identity,
    Gzip,
}

fn response_encoding(
    response: &codex_http_client::HttpResponse,
) -> Result<ResponseEncoding, TinyFishError> {
    let mut values = response.headers().get_all(CONTENT_ENCODING).iter();
    let Some(value) = values.next() else {
        return Ok(ResponseEncoding::Identity);
    };
    if values.next().is_some() {
        return Err(TinyFishError::UnsupportedContentEncoding);
    }
    match value.to_str().ok().map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("identity") => Ok(ResponseEncoding::Identity),
        Some(value) if value.eq_ignore_ascii_case("gzip") => Ok(ResponseEncoding::Gzip),
        _ => Err(TinyFishError::UnsupportedContentEncoding),
    }
}

async fn read_success_body(
    mut response: codex_http_client::HttpResponse,
) -> Result<Vec<u8>, TinyFishError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SUCCESS_BODY_BYTES as u64)
    {
        return Err(TinyFishError::ResponseTooLarge);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| TinyFishError::ResponseRead { source })?
    {
        append_bounded(&mut body, &chunk)?;
    }
    Ok(body)
}

fn decode_gzip_body(body: &[u8]) -> Result<Vec<u8>, TinyFishError> {
    let mut decoder = MultiGzDecoder::new(body);
    let mut decoded = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let bytes_read = decoder
            .read(&mut buffer)
            .map_err(|_| TinyFishError::ResponseDecompression)?;
        if bytes_read == 0 {
            return Ok(decoded);
        }
        append_bounded(&mut decoded, &buffer[..bytes_read])?;
    }
}

fn append_bounded(body: &mut Vec<u8>, bytes: &[u8]) -> Result<(), TinyFishError> {
    let Some(next_length) = body.len().checked_add(bytes.len()) else {
        return Err(TinyFishError::ResponseTooLarge);
    };
    if next_length > MAX_SUCCESS_BODY_BYTES {
        return Err(TinyFishError::ResponseTooLarge);
    }
    body.extend_from_slice(bytes);
    Ok(())
}

async fn bounded_error_body(
    mut response: codex_http_client::HttpResponse,
    api_key: &str,
) -> String {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ERROR_BODY_BYTES as u64)
    {
        return format!("[response body omitted because it exceeds {MAX_ERROR_BODY_BYTES} bytes]");
    }

    let mut bytes = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return "[failed to read response body]".to_string(),
        };
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > MAX_ERROR_BODY_BYTES)
        {
            return format!(
                "[response body omitted because it exceeds {MAX_ERROR_BODY_BYTES} bytes]"
            );
        }
        bytes.extend_from_slice(&chunk);
    }

    let mut body = String::from_utf8_lossy(&bytes).into_owned();
    if !api_key.is_empty() {
        body = body.replace(api_key, "[REDACTED]");
    }
    if body.len() <= MAX_ERROR_BODY_BYTES {
        body
    } else {
        format!(
            "[response body omitted because it exceeds {MAX_ERROR_BODY_BYTES} bytes after redaction]"
        )
    }
}

fn redact_api_key(response: &mut TinyFishSearchResponse, api_key: &str) {
    if api_key.is_empty() {
        return;
    }

    redact_string(&mut response.query, api_key);
    for result in &mut response.results {
        result.for_each_string_mut(|value| redact_string(value, api_key));
    }
}

fn redact_string(value: &mut String, api_key: &str) {
    if value.contains(api_key) {
        *value = value.replace(api_key, "[REDACTED]");
    }
}
