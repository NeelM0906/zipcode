use codex_api::SearchSettings;
use codex_extension_api::FunctionCallError;
use codex_secrets::redact_secrets;
use codex_utils_redacted_string::RedactedString;
use serde::Serialize;

use crate::schema::TinyFishCommands;

pub(crate) const MAX_TINYFISH_RECENCY_DAYS: u64 = 3_650;
const MAX_TINYFISH_REVIEW_COMMAND_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TinyFishSearchRequest {
    pub(crate) query: String,
    pub(crate) domains: Option<Vec<String>>,
    pub(crate) recency_days: Option<u64>,
    pub(crate) location: Option<String>,
}

pub(crate) fn tinyfish_review_command(
    requests: &[TinyFishSearchRequest],
    api_key: &RedactedString,
) -> Result<Vec<String>, FunctionCallError> {
    reject_configured_api_key(requests, api_key)?;
    let requests = serde_json::to_string(requests).map_err(|err| {
        FunctionCallError::Fatal(format!(
            "failed to serialize TinyFish web search for security review: {err}"
        ))
    })?;
    if redact_secrets(requests.clone()) != requests {
        return Err(credentials_error());
    }
    let command = vec!["web.run".to_string(), requests];
    let serialized_len = serde_json::to_vec(&command)
        .map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to measure TinyFish web search security review payload: {err}"
            ))
        })?
        .len();
    if serialized_len > MAX_TINYFISH_REVIEW_COMMAND_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "TinyFish web search request is too large for security review".to_string(),
        ));
    }
    Ok(command)
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
