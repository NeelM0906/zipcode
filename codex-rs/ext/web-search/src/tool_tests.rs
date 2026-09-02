use codex_api::SearchCommands;
use codex_core::X_CODEX_TURN_METADATA_HEADER;
use codex_extension_items::web_search::WebSearchAction;
use pretty_assertions::assert_eq;

use super::command_action;
use super::search_request_headers;

#[test]
fn search_request_headers_forward_thread_originator_and_turn_metadata() {
    let headers = search_request_headers(Some("chatgpt_cca"), Some("turn-metadata"));
    assert_eq!(
        headers
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("chatgpt_cca")
    );
    assert_eq!(
        headers
            .get(X_CODEX_TURN_METADATA_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("turn-metadata")
    );
}

#[test]
fn command_action_reports_queries_and_navigation_detail() {
    let cases = [
        (
            r#"{"image_query":[{"q":"waterfalls"},{"q":"mountains"}]}"#,
            WebSearchAction::Search {
                query: None,
                queries: Some(vec!["waterfalls".to_string(), "mountains".to_string()]),
            },
        ),
        (
            r#"{"open":[{"ref_id":"https://example.com/docs"}]}"#,
            WebSearchAction::OpenPage {
                url: Some("https://example.com/docs".to_string()),
            },
        ),
        (
            r#"{"find":[{"ref_id":"https://example.com/docs","pattern":"install"}]}"#,
            WebSearchAction::FindInPage {
                url: Some("https://example.com/docs".to_string()),
                pattern: Some("install".to_string()),
            },
        ),
        (
            r#"{"find":[{"ref_id":"turn0search0","pattern":"install"}]}"#,
            WebSearchAction::FindInPage {
                url: None,
                pattern: Some("install".to_string()),
            },
        ),
        (
            r#"{"open":[{"ref_id":"turn0search0"}]}"#,
            WebSearchAction::Other,
        ),
    ];

    for (arguments, expected) in cases {
        let commands: SearchCommands =
            serde_json::from_str(arguments).expect("valid search command arguments");
        assert_eq!(command_action(&commands), expected);
    }
}
