use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Result;
use codex_core::config::Config;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ToolContributor;
use codex_login::CodexAuth;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_tools::ToolCall as ExtensionToolCall;
use codex_tools::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolNetworkEgress;
use codex_tools::ToolSpec;
use core_test_support::test_codex::test_codex;

pub(super) const TOOL_NAME: &str = "test_network_egress";

#[derive(Clone)]
struct FakeEgressExecutor {
    egress: ToolNetworkEgress,
    handler_calls: Arc<AtomicUsize>,
}

impl<'call> ToolExecutor<ExtensionToolCall<'call>> for FakeEgressExecutor {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(codex_tools::ResponsesApiTool {
            name: TOOL_NAME.to_string(),
            description: "Exercises host-reviewed extension egress.".to_string(),
            strict: true,
            parameters: codex_tools::parse_tool_input_schema(&serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false,
            }))
            .expect("fake egress tool schema should parse"),
            output_schema: None,
            defer_loading: None,
        })
    }

    fn network_egress(
        &self,
        _payload: &codex_tools::ToolPayload,
    ) -> Result<Option<ToolNetworkEgress>, codex_tools::FunctionCallError> {
        Ok(Some(self.egress.clone()))
    }

    fn handle<'a>(&'a self, _call: ExtensionToolCall<'call>) -> codex_tools::ToolExecutorFuture<'a>
    where
        'call: 'a,
    {
        Box::pin(async {
            self.handler_calls.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(codex_tools::JsonToolOutput::new(
                serde_json::json!({ "ok": true }),
            )) as Box<dyn codex_tools::ToolOutput>)
        })
    }
}

struct FakeEgressContributor(FakeEgressExecutor);

impl ToolContributor for FakeEgressContributor {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        _thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn for<'call> ToolExecutor<ExtensionToolCall<'call>>>> {
        vec![Arc::new(self.0.clone())]
    }
}

pub(super) async fn build_fake_egress_test(
    server: &wiremock::MockServer,
    egress: ToolNetworkEgress,
    handler_calls: Arc<AtomicUsize>,
) -> Result<core_test_support::test_codex::TestCodex> {
    let auth = CodexAuth::from_api_key("dummy");
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.tool_contributor(Arc::new(FakeEgressContributor(FakeEgressExecutor {
        egress,
        handler_calls,
    })));
    let mut builder = test_codex()
        .with_auth(auth)
        .with_extensions(Arc::new(extensions.build()))
        .with_config(|config| {
            config.approvals_reviewer = ApprovalsReviewer::AutoReview;
        });
    builder.build_with_auto_env(server).await
}
