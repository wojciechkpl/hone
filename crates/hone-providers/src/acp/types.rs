use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 envelope types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

/// A JSON-RPC 2.0 notification — no `id` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

// ---------------------------------------------------------------------------
// ACP-specific parameter / result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,
    #[serde(rename = "agentInfo")]
    pub agent_info: AgentInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionNewParams {
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionNewResult {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptParams {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub prompt: Vec<PromptContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResult {
    #[serde(rename = "stopReason")]
    pub stop_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUpdate {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub update: UpdatePayload,
}

/// The polymorphic payload inside a `session/update` notification.
///
/// Serde's externally-tagged enum cannot be used here because the discriminant
/// (`sessionUpdate`) lives inside the same flat object rather than being the
/// outer key.  We therefore use `untagged` and rely on the presence/absence of
/// fields to distinguish variants.  Because `tool_call` and `tool_call_update`
/// share some fields we need the fields that are unique to each variant to
/// drive the match — they are ordered so serde tries them in declaration order.
///
/// A cleaner alternative would be a manual `Deserialize` impl, but untagged
/// works correctly here because:
/// - `MessageChunk`  has `content`                   (unique)
/// - `ToolCall`      has `title`, `kind`              (unique)
/// - `ToolCallUpdate` has neither `content` nor `title`/`kind`
///
/// We also keep `session_update` as a plain string so we can round-trip
/// faithfully and can forward-compat with new variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate")]
pub enum UpdatePayload {
    #[serde(rename = "agent_message_chunk")]
    MessageChunk { content: MessageContent },

    #[serde(rename = "tool_call")]
    ToolCall {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        title: String,
        kind: String,
        status: String,
    },

    #[serde(rename = "tool_call_update")]
    ToolCallUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        status: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}
