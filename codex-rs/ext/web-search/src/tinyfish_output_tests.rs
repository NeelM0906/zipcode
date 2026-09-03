use codex_api::SearchResponseLength;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseInputItem;
use pretty_assertions::assert_eq;

use crate::tinyfish_output::MAX_TINYFISH_OUTPUT_BYTES;
use crate::tinyfish_output::TinyFishOutput;
use crate::tinyfish_output::TinyFishSearchResponse;
use crate::tinyfish_output::TinyFishSearchResult;
use crate::tinyfish_output::prepare_tinyfish_output;

#[test]
fn normalizes_the_documented_search_response_shape() {
    let response = serde_json::from_str::<TinyFishSearchResponse>(
        r#"{"query":"rust async traits","results":[{"position":1,"site_name":"docs.rs","title":"Rust documentation","snippet":"Traits define shared behavior.","url":"https://docs.rs/"}],"total_results":1,"page":0}"#,
    )
    .expect("documented TinyFish response should decode");
    let output = prepare_tinyfish_output(
        "call-1",
        vec![response],
        None,
        MAX_TINYFISH_OUTPUT_BYTES,
        "secret",
    )
    .expect("ordinary TinyFish output should prepare");

    let ResponseInputItem::FunctionCallOutput { output, .. } = output.response_item() else {
        panic!("TinyFish should return function output");
    };
    let [FunctionCallOutputContentItem::InputText { text }] = output
        .content_items()
        .expect("function output should contain content")
    else {
        panic!("TinyFish should return one text item");
    };
    let actual =
        serde_json::from_str::<serde_json::Value>(text).expect("model output should remain JSON");
    let expected = serde_json::from_str::<serde_json::Value>(
        r#"{"provider":"tinyfish","searches":[{"query":"rust async traits","results":[{"position":1,"site_name":"docs.rs","title":"Rust documentation","snippet":"Traits define shared behavior.","url":"https://docs.rs/"}]}]}"#,
    )
    .expect("expected output should be valid JSON");
    assert_eq!(actual, expected);
}

#[test]
fn redacts_a_reflected_api_key_from_every_typed_string_field() {
    let api_key = "tinyfish-top-secret";
    let reflected = format!("before {api_key} after");
    let mut result = search_result(1, &reflected);
    result.date = Some(reflected.clone());
    result.publisher = Some(reflected.clone());
    result.authors = Some(vec![reflected.clone()]);
    result.venue = Some(reflected.clone());
    result.pdf_url = Some(reflected.clone());
    let response = search_response(reflected, vec![result]);
    let output = prepare_tinyfish_output(
        "call-1",
        vec![response],
        None,
        MAX_TINYFISH_OUTPUT_BYTES,
        api_key,
    )
    .expect("reflected credentials should be redacted");

    for serialized in serialized_wrappers(&output) {
        assert!(!serialized.contains(api_key));
        assert!(serialized.contains("[REDACTED]"));
    }
}

#[test]
fn caps_every_final_serialized_wrapper_with_large_utf8_fields_and_query() {
    let huge = "🐟".repeat(8_000);
    let response = search_response(
        "q".repeat(20_000),
        (1..=10)
            .map(|position| search_result(position, &huge))
            .collect(),
    );
    for (requested_budget, expected_budget) in
        [(5_000, 5_000), (usize::MAX, MAX_TINYFISH_OUTPUT_BYTES)]
    {
        let output = prepare_tinyfish_output(
            "call-1",
            vec![response.clone()],
            Some(SearchResponseLength::Short),
            requested_budget,
            "secret",
        )
        .expect("large output should be bounded without breaking UTF-8");
        for serialized in serialized_wrappers(&output) {
            assert!(serialized.len() <= expected_budget);
        }
        let item =
            serde_json::to_value(output.extension_item()).expect("extension item should serialize");
        assert_eq!(item["results"].as_array().map(Vec::len), Some(5));
    }
}

#[test]
fn rejects_output_when_immutable_wrapper_metadata_exceeds_the_limit() {
    let response = search_response(String::new(), Vec::new());
    let error = prepare_tinyfish_output(
        &"c".repeat(20_000),
        vec![response],
        None,
        MAX_TINYFISH_OUTPUT_BYTES,
        "secret",
    )
    .expect_err("an oversized immutable call id cannot be made safe");

    assert!(error.to_string().contains("available output budget"));
}

#[test]
fn prepared_output_cannot_be_rewrapped_with_unverified_metadata() {
    let response = search_response("rust".to_string(), vec![search_result(1, "result")]);
    let output = prepare_tinyfish_output(
        "call-1",
        vec![response],
        None,
        MAX_TINYFISH_OUTPUT_BYTES,
        "secret",
    )
    .expect("ordinary output should prepare");

    for serialized in serialized_wrappers(&output) {
        assert!(serialized.len() <= MAX_TINYFISH_OUTPUT_BYTES);
        assert!(serialized.contains("call-1"));
    }
}

fn serialized_wrappers(output: &TinyFishOutput) -> [String; 3] {
    [
        serde_json::to_string(&output.response_item())
            .expect("response input item should serialize"),
        serde_json::to_string(&output.extension_item()).expect("extension item should serialize"),
        serde_json::to_string(&output.legacy_event()).expect("legacy event should serialize"),
    ]
}

fn search_response(query: String, results: Vec<TinyFishSearchResult>) -> TinyFishSearchResponse {
    TinyFishSearchResponse {
        query,
        total_results: results.len() as u64,
        results,
        page: 0,
    }
}

fn search_result(position: u64, value: &str) -> TinyFishSearchResult {
    TinyFishSearchResult {
        position,
        site_name: value.to_string(),
        title: value.to_string(),
        snippet: value.to_string(),
        url: format!("https://example.com/{value}"),
        date: None,
        publisher: None,
        authors: None,
        venue: None,
        year: None,
        cited_by_count: None,
        pdf_url: None,
    }
}
