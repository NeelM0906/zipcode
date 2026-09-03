use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Context;
use anyhow::Result;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_tools::ToolNetworkEgress;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;

use super::extension_network_egress::TOOL_NAME;
use super::extension_network_egress::build_fake_egress_test;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_extension_egress_is_rejected_before_guardian_or_handler() -> Result<()> {
    skip_if_no_network!(Ok(()));

    const CALL_ID: &str = "oversized-extension-egress";
    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("parent-tool-call"),
                responses::ev_function_call(CALL_ID, TOOL_NAME, "{}"),
                responses::ev_completed("parent-tool-call"),
            ]),
            responses::sse(vec![
                responses::ev_assistant_message("parent-complete", "done"),
                responses::ev_completed("parent-complete"),
            ]),
        ],
    )
    .await;
    let handler_calls = Arc::new(AtomicUsize::new(0));
    let test = build_fake_egress_test(
        &server,
        ToolNetworkEgress {
            protocol: NetworkApprovalProtocol::Https,
            host: "h".repeat(254),
            port: 443,
            review_command: vec![TOOL_NAME.to_string()],
        },
        Arc::clone(&handler_calls),
    )
    .await?;

    test.submit_turn("invoke the fake egress tool").await?;

    let requests = response_mock.requests();
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests
            .iter()
            .filter(|request| {
                request.body_json()["client_metadata"]["x-openai-subagent"].as_str()
                    == Some("guardian")
            })
            .count(),
        0
    );
    let output = requests[1]
        .function_call_output_text(CALL_ID)
        .context("rejected extension call should return model-visible output")?;
    assert_eq!(
        output,
        "Network egress declaration exceeds host safety limits."
    );

    Ok(())
}
