use std::sync::Arc;

use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::protocol::EventMsg;
use codex_tools::FunctionCallError;
use codex_tools::ToolName;
use codex_tools::ToolNetworkEgress;
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use super::review_tool_network_egress;
use super::validate_tool_network_egress;
use crate::session::step_context::StepContext;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::turn_diff_tracker::TurnDiffTracker;

#[test]
fn serialized_command_limit_is_inclusive_for_plain_and_escaped_arguments() {
    let plain_at_limit = egress_with_argument("x".repeat(8 * 1024 - 4));
    let plain_over_limit = egress_with_argument("x".repeat(8 * 1024 - 3));
    let escaped_at_limit = egress_with_argument("\"".repeat(4 * 1024 - 2));
    let escaped_over_limit = egress_with_argument(format!("{}x", "\"".repeat(4 * 1024 - 2)));

    assert_eq!(validate_tool_network_egress(&plain_at_limit), Ok(()));
    assert_eq!(validate_tool_network_egress(&escaped_at_limit), Ok(()));
    for egress in [plain_over_limit, escaped_over_limit] {
        assert_eq!(
            validate_tool_network_egress(&egress),
            Err(FunctionCallError::RespondToModel(
                "Network egress declaration exceeds host safety limits.".to_string()
            ))
        );
    }
}

#[tokio::test]
async fn rejects_oversized_declarations_before_starting_guardian() {
    let invalid_egress = [
        ToolNetworkEgress {
            protocol: NetworkApprovalProtocol::Https,
            host: "h".repeat(254),
            port: 443,
            review_command: vec!["web.run".to_string()],
        },
        ToolNetworkEgress {
            protocol: NetworkApprovalProtocol::Https,
            host: "search.example".to_string(),
            port: 443,
            review_command: vec!["argument".to_string(); 65],
        },
        ToolNetworkEgress {
            protocol: NetworkApprovalProtocol::Https,
            host: "search.example".to_string(),
            port: 443,
            review_command: vec!["x".repeat(8 * 1024 + 1)],
        },
        ToolNetworkEgress {
            protocol: NetworkApprovalProtocol::Https,
            host: "search.example".to_string(),
            port: 443,
            review_command: vec!["\"".repeat(4 * 1024)],
        },
        ToolNetworkEgress {
            protocol: NetworkApprovalProtocol::Https,
            host: "search.example".to_string(),
            port: 443,
            review_command: vec!["x".repeat(4 * 1024 - 2); 2],
        },
    ];
    let (session, turn, events) = crate::session::tests::make_session_and_context_with_rx().await;
    let cancellation_token = CancellationToken::new();
    cancellation_token.cancel();
    let invocation = ToolInvocation {
        session,
        step_context: StepContext::for_test(Arc::clone(&turn)),
        turn,
        cancellation_token,
        tracker: Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
        call_id: "oversized-egress".to_string(),
        tool_name: ToolName::namespaced("test", "egress"),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    };

    for egress in invalid_egress {
        let error = review_tool_network_egress(&invocation, egress)
            .await
            .expect_err("oversized network egress declaration should be rejected");
        assert_eq!(
            error,
            FunctionCallError::RespondToModel(
                "Network egress declaration exceeds host safety limits.".to_string()
            )
        );
    }

    while let Ok(event) = events.try_recv() {
        assert!(
            !matches!(event.msg, EventMsg::GuardianAssessment(_)),
            "invalid declarations must not start Guardian"
        );
    }
}

fn egress_with_argument(argument: String) -> ToolNetworkEgress {
    ToolNetworkEgress {
        protocol: NetworkApprovalProtocol::Https,
        host: "search.example".to_string(),
        port: 443,
        review_command: vec![argument],
    }
}
