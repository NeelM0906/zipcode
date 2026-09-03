use codex_analytics::GuardianApprovalRequestSource;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::protocol::ReviewDecision;
use codex_tools::FunctionCallError;
use codex_tools::ToolNetworkEgress;

use crate::guardian::GuardianApprovalRequest;
use crate::guardian::GuardianNetworkAccessTrigger;
use crate::guardian::GuardianReviewOptions;
use crate::guardian::guardian_timeout_message;
use crate::guardian::new_guardian_review_id;
use crate::guardian::review_approval_request_with_cancel;
use crate::sandboxing::SandboxPermissions;
use crate::tools::context::ToolInvocation;
use crate::tools::flat_tool_name;

const MAX_NETWORK_EGRESS_HOST_BYTES: usize = 253;
const MAX_NETWORK_EGRESS_REVIEW_ARGS: usize = 64;
const MAX_NETWORK_EGRESS_REVIEW_ARG_SERIALIZED_BYTES: usize = 8 * 1024;
const MAX_NETWORK_EGRESS_REVIEW_COMMAND_SERIALIZED_BYTES: usize = 8 * 1024;
const NETWORK_EGRESS_LIMIT_ERROR: &str = "Network egress declaration exceeds host safety limits.";

pub(crate) async fn review_tool_network_egress(
    invocation: &ToolInvocation,
    egress: ToolNetworkEgress,
) -> Result<(), FunctionCallError> {
    validate_tool_network_egress(&egress)?;
    let ToolNetworkEgress {
        protocol,
        host,
        port,
        review_command,
    } = egress;
    let target = network_target(protocol, &host, port);
    let cwd = invocation
        .step_context
        .environments
        .primary()
        .map(|environment| environment.cwd().clone())
        .unwrap_or_else(|| {
            codex_utils_path_uri::PathUri::from_abs_path(&invocation.turn.config.cwd)
        });
    let request = GuardianApprovalRequest::NetworkAccess {
        id: invocation.call_id.clone(),
        turn_id: invocation.turn.sub_id.clone(),
        target,
        host,
        protocol,
        port,
        trigger: Some(GuardianNetworkAccessTrigger {
            call_id: invocation.call_id.clone(),
            tool_name: flat_tool_name(&invocation.tool_name).into_owned(),
            command: review_command,
            cwd,
            sandbox_permissions: SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: None,
            tty: None,
        }),
    };
    let decision = review_approval_request_with_cancel(
        &invocation.session,
        &invocation.step_context,
        new_guardian_review_id(),
        request,
        /*retry_reason*/ None,
        GuardianReviewOptions {
            plugin_attribution_override: None,
            approval_request_source: GuardianApprovalRequestSource::MainTurn,
            external_cancel: Some(invocation.cancellation_token.clone()),
            require_synchronous_review: true,
        },
    )
    .await;

    match decision {
        ReviewDecision::Approved => Ok(()),
        ReviewDecision::Denied { rejection } => Err(FunctionCallError::RespondToModel(rejection)),
        ReviewDecision::TimedOut => Err(FunctionCallError::RespondToModel(
            guardian_timeout_message(invocation.turn.model_info()),
        )),
        ReviewDecision::Abort => Err(FunctionCallError::RespondToModel(
            "Network egress review was cancelled before approval.".to_string(),
        )),
        ReviewDecision::ApprovedExecpolicyAmendment { .. }
        | ReviewDecision::ApprovedForSession
        | ReviewDecision::ApprovedMcpPolicyAmendment
        | ReviewDecision::NetworkPolicyAmendment { .. } => Err(FunctionCallError::RespondToModel(
            "Network egress review did not return an explicit one-time approval.".to_string(),
        )),
    }
}

fn validate_tool_network_egress(egress: &ToolNetworkEgress) -> Result<(), FunctionCallError> {
    if egress.host.len() > MAX_NETWORK_EGRESS_HOST_BYTES
        || egress.review_command.len() > MAX_NETWORK_EGRESS_REVIEW_ARGS
    {
        return Err(network_egress_limit_error());
    }

    // Count the exact JSON representation Guardian receives without ever allocating an
    // unbounded duplicate of extension-owned input.
    let mut command_serialized_bytes = 2_usize;
    for (index, argument) in egress.review_command.iter().enumerate() {
        if argument.len() > MAX_NETWORK_EGRESS_REVIEW_ARG_SERIALIZED_BYTES {
            return Err(network_egress_limit_error());
        }
        let argument_serialized_bytes = serde_json::to_string(argument)
            .map_err(|_| network_egress_limit_error())?
            .len();
        if argument_serialized_bytes > MAX_NETWORK_EGRESS_REVIEW_ARG_SERIALIZED_BYTES {
            return Err(network_egress_limit_error());
        }
        command_serialized_bytes = command_serialized_bytes
            .checked_add(usize::from(index > 0))
            .and_then(|size| size.checked_add(argument_serialized_bytes))
            .filter(|size| *size <= MAX_NETWORK_EGRESS_REVIEW_COMMAND_SERIALIZED_BYTES)
            .ok_or_else(network_egress_limit_error)?;
    }
    Ok(())
}

fn network_egress_limit_error() -> FunctionCallError {
    FunctionCallError::RespondToModel(NETWORK_EGRESS_LIMIT_ERROR.to_string())
}

fn network_target(protocol: NetworkApprovalProtocol, host: &str, port: u16) -> String {
    let protocol = match protocol {
        NetworkApprovalProtocol::Http => "http",
        NetworkApprovalProtocol::Https => "https",
        NetworkApprovalProtocol::Socks5Tcp => "socks5-tcp",
        NetworkApprovalProtocol::Socks5Udp => "socks5-udp",
    };
    format!("{protocol}://{host}:{port}")
}

#[cfg(test)]
#[path = "egress_review_limits_tests.rs"]
mod limits_tests;
