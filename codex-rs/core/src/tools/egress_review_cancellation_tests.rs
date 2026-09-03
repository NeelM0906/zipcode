use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use codex_features::Feature;
use codex_model_provider::create_model_provider;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::GuardianAssessmentStatus;
use codex_tools::FunctionCallError;
use codex_tools::ToolName;
use codex_tools::ToolNetworkEgress;
use codex_tools::ToolSpec;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use crate::session::step_context::StepContext;
use crate::test_support::models_manager_with_provider;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::tools::registry::ToolRegistry;
use crate::turn_diff_tracker::TurnDiffTracker;

struct ReviewableHandler {
    handler_calls: Arc<AtomicUsize>,
}

impl ToolExecutor<ToolInvocation> for ReviewableHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced("test", "egress")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(codex_tools::ResponsesApiTool {
            name: "egress".to_string(),
            description: "Tests reviewed egress.".to_string(),
            strict: true,
            parameters: codex_tools::JsonSchema::default(),
            output_schema: None,
            defer_loading: None,
        })
    }

    fn network_egress(
        &self,
        _payload: &ToolPayload,
    ) -> Result<Option<ToolNetworkEgress>, FunctionCallError> {
        Ok(Some(valid_egress()))
    }

    fn handle<'a>(&'a self, _invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async {
            self.handler_calls.fetch_add(1, Ordering::Relaxed);
            Ok(
                Box::new(crate::tools::context::FunctionToolOutput::from_text(
                    "ok".to_string(),
                    /*success*/ Some(true),
                )) as Box<dyn crate::tools::context::ToolOutput>,
            )
        })
    }
}

impl CoreToolRuntime for ReviewableHandler {}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_pending_review_finishes_abort_before_the_next_review() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    const CANCEL_CALL_ID: &str = "cancel-pending-review";
    const NEXT_CALL_ID: &str = "next-review";
    let server = responses::start_mock_server().await;
    let pending_review = responses::mount_response_once_match(
        &server,
        |request: &wiremock::Request| request_contains(request, CANCEL_CALL_ID),
        responses::sse_response(responses::sse(vec![responses::ev_completed(
            "cancelled-review",
        )]))
        .set_delay(Duration::from_secs(30)),
    )
    .await;
    let (mut session, mut turn, events) =
        crate::session::tests::make_session_and_context_with_rx().await;
    configure_guardian_server(&mut session, &mut turn, &server);
    let handler_calls = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(ToolRegistry::from_tools([Arc::new(ReviewableHandler {
        handler_calls: Arc::clone(&handler_calls),
    })
        as Arc<dyn CoreToolRuntime>]));
    let cancellation_token = CancellationToken::new();
    let dispatch = tokio::spawn({
        let registry = Arc::clone(&registry);
        let invocation = test_invocation(
            Arc::clone(&session),
            Arc::clone(&turn),
            CANCEL_CALL_ID,
            cancellation_token.clone(),
        );
        async move {
            registry
                .dispatch_any_with_terminal_outcome(
                    invocation, /*terminal_outcome_reached*/ None,
                )
                .await
        }
    });

    tokio::time::timeout(Duration::from_secs(10), async {
        while pending_review.requests().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("Guardian review did not reach its pending request")?;
    dispatch.abort();
    let Err(join_error) = dispatch.await else {
        panic!("dispatch should be cancelled");
    };
    assert!(join_error.is_cancelled());
    cancellation_token.cancel();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await?;
            if matches!(
                event.msg,
                EventMsg::GuardianAssessment(assessment)
                    if assessment.status == GuardianAssessmentStatus::Aborted
            ) {
                break;
            }
        }
        anyhow::Ok(())
    })
    .await
    .context("detached Guardian review did not emit terminal abort")??;
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);

    let next_review = responses::mount_response_once_match(
        &server,
        |request: &wiremock::Request| request_contains(request, NEXT_CALL_ID),
        responses::sse_response(responses::sse(vec![
            responses::ev_response_created("next-review"),
            responses::ev_assistant_message(
                "next-review-decision",
                &serde_json::json!({
                    "risk_level": "low",
                    "user_authorization": "high",
                    "outcome": "allow",
                    "rationale": "The follow-up review is safe.",
                })
                .to_string(),
            ),
            responses::ev_completed("next-review"),
        ])),
    )
    .await;
    registry
        .dispatch_any_with_terminal_outcome(
            test_invocation(session, turn, NEXT_CALL_ID, CancellationToken::new()),
            /*terminal_outcome_reached*/ None,
        )
        .await?;

    assert_eq!(next_review.requests().len(), 1);
    assert_eq!(handler_calls.load(Ordering::Relaxed), 1);
    Ok(())
}

fn configure_guardian_server(
    session: &mut Arc<crate::session::session::Session>,
    turn: &mut Arc<crate::session::turn_context::TurnContext>,
    server: &wiremock::MockServer,
) {
    let turn = Arc::get_mut(turn).expect("turn should have one owner during setup");
    let mut config = (*turn.config).clone();
    config
        .features
        .enable(Feature::GuardianApproval)
        .expect("Guardian approval feature should be configurable");
    config.approvals_reviewer = ApprovalsReviewer::AutoReview;
    config.model_provider.base_url = Some(format!("{}/v1", server.uri()));
    let config = Arc::new(config);
    let auth_manager = Arc::clone(&session.services.auth_manager);
    Arc::get_mut(session)
        .expect("session should have one owner during setup")
        .services
        .models_manager = models_manager_with_provider(
        config.codex_home.to_path_buf(),
        auth_manager,
        config.model_provider.clone(),
    );
    turn.config = Arc::clone(&config);
    turn.provider = create_model_provider(config.model_provider.clone(), turn.auth_manager.clone());
}

fn test_invocation(
    session: Arc<crate::session::session::Session>,
    turn: Arc<crate::session::turn_context::TurnContext>,
    call_id: &str,
    cancellation_token: CancellationToken,
) -> ToolInvocation {
    ToolInvocation {
        session,
        step_context: StepContext::for_test(Arc::clone(&turn)),
        turn,
        cancellation_token,
        tracker: Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
        call_id: call_id.to_string(),
        tool_name: ToolName::namespaced("test", "egress"),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    }
}

fn valid_egress() -> ToolNetworkEgress {
    ToolNetworkEgress {
        protocol: NetworkApprovalProtocol::Https,
        host: "search.example".to_string(),
        port: 443,
        review_command: vec!["test.egress".to_string(), "{}".to_string()],
    }
}

fn request_contains(request: &wiremock::Request, needle: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(&request.body)
        .is_ok_and(|body| body.to_string().contains(needle))
}
