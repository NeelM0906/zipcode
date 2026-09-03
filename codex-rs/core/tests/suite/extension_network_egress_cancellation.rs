use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::GuardianAssessmentStatus;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use codex_tools::ToolNetworkEgress;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;

use super::extension_network_egress::TOOL_NAME;
use super::extension_network_egress::build_fake_egress_test;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupting_extension_egress_aborts_guardian_and_allows_the_next_review() -> Result<()> {
    skip_if_no_network!(Ok(()));

    const CANCEL_CALL_ID: &str = "cancel-extension-egress";
    const NEXT_CALL_ID: &str = "next-extension-egress";
    const INITIAL_PROMPT: &str = "start a cancellable extension egress review";
    const NEXT_PROMPT: &str = "run the extension egress tool again";

    let server = responses::start_mock_server().await;
    let initial_parent = responses::mount_response_once_match(
        &server,
        |request: &wiremock::Request| {
            !is_guardian_request(request) && request_contains(request, INITIAL_PROMPT)
        },
        responses::sse_response(responses::sse(vec![
            responses::ev_response_created("cancel-parent"),
            responses::ev_function_call(CANCEL_CALL_ID, TOOL_NAME, "{}"),
            responses::ev_completed("cancel-parent"),
        ])),
    )
    .await;
    let pending_guardian = responses::mount_response_once_match(
        &server,
        |request: &wiremock::Request| {
            is_guardian_request(request) && request_contains(request, CANCEL_CALL_ID)
        },
        responses::sse_response(responses::sse(vec![
            responses::ev_response_created("cancel-guardian"),
            responses::ev_assistant_message(
                "cancel-guardian-decision",
                &serde_json::json!({
                    "risk_level": "low",
                    "user_authorization": "high",
                    "outcome": "allow",
                    "rationale": "This delayed review should be interrupted.",
                })
                .to_string(),
            ),
            responses::ev_completed("cancel-guardian"),
        ]))
        .set_delay(Duration::from_secs(30)),
    )
    .await;
    let handler_calls = Arc::new(AtomicUsize::new(0));
    let test = build_fake_egress_test(&server, valid_egress(), Arc::clone(&handler_calls)).await?;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: INITIAL_PROMPT.into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    wait_for_request(&pending_guardian).await?;
    test.codex.submit(Op::Interrupt).await?;

    tokio::time::timeout(Duration::from_secs(10), async {
        let mut guardian_aborted = false;
        let mut turn_aborted = false;
        while !guardian_aborted || !turn_aborted {
            let event = test.codex.next_event().await?;
            guardian_aborted |= matches!(
                &event.msg,
                EventMsg::GuardianAssessment(assessment)
                    if assessment.status == GuardianAssessmentStatus::Aborted
            );
            turn_aborted |= matches!(&event.msg, EventMsg::TurnAborted(_));
        }
        anyhow::Ok(())
    })
    .await
    .context("extension egress interrupt did not finish parent and Guardian cancellation")??;
    assert_eq!(initial_parent.requests().len(), 1);
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);

    let next_parent = responses::mount_response_once_match(
        &server,
        |request: &wiremock::Request| {
            !is_guardian_request(request)
                && request_contains(request, NEXT_PROMPT)
                && !request_contains(request, NEXT_CALL_ID)
        },
        responses::sse_response(responses::sse(vec![
            responses::ev_response_created("next-parent"),
            responses::ev_function_call(NEXT_CALL_ID, TOOL_NAME, "{}"),
            responses::ev_completed("next-parent"),
        ])),
    )
    .await;
    let next_guardian = responses::mount_response_once_match(
        &server,
        |request: &wiremock::Request| {
            is_guardian_request(request) && request_contains(request, NEXT_CALL_ID)
        },
        responses::sse_response(responses::sse(vec![
            responses::ev_response_created("next-guardian"),
            responses::ev_assistant_message(
                "next-guardian-decision",
                &serde_json::json!({
                    "risk_level": "low",
                    "user_authorization": "high",
                    "outcome": "allow",
                    "rationale": "The follow-up egress is safe.",
                })
                .to_string(),
            ),
            responses::ev_completed("next-guardian"),
        ])),
    )
    .await;
    let next_completion = responses::mount_response_once_match(
        &server,
        |request: &wiremock::Request| {
            !is_guardian_request(request) && request_contains(request, NEXT_CALL_ID)
        },
        responses::sse_response(responses::sse(vec![
            responses::ev_assistant_message("next-complete", "done"),
            responses::ev_completed("next-complete"),
        ])),
    )
    .await;

    test.submit_turn(NEXT_PROMPT).await?;

    assert_eq!(next_parent.requests().len(), 1);
    assert_eq!(next_guardian.requests().len(), 1);
    assert_eq!(next_completion.requests().len(), 1);
    assert_eq!(handler_calls.load(Ordering::Relaxed), 1);
    Ok(())
}

fn valid_egress() -> ToolNetworkEgress {
    ToolNetworkEgress {
        protocol: NetworkApprovalProtocol::Https,
        host: "search.example".to_string(),
        port: 443,
        review_command: vec![TOOL_NAME.to_string(), "{}".to_string()],
    }
}

fn is_guardian_request(request: &wiremock::Request) -> bool {
    serde_json::from_slice::<serde_json::Value>(&request.body)
        .is_ok_and(|body| body["client_metadata"]["x-openai-subagent"].as_str() == Some("guardian"))
}

fn request_contains(request: &wiremock::Request, text: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(&request.body)
        .is_ok_and(|body| body.to_string().contains(text))
}

async fn wait_for_request(response: &responses::ResponseMock) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while response.requests().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("timed out waiting for Responses API request")
}
