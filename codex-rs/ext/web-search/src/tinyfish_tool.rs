use codex_extension_api::ExtensionTurnItem;
use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolPayload;
use codex_extension_items::web_search::WebSearchItem;
use codex_http_client::HttpClientFactory;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WebSearchBeginEvent;
use codex_utils_redacted_string::RedactedString;
#[cfg(any(test, feature = "test-support"))]
use url::Url;

use crate::schema::TinyFishCommands;
use crate::tinyfish_client::TinyFishSearchClient;
use crate::tinyfish_output::MAX_TINYFISH_OUTPUT_BYTES;
use crate::tinyfish_output::TinyFishOutput;
use crate::tinyfish_output::prepare_tinyfish_output;
use crate::tinyfish_request::prepare_tinyfish_requests;
use crate::tool::WebSearchTool;
use crate::tool::extension_turn_item;

#[derive(Clone)]
pub(crate) struct TinyFishRuntime {
    http_client_factory: HttpClientFactory,
    api_key: Option<RedactedString>,
    #[cfg(any(test, feature = "test-support"))]
    endpoint: Option<Url>,
}

impl TinyFishRuntime {
    #[cfg(not(test))]
    #[expect(
        dead_code,
        reason = "production construction is deliberately deferred to the activation slice"
    )]
    pub(crate) fn new(
        http_client_factory: HttpClientFactory,
        api_key: Option<RedactedString>,
    ) -> Self {
        Self {
            http_client_factory,
            api_key,
            #[cfg(any(test, feature = "test-support"))]
            endpoint: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn new_for_test(
        http_client_factory: HttpClientFactory,
        endpoint: Url,
        api_key: Option<RedactedString>,
    ) -> Self {
        Self {
            http_client_factory,
            api_key,
            endpoint: Some(endpoint),
        }
    }

    pub(crate) async fn handle(
        &self,
        tool: &WebSearchTool,
        call: ToolCall<'_>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let commands = parse_commands(&call)?;
        let requests = prepare_tinyfish_requests(&commands, &tool.settings)?;
        let api_key = self
            .api_key
            .as_ref()
            .filter(|api_key| !api_key.as_str().trim().is_empty())
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "TinyFish web search requires TINYFISH_API_KEY".to_string(),
                )
            })?;
        let client = self.client(api_key)?;

        call.turn_item_emitter
            .emit_started(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: String::new(),
                    action: None,
                    results: None,
                },
                EventMsg::WebSearchBegin(WebSearchBeginEvent {
                    call_id: call.call_id.clone(),
                }),
            ))
            .await;

        let mut responses = Vec::with_capacity(requests.len());
        for request in &requests {
            let mut response = client
                .search(request)
                .await
                .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
            response.query = request.query.clone();
            responses.push(response);
        }
        let output = prepare_tinyfish_output(
            &call.call_id,
            responses,
            commands.response_length,
            call.response_byte_budget(MAX_TINYFISH_OUTPUT_BYTES),
            api_key.as_str(),
        )?;

        call.turn_item_emitter
            .emit_completed(ExtensionTurnItem {
                item: output.extension_item(),
                legacy_events: vec![output.legacy_event()],
            })
            .await;

        Ok(Box::new(output))
    }

    fn client(&self, api_key: &RedactedString) -> Result<TinyFishSearchClient, FunctionCallError> {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(endpoint) = self.endpoint.clone() {
            return crate::tinyfish_client::test_support::client(
                self.http_client_factory.clone(),
                endpoint,
                api_key.clone(),
            )
            .map_err(|err| FunctionCallError::RespondToModel(err.to_string()));
        }

        TinyFishSearchClient::new(self.http_client_factory.clone(), api_key.clone())
            .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
    }
}

fn parse_commands(call: &ToolCall<'_>) -> Result<TinyFishCommands, FunctionCallError> {
    serde_json::from_str(call.function_arguments()?)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

impl ToolOutput for TinyFishOutput {
    fn log_output(&self) -> String {
        "[TinyFish web search output]".to_string()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn contains_external_context(&self) -> bool {
        true
    }

    fn to_response_item(&self, _call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        self.response_item()
    }
}
