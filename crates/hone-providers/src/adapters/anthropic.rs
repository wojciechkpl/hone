use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_proto::{Content, Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, Role, TokenUsage, ToolCallRequest, ToolSchema};
use reqwest::Client;
use serde_json::{Value, json};
use tracing::debug;

use crate::error::{ProviderError, extract_api_error_message};
use crate::retry_after::parse_retry_after;
use crate::traits::{CompletionRequest, ModelProvider};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: Client,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            client: Client::new(),
        }
    }

    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            client: Client::new(),
        }
    }
}

/// Converts a `Message` into the Anthropic API message format.
/// Returns `None` for system messages (they are handled separately).
pub fn convert_message(message: &Message) -> Option<Value> {
    match message.role {
        Role::System => None,
        Role::User => {
            let content = content_to_anthropic(&message.content);
            Some(json!({ "role": "user", "content": content }))
        }
        Role::Assistant => {
            let mut parts: Vec<Value> = Vec::new();

            // Add text content if present
            let text = message.content.as_text();
            if !text.is_empty() {
                parts.push(json!({ "type": "text", "text": text }));
            }

            // Add tool_use blocks for any tool calls
            for call in &message.tool_calls {
                parts.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }

            let content = if parts.len() == 1 && message.tool_calls.is_empty() {
                // Plain text assistant message
                Value::String(text)
            } else {
                Value::Array(parts)
            };

            Some(json!({ "role": "assistant", "content": content }))
        }
        Role::Tool => {
            // Tool result messages become user messages with tool_result content
            let tool_call_id = message.tool_call_id.as_deref().unwrap_or("");
            let content_text = message.content.as_text();
            Some(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content_text,
                }]
            }))
        }
        // Forward-compat: unknown roles are silently skipped (no mapping to
        // the Anthropic API exists yet).
        _ => None,
    }
}

fn content_to_anthropic(content: &Content) -> Value {
    match content {
        Content::Text(text) => Value::String(text.clone()),
        Content::Parts(parts) => {
            let anthropic_parts: Vec<Value> = parts
                .iter()
                .map(|p| match p {
                    hone_proto::ContentPart::Text { text } => {
                        json!({ "type": "text", "text": text })
                    }
                    hone_proto::ContentPart::Image { media_type, data } => {
                        json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": data,
                            }
                        })
                    }
                })
                .collect();
            Value::Array(anthropic_parts)
        }
    }
}

fn convert_tool(tool: &ToolSchema) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.parameters,
    })
}

fn convert_tool_cached(tool: &ToolSchema) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.parameters,
        "cache_control": { "type": "ephemeral" },
    })
}

/// Builds the Anthropic API request body from a `CompletionRequest`.
/// Returns `(body, system_message)` where `system_message` is extracted separately.
pub fn build_request_body(request: &CompletionRequest) -> Value {
    // Extract system message (Anthropic requires it as a top-level field)
    let system_text: Option<String> = request
        .messages
        .iter()
        .find(|m| m.role == Role::System)
        .map(|m| m.content.as_text());

    let messages: Vec<Value> = request
        .messages
        .iter()
        .filter_map(convert_message)
        .collect();

    // Apply cache_control to the last tool so Anthropic caches all tool schemas.
    let tools: Vec<Value> = match request.tools.len() {
        0 => Vec::new(),
        n => {
            let mut v: Vec<Value> = request.tools[..n - 1].iter().map(convert_tool).collect();
            v.push(convert_tool_cached(&request.tools[n - 1]));
            v
        }
    };

    let model_name = request.model.model_name();
    let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

    let mut body = json!({
        "model": model_name,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": true,
    });

    // Emit system as an array of text blocks with cache_control on the last block.
    // This enables Anthropic prompt caching for the stable system prefix.
    if let Some(system) = system_text {
        body["system"] = json!([{
            "type": "text",
            "text": system,
            "cache_control": { "type": "ephemeral" },
        }]);
    }

    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }

    if let Some(temp) = request.temperature {
        body["temperature"] = json!(temp);
    }

    body
}

/// State of an in-progress tool call block, accumulated across delta events.
#[derive(Debug)]
pub struct ToolCallState {
    id: String,
    name: String,
    partial_json: String,
}

/// Parses a single SSE `data:` line into zero or more `ProviderEvent`s.
///
/// `tool_states` is a mutable map from block index to in-progress tool call state.
/// Tool calls are only emitted as `ProviderEvent::ToolCall` when `content_block_stop`
/// is received, at which point the accumulated JSON is parsed.
pub fn parse_sse_data(
    data: &str,
    tool_states: &mut HashMap<usize, ToolCallState>,
) -> Vec<ProviderEvent> {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        debug!("Failed to parse SSE data as JSON: {data}");
        return Vec::new();
    };

    let event_type = value["type"].as_str().unwrap_or("");
    let mut events = Vec::new();

    match event_type {
        "content_block_start" => {
            let index = value["index"].as_u64().unwrap_or(0) as usize;
            let block = &value["content_block"];
            if block["type"].as_str() == Some("tool_use") {
                let id = block["id"].as_str().unwrap_or("").to_owned();
                let name = block["name"].as_str().unwrap_or("").to_owned();
                tool_states.insert(index, ToolCallState { id, name, partial_json: String::new() });
            }
        }
        "content_block_delta" => {
            let index = value["index"].as_u64().unwrap_or(0) as usize;
            let delta = &value["delta"];
            match delta["type"].as_str() {
                Some("text_delta") => {
                    let text = delta["text"].as_str().unwrap_or("").to_owned();
                    if !text.is_empty() {
                        events.push(ProviderEvent::Token { text });
                    }
                }
                Some("input_json_delta") => {
                    let partial = delta["partial_json"].as_str().unwrap_or("");
                    if let Some(state) = tool_states.get_mut(&index) {
                        state.partial_json.push_str(partial);
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            let index = value["index"].as_u64().unwrap_or(0) as usize;
            if let Some(state) = tool_states.remove(&index) {
                // Parse accumulated JSON; fall back to null if empty or invalid
                let arguments = if state.partial_json.is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(&state.partial_json).unwrap_or(Value::Null)
                };
                events.push(ProviderEvent::ToolCall(ToolCallRequest {
                    id: state.id,
                    name: state.name,
                    arguments,
                }));
            }
        }
        "message_delta" => {
            let usage = &value["usage"];
            let output_tokens = usage["output_tokens"].as_u64().unwrap_or(0) as u32;
            events.push(ProviderEvent::Usage(TokenUsage {
                input: 0,
                output: output_tokens,
                cache_read: 0,
            }));
        }
        "message_start" => {
            // Extract input token count from the initial message event
            let usage = &value["message"]["usage"];
            let input_tokens = usage["input_tokens"].as_u64().unwrap_or(0) as u32;
            let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0) as u32;
            if input_tokens > 0 || cache_read > 0 {
                events.push(ProviderEvent::Usage(TokenUsage {
                    input: input_tokens,
                    output: 0,
                    cache_read,
                }));
            }
        }
        "message_stop" => {
            events.push(ProviderEvent::Done);
        }
        _ => {}
    }

    events
}

/// Parses a complete SSE chunk (potentially multiple `event:`/`data:` lines)
/// into a list of (event_type, data) pairs.
pub fn extract_sse_events(chunk: &str) -> Vec<(&str, &str)> {
    let mut result = Vec::new();
    // SSE events are separated by blank lines; within each event we have lines
    // like "event: foo" and "data: {...}"
    for block in chunk.split("\n\n") {
        let mut event_type = "";
        let mut data = "";
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event: ") {
                event_type = rest.trim();
            } else if let Some(rest) = line.strip_prefix("data: ") {
                data = rest.trim();
            }
        }
        if !data.is_empty() {
            result.push((event_type, data));
        }
    }
    result
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn display_name(&self) -> &str {
        "Anthropic"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("anthropic:claude-opus-4-5"),
                display_name: "Claude Opus 4.5".to_owned(),
                context_length: 200_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("anthropic:claude-sonnet-4-5"),
                display_name: "Claude Sonnet 4.5".to_owned(),
                context_length: 200_000,
                supports_tools: true,
                supports_streaming: true,
            },
        ]
    }

    #[tracing::instrument(skip(self, request), fields(
        model = %request.model,
        message_count = request.messages.len(),
        tool_count = request.tools.len()
    ))]
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let body = build_request_body(&request);
        let url = format!("{}/v1/messages", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http {
                provider: "anthropic".to_owned(),
                status: 0,
                body: e.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            // Read headers BEFORE consuming the body — `response.text()` moves
            // the response and makes headers inaccessible afterward.
            let retry_after_secs = parse_retry_after(response.headers());
            let body_text = response.text().await.unwrap_or_default();

            if status_code == 429 {
                return Err(ProviderError::RateLimit {
                    provider: "anthropic".to_owned(),
                    retry_after_secs,
                });
            }

            let body = if status_code == 401 {
                "invalid or expired API key — check your ANTHROPIC_API_KEY environment variable".to_owned()
            } else {
                extract_api_error_message(&body_text)
            };
            return Err(ProviderError::Http {
                provider: "anthropic".to_owned(),
                status: status_code,
                body,
            });
        }

        // Stream the response body, parsing SSE events line-by-line
        let byte_stream = response.bytes_stream();

        let event_stream = stream::unfold(
            (byte_stream, String::new(), HashMap::<usize, ToolCallState>::new()),
            |(mut byte_stream, mut buffer, mut tool_states)| async move {
                loop {
                    // Try to process events already in the buffer
                    let mut emitted = Vec::new();

                    // Look for complete SSE blocks (terminated by \n\n)
                    while let Some(pos) = buffer.find("\n\n") {
                        let block = buffer[..pos + 2].to_owned();
                        buffer.drain(..pos + 2);

                        for (_event_type, data) in extract_sse_events(&block) {
                            let parsed = parse_sse_data(data, &mut tool_states);
                            emitted.extend(parsed);
                        }
                    }

                    if !emitted.is_empty() {
                        return Some((
                            stream::iter(emitted),
                            (byte_stream, buffer, tool_states),
                        ));
                    }

                    // Need more bytes
                    match byte_stream.next().await {
                        Some(Ok(chunk)) => {
                            match std::str::from_utf8(&chunk) {
                                Ok(text) => buffer.push_str(text),
                                Err(_) => {
                                    // Skip invalid UTF-8 chunks
                                    debug!("Received non-UTF-8 chunk from Anthropic SSE stream");
                                }
                            }
                        }
                        Some(Err(_)) | None => {
                            // Process any remaining buffer content
                            if !buffer.is_empty() {
                                let remaining = buffer.clone();
                                buffer.clear();
                                let mut final_events = Vec::new();
                                for (_event_type, data) in extract_sse_events(&remaining) {
                                    let parsed = parse_sse_data(data, &mut tool_states);
                                    final_events.extend(parsed);
                                }
                                if !final_events.is_empty() {
                                    return Some((
                                        stream::iter(final_events),
                                        (byte_stream, buffer, tool_states),
                                    ));
                                }
                            }
                            return None;
                        }
                    }
                }
            },
        )
        .flatten();

        Ok(Box::pin(event_stream))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        let total_chars: usize = messages
            .iter()
            .map(|m| m.content.as_text().len())
            .sum();
        ((total_chars / 4) as u32).max(1)
    }

    async fn health(&self) -> ProviderHealth {
        // Send a minimal request; 401 means the key is bad, 400 means we're healthy
        // (Anthropic returns 400 for invalid requests, not 401 for valid keys with bad bodies)
        let url = format!("{}/v1/messages", self.base_url);

        let result = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&json!({
                "model": "claude-sonnet-4-5-20250514",
                "max_tokens": 1,
                "messages": [{ "role": "user", "content": "ping" }]
            }))
            .send()
            .await;

        match result {
            Ok(response) => {
                let status = response.status().as_u16();
                match status {
                    200..=299 => ProviderHealth::Healthy,
                    401 => ProviderHealth::Unavailable {
                        reason: "Invalid API key".to_owned(),
                    },
                    429 => ProviderHealth::Degraded {
                        reason: "Rate limited".to_owned(),
                    },
                    _ => ProviderHealth::Degraded {
                        reason: format!("Unexpected status {status}"),
                    },
                }
            }
            Err(e) => ProviderHealth::Unavailable {
                reason: format!("Network error: {e}"),
            },
        }
    }
}
