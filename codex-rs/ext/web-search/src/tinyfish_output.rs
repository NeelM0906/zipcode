use codex_api::SearchResponseLength;
use codex_extension_api::FunctionCallError;
use codex_extension_items::ExtensionItem;
use codex_extension_items::web_search::WebSearchAction;
use codex_extension_items::web_search::WebSearchItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::WebSearchAction as CoreWebSearchAction;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WebSearchEndEvent;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

pub(crate) const MAX_TINYFISH_OUTPUT_BYTES: usize = 10_000;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TinyFishSearchResponse {
    pub(crate) query: String,
    pub(crate) results: Vec<TinyFishSearchResult>,
    pub(crate) total_results: u64,
    #[serde(default)]
    pub(crate) page: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TinyFishSearchResult {
    #[serde(default)]
    pub(crate) position: u64,
    #[serde(default)]
    pub(crate) site_name: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) snippet: String,
    #[serde(default)]
    pub(crate) url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) authors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) venue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) year: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cited_by_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pdf_url: Option<String>,
}

impl TinyFishSearchResult {
    pub(crate) fn for_each_string_mut(&mut self, mut visit: impl FnMut(&mut String)) {
        for value in [
            &mut self.site_name,
            &mut self.title,
            &mut self.snippet,
            &mut self.url,
        ] {
            visit(value);
        }
        for value in [
            &mut self.date,
            &mut self.publisher,
            &mut self.venue,
            &mut self.pdf_url,
        ]
        .into_iter()
        .flatten()
        {
            visit(value);
        }
        for author in self.authors.iter_mut().flatten() {
            visit(author);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TinyFishOutput {
    call_id: String,
    output: String,
    results: Vec<Value>,
    queries: Vec<String>,
}

impl TinyFishOutput {
    pub(crate) fn response_item(&self) -> ResponseInputItem {
        ResponseInputItem::FunctionCallOutput {
            call_id: self.call_id.clone(),
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::InputText {
                    text: self.output.clone(),
                },
            ]),
        }
    }

    pub(crate) fn extension_item(&self) -> ExtensionItem {
        let (item, _) = self.completion_items();
        ExtensionItem::WebSearch(item)
    }

    pub(crate) fn legacy_event(&self) -> EventMsg {
        let (_, event) = self.completion_items();
        EventMsg::WebSearchEnd(event)
    }

    fn completion_items(&self) -> (WebSearchItem, WebSearchEndEvent) {
        let (query, queries) = match self.queries.as_slice() {
            [query] => (Some(query.clone()), None),
            queries => (None, Some(queries.to_vec())),
        };
        let command_action = WebSearchAction::Search {
            query: query.clone(),
            queries: queries.clone(),
        };
        let legacy_action = CoreWebSearchAction::Search { query, queries };
        let query = codex_core::web_search_action_detail(&legacy_action);
        (
            WebSearchItem {
                id: self.call_id.clone(),
                query: query.clone(),
                action: Some(command_action),
                results: Some(self.results.clone()),
            },
            WebSearchEndEvent {
                call_id: self.call_id.clone(),
                query,
                action: legacy_action,
                results: Some(self.results.clone()),
            },
        )
    }
}

pub(crate) fn prepare_tinyfish_output(
    call_id: &str,
    mut responses: Vec<TinyFishSearchResponse>,
    response_length: Option<SearchResponseLength>,
    response_byte_budget: usize,
    api_key: &str,
) -> Result<TinyFishOutput, FunctionCallError> {
    let response_byte_budget = response_byte_budget.min(MAX_TINYFISH_OUTPUT_BYTES);
    redact_api_key(&mut responses, api_key);
    let result_limit = match response_length {
        Some(SearchResponseLength::Short) => 5,
        Some(SearchResponseLength::Medium | SearchResponseLength::Long) | None => 10,
    };
    for response in &mut responses {
        response.results.truncate(result_limit);
    }

    let unbounded = build_output(call_id, &responses, usize::MAX)?;
    if wrappers_fit(&unbounded, response_byte_budget)? {
        return Ok(unbounded);
    }

    loop {
        let minimum = build_output(call_id, &responses, 0)?;
        if wrappers_fit(&minimum, response_byte_budget)? {
            let mut best = minimum;
            let mut lower_bound = 1;
            let mut upper_bound = max_field_bytes(&responses);
            while lower_bound <= upper_bound {
                let field_byte_budget = lower_bound + (upper_bound - lower_bound) / 2;
                let candidate = build_output(call_id, &responses, field_byte_budget)?;
                if wrappers_fit(&candidate, response_byte_budget)? {
                    best = candidate;
                    lower_bound = field_byte_budget.saturating_add(1);
                } else {
                    upper_bound = field_byte_budget - 1;
                }
            }
            return Ok(best);
        }

        let Some(response) = responses
            .iter_mut()
            .rev()
            .find(|response| !response.results.is_empty())
        else {
            return Err(FunctionCallError::RespondToModel(
                "TinyFish output exceeds the available output budget".to_string(),
            ));
        };
        response.results.pop();
    }
}

fn wrappers_fit(
    output: &TinyFishOutput,
    response_byte_budget: usize,
) -> Result<bool, FunctionCallError> {
    for value in [
        serde_json::to_vec(&output.response_item()),
        serde_json::to_vec(&output.extension_item()),
        serde_json::to_vec(&output.legacy_event()),
    ] {
        if value
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?
            .len()
            > response_byte_budget
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn build_output(
    call_id: &str,
    responses: &[TinyFishSearchResponse],
    field_byte_budget: usize,
) -> Result<TinyFishOutput, FunctionCallError> {
    let mut responses = responses.to_vec();
    truncate_responses(&mut responses, field_byte_budget);
    let searches = responses
        .iter()
        .map(|response| TinyFishSearchView {
            query: &response.query,
            results: &response.results,
        })
        .collect();
    let output = serde_json::to_string_pretty(&TinyFishModelOutput {
        provider: "tinyfish",
        searches,
    })
    .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
    let results = responses
        .iter()
        .flat_map(|response| response.results.iter())
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
    let queries = responses
        .into_iter()
        .map(|response| response.query)
        .collect();
    Ok(TinyFishOutput {
        call_id: call_id.to_string(),
        output,
        results,
        queries,
    })
}

#[derive(Serialize)]
struct TinyFishModelOutput<'a> {
    provider: &'static str,
    searches: Vec<TinyFishSearchView<'a>>,
}

#[derive(Serialize)]
struct TinyFishSearchView<'a> {
    query: &'a str,
    results: &'a [TinyFishSearchResult],
}

fn redact_api_key(responses: &mut [TinyFishSearchResponse], api_key: &str) {
    if api_key.trim().is_empty() {
        return;
    }
    for response in responses {
        redact_string(&mut response.query, api_key);
        for result in &mut response.results {
            result.for_each_string_mut(|value| redact_string(value, api_key));
        }
    }
}

fn redact_string(value: &mut String, api_key: &str) {
    if value.contains(api_key) {
        *value = value.replace(api_key, "[REDACTED]");
    }
}

fn max_field_bytes(responses: &[TinyFishSearchResponse]) -> usize {
    responses
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
        .unwrap_or(0)
}

fn truncate_responses(responses: &mut [TinyFishSearchResponse], field_byte_budget: usize) {
    for response in responses {
        truncate_string(&mut response.query, field_byte_budget);
        for result in &mut response.results {
            result.for_each_string_mut(|value| truncate_string(value, field_byte_budget));
        }
    }
}

fn truncate_string(value: &mut String, byte_budget: usize) {
    if value.len() <= byte_budget {
        return;
    }
    let mut boundary = byte_budget;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}
