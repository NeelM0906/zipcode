use codex_api::ApproximateLocation;
use codex_api::LocationType;
use codex_api::SearchFilters;
use codex_api::SearchResponseLength;
use codex_api::SearchSettings;
use codex_extension_api::FunctionCallError;
use codex_utils_redacted_string::RedactedString;
use pretty_assertions::assert_eq;

use crate::schema::TinyFishCommands;
use crate::schema::TinyFishSearchQuery;
use crate::schema::tinyfish_commands_schema;
use crate::tinyfish_request::TinyFishSearchRequest;
use crate::tinyfish_request::prepare_tinyfish_requests as prepare_tinyfish_requests_with_api_key;
use crate::tinyfish_request::tinyfish_review_command;

const API_KEY: &str = "x7Qp4mN9vL2sR8tW";

#[test]
fn tinyfish_schema_accepts_only_one_to_four_search_queries() {
    let schema = tinyfish_commands_schema();
    let properties = schema["properties"]
        .as_object()
        .expect("schema properties should be an object");

    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["response_length", "search_query"]
    );
    assert_eq!(schema["required"], serde_json::json!(["search_query"]));
    assert_eq!(schema["properties"]["search_query"]["minItems"], 1);
    assert_eq!(schema["properties"]["search_query"]["maxItems"], 4);
    assert_eq!(
        schema["properties"]["search_query"]["items"]["additionalProperties"],
        false
    );
}

#[test]
fn tinyfish_commands_reject_non_search_operations() {
    let commands = serde_json::from_str::<TinyFishCommands>(
        r#"{"search_query":[{"q":"rust"}],"response_length":"short"}"#,
    )
    .expect("TinyFish should accept the search-only contract");
    assert_eq!(commands.response_length, Some(SearchResponseLength::Short));

    let error = serde_json::from_str::<TinyFishCommands>(
        r#"{"search_query":[{"q":"rust"}],"open":[{"ref_id":"https://example.com"}]}"#,
    )
    .expect_err("TinyFish should reject commands outside search_query");

    assert!(error.to_string().contains("unknown field `open`"));

    let error = serde_json::from_str::<TinyFishCommands>(
        r#"{"search_query":[{"q":"rust","unsupported":true}]}"#,
    )
    .expect_err("TinyFish should reject unknown search query fields");

    assert!(error.to_string().contains("unknown field `unsupported`"));
}

#[test]
fn prepares_trimmed_query_with_allowlist_intersection_and_country_only() {
    let commands = TinyFishCommands {
        search_query: vec![TinyFishSearchQuery {
            q: "  rust async traits  ".to_string(),
            recency: Some(2),
            domains: Some(vec![
                "EXAMPLE.COM".to_string(),
                "attacker.example".to_string(),
                "DOCS.RS".to_string(),
            ]),
        }],
        response_length: None,
    };
    let settings = SearchSettings {
        user_location: Some(ApproximateLocation {
            r#type: LocationType::Approximate,
            country: Some(" US ".to_string()),
            region: Some("NY".to_string()),
            city: Some("New York".to_string()),
            timezone: Some("America/New_York".to_string()),
        }),
        filters: Some(SearchFilters {
            allowed_domains: Some(vec!["docs.rs".to_string(), "example.com".to_string()]),
            blocked_domains: None,
        }),
        ..Default::default()
    };

    assert_eq!(
        prepare_tinyfish_requests(&commands, &settings),
        Ok(vec![TinyFishSearchRequest {
            query: "rust async traits".to_string(),
            domains: Some(vec!["docs.rs".to_string(), "example.com".to_string()]),
            recency_days: Some(2),
            location: Some("US".to_string()),
        }])
    );
}

#[test]
fn omits_location_when_country_is_blank() {
    let commands = TinyFishCommands {
        search_query: vec![search_query("rust")],
        response_length: None,
    };
    let settings = SearchSettings {
        user_location: Some(ApproximateLocation {
            r#type: LocationType::Approximate,
            country: Some("   ".to_string()),
            region: Some("private-region".to_string()),
            city: Some("private-city".to_string()),
            timezone: Some("private/timezone".to_string()),
        }),
        ..Default::default()
    };

    assert_eq!(
        prepare_tinyfish_requests(&commands, &settings),
        Ok(vec![TinyFishSearchRequest {
            query: "rust".to_string(),
            domains: None,
            recency_days: None,
            location: None,
        }])
    );
}

#[test]
fn rejects_query_batches_outside_one_to_four() {
    for search_query in [Vec::new(), vec![search_query("rust"); 5]] {
        let commands = TinyFishCommands {
            search_query,
            response_length: None,
        };

        assert_eq!(
            prepare_tinyfish_requests(&commands, &SearchSettings::default()),
            Err(codex_extension_api::FunctionCallError::RespondToModel(
                "TinyFish web search accepts one to four queries".to_string()
            ))
        );
    }
}

#[test]
fn rejects_blank_queries() {
    let commands = TinyFishCommands {
        search_query: vec![search_query(" \t\n ")],
        response_length: None,
    };

    assert_eq!(
        prepare_tinyfish_requests(&commands, &SearchSettings::default()),
        Err(codex_extension_api::FunctionCallError::RespondToModel(
            "TinyFish web search queries must not be empty".to_string()
        ))
    );
}

#[test]
fn rejects_queries_that_contain_recognized_secrets() {
    for query in [
        "find sk-abcdefghijklmnopqrstuv",
        "search token=abcdefghijklmnop",
    ] {
        let commands = TinyFishCommands {
            search_query: vec![search_query(query)],
            response_length: None,
        };

        assert_eq!(
            prepare_tinyfish_requests(&commands, &SearchSettings::default()),
            Err(codex_extension_api::FunctionCallError::RespondToModel(
                "TinyFish web search queries must not contain credentials or secrets".to_string()
            ))
        );
    }
}

#[test]
fn review_command_rejects_the_configured_api_key_from_every_request_field() {
    let api_key = RedactedString::from(API_KEY);
    let requests = [
        TinyFishSearchRequest {
            query: format!("find {API_KEY}"),
            domains: None,
            recency_days: None,
            location: None,
        },
        TinyFishSearchRequest {
            query: "rust".to_string(),
            domains: Some(vec![format!("docs.{API_KEY}.example")]),
            recency_days: None,
            location: None,
        },
        TinyFishSearchRequest {
            query: "rust".to_string(),
            domains: None,
            recency_days: None,
            location: Some(format!("US-{API_KEY}")),
        },
    ];

    for request in requests {
        assert_eq!(
            tinyfish_review_command(&[request], &api_key),
            Err(FunctionCallError::RespondToModel(
                "TinyFish web search queries must not contain credentials or secrets".to_string()
            ))
        );
    }
}

#[test]
fn rejects_recency_outside_the_supported_range() {
    for recency in [0, 3_651, u64::MAX] {
        let mut query = search_query("rust");
        query.recency = Some(recency);
        let commands = TinyFishCommands {
            search_query: vec![query],
            response_length: None,
        };

        assert_eq!(
            prepare_tinyfish_requests(&commands, &SearchSettings::default()),
            Err(codex_extension_api::FunctionCallError::RespondToModel(
                "TinyFish web search recency must be between 1 and 3650 days".to_string()
            ))
        );
    }
}

#[test]
fn preserves_the_available_domain_filter() {
    let cases = [
        (
            Some(vec!["docs.rs".to_string()]),
            None,
            Some(vec!["docs.rs".to_string()]),
        ),
        (
            None,
            Some(vec!["example.com".to_string()]),
            Some(vec!["example.com".to_string()]),
        ),
        (None, None, None),
    ];

    for (configured, requested, expected) in cases {
        let mut query = search_query("rust");
        query.domains = requested;
        let commands = TinyFishCommands {
            search_query: vec![query],
            response_length: None,
        };
        let settings = SearchSettings {
            filters: Some(SearchFilters {
                allowed_domains: configured,
                blocked_domains: None,
            }),
            ..Default::default()
        };

        assert_eq!(
            prepare_tinyfish_requests(&commands, &settings),
            Ok(vec![TinyFishSearchRequest {
                query: "rust".to_string(),
                domains: expected,
                recency_days: None,
                location: None,
            }])
        );
    }
}

#[test]
fn rejects_an_empty_configured_domain_intersection() {
    let mut query = search_query("rust");
    query.domains = Some(vec!["attacker.example".to_string()]);
    let commands = TinyFishCommands {
        search_query: vec![query],
        response_length: None,
    };
    let settings = SearchSettings {
        filters: Some(SearchFilters {
            allowed_domains: Some(vec!["docs.rs".to_string()]),
            blocked_domains: None,
        }),
        ..Default::default()
    };

    assert_eq!(
        prepare_tinyfish_requests(&commands, &settings),
        Err(codex_extension_api::FunctionCallError::RespondToModel(
            "requested domains do not overlap the configured TinyFish allowlist".to_string()
        ))
    );
}

#[test]
fn rejects_an_empty_configured_allowlist() {
    let commands = TinyFishCommands {
        search_query: vec![search_query("rust")],
        response_length: None,
    };
    let settings = SearchSettings {
        filters: Some(SearchFilters {
            allowed_domains: Some(Vec::new()),
            blocked_domains: None,
        }),
        ..Default::default()
    };

    assert_eq!(
        prepare_tinyfish_requests(&commands, &settings),
        Err(codex_extension_api::FunctionCallError::RespondToModel(
            "TinyFish web search is blocked by the configured domain allowlist".to_string()
        ))
    );
}

fn search_query(q: &str) -> TinyFishSearchQuery {
    TinyFishSearchQuery {
        q: q.to_string(),
        recency: None,
        domains: None,
    }
}

fn prepare_tinyfish_requests(
    commands: &TinyFishCommands,
    settings: &SearchSettings,
) -> Result<Vec<TinyFishSearchRequest>, FunctionCallError> {
    prepare_tinyfish_requests_with_api_key(commands, settings, &RedactedString::from(API_KEY))
}
