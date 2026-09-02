use codex_api::SearchResponseLength;
use codex_api::SearchSettings;
use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolOutput;
use codex_extension_items::web_search::WebSearchAction;
use codex_extension_items::web_search::WebSearchItem;
use codex_http_client::HttpClientFactory;
use codex_protocol::models::WebSearchAction as CoreWebSearchAction;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TruncationPolicy;
use codex_protocol::protocol::WebSearchBeginEvent;
use codex_protocol::protocol::WebSearchEndEvent;
use codex_utils_redacted_string::RedactedString;
use serde::Serialize;
use url::Url;

use super::WebSearchTool;
use super::extension_turn_item;
use super::record_results_payload_bytes;
use crate::output::SearchOutput;
use crate::schema::TinyFishCommands;
use crate::tinyfish::TinyFishSearchClient;
use crate::tinyfish::TinyFishSearchRequest;
use crate::tinyfish::TinyFishSearchResponse;

const MAX_TINYFISH_RESPONSE_TOKENS: usize = 10_000;

pub(super) async fn handle_tinyfish_call(
    tool: &WebSearchTool,
    call: ToolCall<'_>,
    http_client_factory: &HttpClientFactory,
    endpoint: &Url,
    api_key: Option<&RedactedString>,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let commands = parse_tinyfish_commands(&call)?;
    let requests = prepare_tinyfish_requests(&commands, &tool.settings)?;
    let api_key = api_key
        .filter(|api_key| !api_key.as_str().trim().is_empty())
        .ok_or_else(|| {
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
    let response_byte_budget = call
        .response_byte_budget(TruncationPolicy::Tokens(MAX_TINYFISH_RESPONSE_TOKENS).byte_budget());
    let formatted =
        format_tinyfish_output(responses, commands.response_length, response_byte_budget)?;
    record_results_payload_bytes(&formatted.results);

    let (query, queries) = match formatted.queries.as_slice() {
        [query] => (Some(query.clone()), None),
        queries => (None, Some(queries.to_vec())),
    };
    let command_action = WebSearchAction::Search {
        query: query.clone(),
        queries: queries.clone(),
    };
    let legacy_action = CoreWebSearchAction::Search { query, queries };
    let query = codex_core::web_search_action_detail(&legacy_action);
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

fn parse_tinyfish_commands(call: &ToolCall<'_>) -> Result<TinyFishCommands, FunctionCallError> {
    serde_json::from_str(call.function_arguments()?)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

pub(super) fn prepare_tinyfish_requests(
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
    let location = settings
        .user_location
        .as_ref()
        .and_then(|location| location.country.as_deref())
        .map(str::trim)
        .filter(|country| !country.is_empty())
        .map(str::to_string);

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
            if query
                .recency
                .is_some_and(|days| days.checked_mul(24 * 60).is_none())
            {
                return Err(FunctionCallError::RespondToModel(
                    "TinyFish web search recency is too large".to_string(),
                ));
            }
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

pub(super) struct TinyFishFormattedOutput {
    pub(super) output: String,
    pub(super) results: Vec<serde_json::Value>,
    pub(super) queries: Vec<String>,
}

#[derive(Serialize)]
struct TinyFishOutputView<'a> {
    provider: &'static str,
    searches: Vec<TinyFishSearchView<'a>>,
}

#[derive(Serialize)]
struct TinyFishSearchView<'a> {
    query: BoundedString<'a>,
    results: Vec<TinyFishResultView<'a>>,
}

#[derive(Serialize)]
struct TinyFishResultView<'a> {
    position: u64,
    site_name: BoundedString<'a>,
    title: BoundedString<'a>,
    snippet: BoundedString<'a>,
    url: BoundedString<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<BoundedString<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    publisher: Option<BoundedString<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    authors: Option<Vec<BoundedString<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    venue: Option<BoundedString<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    year: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cited_by_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pdf_url: Option<BoundedString<'a>>,
}

#[derive(Clone, Copy)]
struct BoundedString<'a> {
    value: &'a str,
    byte_budget: usize,
}

impl Serialize for BoundedString<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(bounded_prefix(self.value, self.byte_budget))
    }
}

pub(super) fn format_tinyfish_output(
    mut responses: Vec<TinyFishSearchResponse>,
    response_length: Option<SearchResponseLength>,
    response_byte_budget: usize,
) -> Result<TinyFishFormattedOutput, FunctionCallError> {
    let result_limit = match response_length {
        Some(SearchResponseLength::Short) => 5,
        Some(SearchResponseLength::Medium | SearchResponseLength::Long) | None => 10,
    };
    for response in &mut responses {
        response.results.truncate(result_limit);
    }

    let output = serialize_tinyfish_output(&responses, usize::MAX)?;
    if output.len() <= response_byte_budget {
        return build_tinyfish_formatted_output(responses, output);
    }
    drop(output);

    let mut best_output = serialize_tinyfish_output(&responses, 0)?;
    while best_output.len() > response_byte_budget {
        let Some(response) = responses
            .iter_mut()
            .rev()
            .find(|response| !response.results.is_empty())
        else {
            return Err(FunctionCallError::RespondToModel(
                "TinyFish response exceeds the available output budget".to_string(),
            ));
        };
        response.results.pop();
        best_output = serialize_tinyfish_output(&responses, 0)?;
    }

    let max_field_bytes = responses
        .iter()
        .flat_map(|response| {
            std::iter::once(response.query.len()).chain(response.results.iter().flat_map(
                |result| {
                    [
                        Some(result.site_name.len()),
                        Some(result.title.len()),
                        Some(result.snippet.len()),
                        Some(result.url.len()),
                        result.date.as_ref().map(String::len),
                        result.publisher.as_ref().map(String::len),
                        result.venue.as_ref().map(String::len),
                        result.pdf_url.as_ref().map(String::len),
                    ]
                    .into_iter()
                    .flatten()
                    .chain(result.authors.iter().flatten().map(String::len))
                },
            ))
        })
        .max()
        .unwrap_or(0);
    let mut lower_bound = 0;
    let mut upper_bound = max_field_bytes;
    let mut best_field_byte_budget = 0;
    while lower_bound <= upper_bound {
        let field_byte_budget = lower_bound + (upper_bound - lower_bound) / 2;
        let candidate = serialize_tinyfish_output(&responses, field_byte_budget)?;
        if candidate.len() <= response_byte_budget {
            best_output = candidate;
            best_field_byte_budget = field_byte_budget;
            lower_bound = field_byte_budget.saturating_add(1);
        } else if field_byte_budget == 0 {
            break;
        } else {
            upper_bound = field_byte_budget - 1;
        }
    }

    truncate_tinyfish_responses(&mut responses, best_field_byte_budget);
    build_tinyfish_formatted_output(responses, best_output)
}

fn truncate_tinyfish_responses(responses: &mut [TinyFishSearchResponse], field_byte_budget: usize) {
    for response in responses {
        truncate_to_byte_budget(&mut response.query, field_byte_budget);
        for result in &mut response.results {
            truncate_to_byte_budget(&mut result.site_name, field_byte_budget);
            truncate_to_byte_budget(&mut result.title, field_byte_budget);
            truncate_to_byte_budget(&mut result.snippet, field_byte_budget);
            truncate_to_byte_budget(&mut result.url, field_byte_budget);
            for value in [
                &mut result.date,
                &mut result.publisher,
                &mut result.venue,
                &mut result.pdf_url,
            ]
            .into_iter()
            .flatten()
            {
                truncate_to_byte_budget(value, field_byte_budget);
            }
            for author in result.authors.iter_mut().flatten() {
                truncate_to_byte_budget(author, field_byte_budget);
            }
        }
    }
}

fn truncate_to_byte_budget(value: &mut String, byte_budget: usize) {
    if value.len() <= byte_budget {
        return;
    }
    let mut boundary = byte_budget;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn bounded_prefix(value: &str, byte_budget: usize) -> &str {
    if value.len() <= byte_budget {
        return value;
    }
    let mut boundary = byte_budget;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

fn serialize_tinyfish_output(
    responses: &[TinyFishSearchResponse],
    field_byte_budget: usize,
) -> Result<String, FunctionCallError> {
    let searches = responses
        .iter()
        .map(|response| TinyFishSearchView {
            query: BoundedString {
                value: &response.query,
                byte_budget: field_byte_budget,
            },
            results: response
                .results
                .iter()
                .map(|result| TinyFishResultView {
                    position: result.position,
                    site_name: BoundedString {
                        value: &result.site_name,
                        byte_budget: field_byte_budget,
                    },
                    title: BoundedString {
                        value: &result.title,
                        byte_budget: field_byte_budget,
                    },
                    snippet: BoundedString {
                        value: &result.snippet,
                        byte_budget: field_byte_budget,
                    },
                    url: BoundedString {
                        value: &result.url,
                        byte_budget: field_byte_budget,
                    },
                    date: result.date.as_deref().map(|value| BoundedString {
                        value,
                        byte_budget: field_byte_budget,
                    }),
                    publisher: result.publisher.as_deref().map(|value| BoundedString {
                        value,
                        byte_budget: field_byte_budget,
                    }),
                    authors: result.authors.as_ref().map(|authors| {
                        authors
                            .iter()
                            .map(|value| BoundedString {
                                value,
                                byte_budget: field_byte_budget,
                            })
                            .collect()
                    }),
                    venue: result.venue.as_deref().map(|value| BoundedString {
                        value,
                        byte_budget: field_byte_budget,
                    }),
                    year: result.year,
                    cited_by_count: result.cited_by_count,
                    pdf_url: result.pdf_url.as_deref().map(|value| BoundedString {
                        value,
                        byte_budget: field_byte_budget,
                    }),
                })
                .collect(),
        })
        .collect();
    serde_json::to_string_pretty(&TinyFishOutputView {
        provider: "tinyfish",
        searches,
    })
    .map_err(|err| FunctionCallError::Fatal(err.to_string()))
}

fn build_tinyfish_formatted_output(
    responses: Vec<TinyFishSearchResponse>,
    output: String,
) -> Result<TinyFishFormattedOutput, FunctionCallError> {
    let results = responses
        .iter()
        .flat_map(|response| response.results.iter())
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
    let queries = responses
        .iter()
        .map(|response| response.query.clone())
        .collect::<Vec<_>>();
    Ok(TinyFishFormattedOutput {
        output,
        results,
        queries,
    })
}

#[cfg(test)]
#[path = "tinyfish_tool_tests.rs"]
mod tests;
