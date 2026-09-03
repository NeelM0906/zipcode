use codex_protocol::approvals::NetworkApprovalProtocol;

/// Network destination and request details that a tool asks the host to review before egress.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolNetworkEgress {
    /// Protocol used by the outbound connection.
    pub protocol: NetworkApprovalProtocol,
    /// Destination hostname without a scheme or port.
    pub host: String,
    /// Destination port.
    pub port: u16,
    /// Credential-free representation of every outbound model-controlled field.
    pub review_command: Vec<String>,
}
