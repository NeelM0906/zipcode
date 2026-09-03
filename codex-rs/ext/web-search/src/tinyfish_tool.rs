use codex_extension_api::ExtensionTurnItem;
use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolNetworkEgress;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolPayload;
use codex_extension_items::web_search::WebSearchItem;
use codex_http_client::HttpClientFactory;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WebSearchBeginEvent;
use codex_utils_redacted_string::RedactedString;
use url::Url;

use crate::schema::TinyFishCommands;
use crate::tinyfish_client::TINYFISH_SEARCH_ENDPOINT;
use crate::tinyfish_client::TinyFishSearchClient;
use crate::tinyfish_output::MAX_TINYFISH_OUTPUT_BYTES;
use crate::tinyfish_output::TinyFishOutput;
use crate::tinyfish_output::prepare_tinyfish_output;
use crate::tinyfish_request::prepare_tinyfish_egress;
use crate::tinyfish_request::prepare_tinyfish_requests;
use crate::tinyfish_request::reject_review_text;
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

    pub(crate) async fn handle(
        &self,
        tool: &WebSearchTool,
        call: ToolCall<'_>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let api_key = self.api_key()?;
        let prepared = prepare_call(&call.payload, &tool.settings, api_key)?;
        let commands = prepared.commands;
        let requests = prepared.requests;
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

    pub(crate) fn network_egress(
        &self,
        payload: &ToolPayload,
        settings: &codex_api::SearchSettings,
    ) -> Result<ToolNetworkEgress, FunctionCallError> {
        let api_key = self.api_key()?;
        let prepared = prepare_call(payload, settings, api_key)?;
        let endpoint = self.endpoint()?;
        let protocol = match endpoint.scheme() {
            "http" => NetworkApprovalProtocol::Http,
            "https" => NetworkApprovalProtocol::Https,
            scheme => {
                return Err(FunctionCallError::Fatal(format!(
                    "TinyFish web search endpoint uses unsupported scheme {scheme}"
                )));
            }
        };
        let host = endpoint.host_str().ok_or_else(|| {
            FunctionCallError::Fatal(
                "TinyFish web search endpoint does not contain a host".to_string(),
            )
        })?;
        let port = endpoint.port_or_known_default().ok_or_else(|| {
            FunctionCallError::Fatal(
                "TinyFish web search endpoint does not contain a known port".to_string(),
            )
        })?;

        Ok(ToolNetworkEgress {
            protocol,
            host: host.to_string(),
            port,
            review_command: prepared.review_command,
        })
    }

    fn endpoint(&self) -> Result<Url, FunctionCallError> {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(endpoint) = self.endpoint.as_ref() {
            return Ok(endpoint.clone());
        }

        Url::parse(TINYFISH_SEARCH_ENDPOINT).map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to configure the fixed TinyFish web search endpoint: {err}"
            ))
        })
    }

    fn api_key(&self) -> Result<&RedactedString, FunctionCallError> {
        self.api_key
            .as_ref()
            .filter(|api_key| !api_key.as_str().trim().is_empty())
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "TinyFish web search requires TINYFISH_API_KEY".to_string(),
                )
            })
    }

    fn client(&self, api_key: &RedactedString) -> Result<TinyFishSearchClient, FunctionCallError> {
        TinyFishSearchClient::from_endpoint(
            self.http_client_factory.clone(),
            self.endpoint()?,
            api_key.clone(),
        )
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod test_support {
    use super::HttpClientFactory;
    use super::RedactedString;
    use super::TinyFishRuntime;
    use super::Url;

    pub(crate) fn runtime(
        http_client_factory: HttpClientFactory,
        endpoint: Url,
        api_key: Option<RedactedString>,
    ) -> TinyFishRuntime {
        TinyFishRuntime {
            http_client_factory,
            api_key,
            endpoint: Some(endpoint),
        }
    }
}

struct PreparedTinyFishCall {
    commands: TinyFishCommands,
    requests: Vec<crate::tinyfish_request::TinyFishWireRequest>,
    review_command: Vec<String>,
}

fn prepare_call(
    payload: &ToolPayload,
    settings: &codex_api::SearchSettings,
    api_key: &RedactedString,
) -> Result<PreparedTinyFishCall, FunctionCallError> {
    let ToolPayload::Function { arguments } = payload else {
        return Err(FunctionCallError::Fatal(
            "TinyFish web search received an incompatible tool payload".to_string(),
        ));
    };
    reject_review_text(arguments, api_key)?;
    let commands: TinyFishCommands = serde_json::from_str(arguments)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
    let source_requests = prepare_tinyfish_requests(&commands, settings, api_key)?;
    let prepared_egress = prepare_tinyfish_egress(&source_requests, api_key)?;
    Ok(PreparedTinyFishCall {
        commands,
        requests: prepared_egress.requests,
        review_command: prepared_egress.review_command,
    })
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
