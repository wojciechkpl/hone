use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_proto::{
    Content, Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, Role, TokenUsage,
    ToolCallRequest, ToolSchema,
};
use reqwest::Client;
use serde_json::{Value, json};
use tracing::debug;

use crate::error::{ProviderError, extract_api_error_message};
use crate::retry_after::parse_retry_after;
use crate::traits::{CompletionRequest, ModelProvider};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

pub struct OpenAiProvider {
    api_key: String,
    base_url: String,
    client: Client,
}

impl OpenAiProvider {
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

/// Converts a `Message` into the OpenAI API message format.
/// Unlike Anthropic, system messages stay in the messages array with role "system".
pub fn convert_message(message: &Message) -> Value {
    match message.role {
        Role::System => {
            json!({ "role": "system", "content": message.content.as_text() })
        }
        Role::User => {
            let content = content_to_openai(&message.content);
            json!({ "role": "user", "content": content })
        }
        Role::Assistant => {
            let mut obj = serde_json::Map::new();
            obj.insert("role".to_owned(), json!("assistant"));

            let text = message.content.as_text();
            if !text.is_empty() {
                obj.insert("content".to_owned(), json!(text));
            }

            if !message.tool_calls.is_empty() {
                let tool_calls: Vec<Value> = message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            }
                        })
                    })
                    .collect();
                obj.insert("tool_calls".to_owned(), json!(tool_calls));
            }

            Value::Object(obj)
        }
        Role::Tool => {
            let tool_call_id = message.tool_call_id.as_deref().unwrap_or("");
            json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": message.content.as_text(),
            })
        }
        // Forward-compat: unknown roles fall back to a plain user message so
        // the conversation remains well-formed for the OpenAI API.
        _ => {
            json!({ "role": "user", "content": message.content.as_text() })
        }
    }
}

fn content_to_openai(content: &Content) -> Value {
    match content {
        Content::Text(text) => Value::String(text.clone()),
        Content::Parts(parts) => {
            let openai_parts: Vec<Value> = parts
                .iter()
                .map(|p| match p {
                    hone_proto::ContentPart::Text { text } => {
                        json!({ "type": "text", "text": text })
                    }
                    hone_proto::ContentPart::Image { media_type, data } => {
                        let data_url = format!("data:{media_type};base64,{data}");
                        json!({
                            "type": "image_url",
                            "image_url": { "url": data_url }
                        })
                    }
                })
                .collect();
            Value::Array(openai_parts)
        }
    }
}

fn convert_tool(tool: &ToolSchema) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        }
    })
}

/// Builds the OpenAI API request body from a `CompletionRequest`.
pub fn build_request_body(request: &CompletionRequest) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(convert_message).collect();
    let tools: Vec<Value> = request.tools.iter().map(convert_tool).collect();
    let model_name = request.model.model_name();

    let mut body = json!({
        "model": model_name,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }

    if let Some(temp) = request.temperature {
        body["temperature"] = json!(temp);
    }

    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }

    body
}

/// State of an in-progress tool call, accumulated across streaming deltas.
#[derive(Debug, Default)]
pub struct ToolCallState {
    id: String,
    name: String,
    arguments: String,
}

/// Parses a single SSE `data:` line from the OpenAI streaming response.
///
/// `tool_states` accumulates tool call fragments by their index.
/// Tool calls are emitted as `ProviderEvent::ToolCall` when `finish_reason` is
/// `"tool_calls"`. `finish_reason: "stop"` emits `ProviderEvent::Done`.
/// Usage in the final chunk emits `ProviderEvent::Usage`.
pub fn parse_sse_chunk(data: &str, tool_states: &mut HashMap<usize, ToolCallState>) -> Vec<ProviderEvent> {
    if data == "[DONE]" {
        return vec![ProviderEvent::Done];
    }

    let Ok(value) = serde_json::from_str::<Value>(data) else {
        debug!("Failed to parse OpenAI SSE chunk as JSON: {data}");
        return Vec::new();
    };

    let mut events = Vec::new();

    // Extract usage if present (appears in the final chunk alongside choices)
    if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
        let input = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
        let output = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;
        if input > 0 || output > 0 {
            // OpenAI automatic prompt caching: cached_tokens reflects the number of
            // prompt tokens served from cache (kicks in for inputs >1024 tokens).
            let cached = usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            events.push(ProviderEvent::Usage(TokenUsage {
                input,
                output,
                cache_read: cached,
                cache_creation: 0,
            }));
        }
    }

    let Some(choices) = value["choices"].as_array() else {
        return events;
    };

    for choice in choices {
        let delta = &choice["delta"];
        let finish_reason = choice["finish_reason"].as_str();

        // Text content token
        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                events.push(ProviderEvent::Token { text: text.to_owned() });
            }
        }

        // Tool call fragments
        if let Some(tool_call_deltas) = delta["tool_calls"].as_array() {
            for tc_delta in tool_call_deltas {
                let index = tc_delta["index"].as_u64().unwrap_or(0) as usize;
                let state = tool_states.entry(index).or_default();

                // The first chunk for this index carries id and name
                if let Some(id) = tc_delta["id"].as_str() {
                    state.id = id.to_owned();
                }
                if let Some(name) = tc_delta["function"]["name"].as_str() {
                    state.name = name.to_owned();
                }
                if let Some(args) = tc_delta["function"]["arguments"].as_str() {
                    state.arguments.push_str(args);
                }
            }
        }

        // Emit accumulated tool calls when the finish reason signals completion
        match finish_reason {
            Some("tool_calls") => {
                let mut indices: Vec<usize> = tool_states.keys().cloned().collect();
                indices.sort_unstable();
                for idx in indices {
                    if let Some(state) = tool_states.remove(&idx) {
                        let arguments = if state.arguments.is_empty() {
                            Value::Object(serde_json::Map::new())
                        } else {
                            serde_json::from_str(&state.arguments).unwrap_or(Value::Null)
                        };
                        events.push(ProviderEvent::ToolCall(ToolCallRequest {
                            id: state.id,
                            name: state.name,
                            arguments,
                        }));
                    }
                }
                events.push(ProviderEvent::Done);
            }
            Some("stop") => {
                events.push(ProviderEvent::Done);
            }
            _ => {}
        }
    }

    events
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn display_name(&self) -> &str {
        "OpenAI"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("openai:gpt-4o"),
                display_name: "GPT-4o".to_owned(),
                context_length: 128_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("openai:gpt-4o-mini"),
                display_name: "GPT-4o mini".to_owned(),
                context_length: 128_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("openai:o3"),
                display_name: "o3".to_owned(),
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
        let url = format!("{}/v1/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http {
                provider: "openai".to_owned(),
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
                    provider: "openai".to_owned(),
                    retry_after_secs,
                });
            }

            let body = if status_code == 401 {
                "invalid or expired API key — check your OPENAI_API_KEY environment variable".to_owned()
            } else {
                extract_api_error_message(&body_text)
            };
            return Err(ProviderError::Http {
                provider: "openai".to_owned(),
                status: status_code,
                body,
            });
        }

        let byte_stream = response.bytes_stream();

        let event_stream = stream::unfold(
            (byte_stream, String::new(), HashMap::<usize, ToolCallState>::new()),
            |(mut byte_stream, mut buffer, mut tool_states)| async move {
                loop {
                    let mut emitted = Vec::new();

                    // Process complete lines (SSE lines are newline-delimited)
                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer[..pos].trim().to_owned();
                        buffer.drain(..pos + 1);

                        if let Some(data) = line.strip_prefix("data: ") {
                            let parsed = parse_sse_chunk(data, &mut tool_states);
                            emitted.extend(parsed);
                        }
                    }

                    if !emitted.is_empty() {
                        return Some((stream::iter(emitted), (byte_stream, buffer, tool_states)));
                    }

                    match byte_stream.next().await {
                        Some(Ok(chunk)) => match std::str::from_utf8(&chunk) {
                            Ok(text) => buffer.push_str(text),
                            Err(_) => {
                                debug!("Received non-UTF-8 chunk from OpenAI SSE stream");
                            }
                        },
                        Some(Err(_)) | None => {
                            // Process any remaining lines in the buffer
                            let remaining = buffer.clone();
                            buffer.clear();
                            let mut final_events = Vec::new();
                            for line in remaining.lines() {
                                let line = line.trim();
                                if let Some(data) = line.strip_prefix("data: ") {
                                    let parsed = parse_sse_chunk(data, &mut tool_states);
                                    final_events.extend(parsed);
                                }
                            }
                            if !final_events.is_empty() {
                                return Some((
                                    stream::iter(final_events),
                                    (byte_stream, buffer, tool_states),
                                ));
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
        let total_chars: usize = messages.iter().map(|m| m.content.as_text().len()).sum();
        ((total_chars / 4) as u32).max(1)
    }

    async fn health(&self) -> ProviderHealth {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let result = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": "gpt-4o-mini",
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
