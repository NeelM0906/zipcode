use codex_api::SearchSettings;
use codex_extension_api::FunctionCallError;
use codex_secrets::redact_secrets;
use codex_utils_redacted_string::RedactedString;
use serde::Serialize;
use url::Url;

use crate::schema::TinyFishCommands;

pub(crate) const MAX_TINYFISH_RECENCY_DAYS: u64 = 3_650;
const MAX_TINYFISH_REVIEW_COMMAND_BYTES: usize = 8 * 1024;
const MINUTES_PER_DAY: u64 = 24 * 60;
const TINYFISH_REVIEW_URL: &str = "https://tinyfish.invalid/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TinyFishSearchRequest {
    pub(crate) query: String,
    pub(crate) domains: Option<Vec<String>>,
    pub(crate) recency_days: Option<u64>,
    pub(crate) location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TinyFishWireRequest {
    pub(crate) query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) include_domains: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recency_minutes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) location: Option<String>,
}

impl TinyFishWireRequest {
    pub(crate) fn request_url(&self, mut endpoint: Url) -> Url {
        let mut query = endpoint.query_pairs_mut();
        query.append_pair("query", &self.query);
        if let Some(include_domains) = self.include_domains.as_deref() {
            query.append_pair("include_domains", include_domains);
        }
        if let Some(recency_minutes) = self.recency_minutes {
            query.append_pair("recency_minutes", &recency_minutes.to_string());
        }
        if let Some(location) = self.location.as_deref() {
            query.append_pair("location", location);
        }
        drop(query);
        endpoint
    }

    fn contains(&self, value: &str) -> bool {
        self.query.contains(value)
            || self
                .include_domains
                .as_deref()
                .is_some_and(|domains| domains.contains(value))
            || self
                .recency_minutes
                .is_some_and(|minutes| minutes.to_string().contains(value))
            || self
                .location
                .as_deref()
                .is_some_and(|location| location.contains(value))
    }
}

pub(crate) struct PreparedTinyFishEgress {
    pub(crate) requests: Vec<TinyFishWireRequest>,
    pub(crate) review_command: Vec<String>,
}

pub(crate) fn prepare_tinyfish_egress(
    requests: &[TinyFishSearchRequest],
    api_key: &RedactedString,
) -> Result<PreparedTinyFishEgress, FunctionCallError> {
    reject_configured_api_key(requests, api_key)?;
    let source_requests = serialize_for_review(requests)?;
    reject_review_text(&source_requests, api_key)?;
    let serialized_source_requests = serialize_for_review(&source_requests)?;
    reject_review_text(&serialized_source_requests, api_key)?;

    let requests = requests
        .iter()
        .map(prepare_wire_request)
        .collect::<Result<Vec<_>, _>>()?;
    let review_requests = serialize_for_review(&requests)?;
    reject_review_text(&review_requests, api_key)?;
    reject_wire_requests(&requests, api_key)?;

    let command = vec!["web.run".to_string(), review_requests];
    let serialized_command = serde_json::to_string(&command).map_err(|err| {
        FunctionCallError::Fatal(format!(
            "failed to measure TinyFish web search security review payload: {err}"
        ))
    })?;
    reject_review_text(&serialized_command, api_key)?;
    if serialized_command.len() > MAX_TINYFISH_REVIEW_COMMAND_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "TinyFish web search request is too large for security review".to_string(),
        ));
    }
    Ok(PreparedTinyFishEgress {
        requests,
        review_command: command,
    })
}

fn serialize_for_review<T>(value: &T) -> Result<String, FunctionCallError>
where
    T: ?Sized + Serialize,
{
    serde_json::to_string(value).map_err(|err| {
        FunctionCallError::Fatal(format!(
            "failed to serialize TinyFish web search for security review: {err}"
        ))
    })
}

fn prepare_wire_request(
    request: &TinyFishSearchRequest,
) -> Result<TinyFishWireRequest, FunctionCallError> {
    let recency_minutes = request
        .recency_days
        .map(|days| {
            if !(1..=MAX_TINYFISH_RECENCY_DAYS).contains(&days) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "TinyFish web search recency must be between 1 and {MAX_TINYFISH_RECENCY_DAYS} days"
                )));
            }
            Ok(days * MINUTES_PER_DAY)
        })
        .transpose()?;
    Ok(TinyFishWireRequest {
        query: request.query.clone(),
        include_domains: request.domains.as_ref().map(|domains| domains.join(",")),
        recency_minutes,
        location: request.location.clone(),
    })
}

fn reject_wire_requests(
    requests: &[TinyFishWireRequest],
    api_key: &RedactedString,
) -> Result<(), FunctionCallError> {
    let api_key = api_key.as_str();
    if api_key.is_empty() {
        return Ok(());
    }
    let review_url = Url::parse(TINYFISH_REVIEW_URL).map_err(|err| {
        FunctionCallError::Fatal(format!(
            "failed to configure TinyFish web search security review URL: {err}"
        ))
    })?;
    if requests.iter().any(|request| {
        request.contains(api_key)
            || request
                .request_url(review_url.clone())
                .query()
                .is_some_and(|query| query.contains(api_key))
    }) {
        return Err(credentials_error());
    }
    Ok(())
}

fn reject_review_text(text: &str, api_key: &RedactedString) -> Result<(), FunctionCallError> {
    if (!api_key.as_str().is_empty() && text.contains(api_key.as_str()))
        || redact_secrets(text.to_string()) != text
    {
        return Err(credentials_error());
    }
    Ok(())
}

pub(crate) fn prepare_tinyfish_requests(
    commands: &TinyFishCommands,
    settings: &SearchSettings,
    api_key: &RedactedString,
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
    if configured_domains.is_some_and(<[String]>::is_empty) {
        return Err(FunctionCallError::RespondToModel(
            "TinyFish web search is blocked by the configured domain allowlist".to_string(),
        ));
    }
    let location = settings
        .user_location
        .as_ref()
        .and_then(|location| location.country.as_deref())
        .map(str::trim)
        .filter(|country| !country.is_empty())
        .map(str::to_string);

    let requests = commands
        .search_query
        .iter()
        .map(|query| {
            let query_text = query.q.trim();
            if query_text.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "TinyFish web search queries must not be empty".to_string(),
                ));
            }
            if redact_secrets(query_text.to_string()) != query_text {
                return Err(credentials_error());
            }
            if query
                .recency
                .is_some_and(|days| !(1..=MAX_TINYFISH_RECENCY_DAYS).contains(&days))
            {
                return Err(FunctionCallError::RespondToModel(
                    "TinyFish web search recency must be between 1 and 3650 days".to_string(),
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
        .collect::<Result<Vec<_>, _>>()?;
    reject_configured_api_key(&requests, api_key)?;
    Ok(requests)
}

fn reject_configured_api_key(
    requests: &[TinyFishSearchRequest],
    api_key: &RedactedString,
) -> Result<(), FunctionCallError> {
    let api_key = api_key.as_str();
    if !api_key.is_empty()
        && requests.iter().any(|request| {
            request.query.contains(api_key)
                || request
                    .domains
                    .as_ref()
                    .is_some_and(|domains| domains.iter().any(|domain| domain.contains(api_key)))
                || request
                    .location
                    .as_deref()
                    .is_some_and(|location| location.contains(api_key))
        })
    {
        return Err(credentials_error());
    }
    Ok(())
}

fn credentials_error() -> FunctionCallError {
    FunctionCallError::RespondToModel(
        "TinyFish web search queries must not contain credentials or secrets".to_string(),
    )
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
