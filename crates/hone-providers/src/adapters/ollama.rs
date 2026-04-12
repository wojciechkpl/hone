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

use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

pub struct OllamaProvider {
    base_url: String,
    client: Client,
}

impl OllamaProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: Client::new(),
        }
    }
}

/// Converts a `Message` into the Ollama API message format.
///
/// Ollama supports the same roles and tool calling format as OpenAI since
/// v0.3.0 (July 2024): system, user, assistant, tool.
pub fn convert_message(message: &Message) -> Value {
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };

    let mut obj = serde_json::Map::new();
    obj.insert("role".to_owned(), json!(role));
    obj.insert("content".to_owned(), json!(message.content.as_text()));

    // Include tool_call_id for tool-result messages.
    if let Some(ref tc_id) = message.tool_call_id {
        obj.insert("tool_call_id".to_owned(), json!(tc_id));
    }

    // Include tool_calls for assistant messages that requested tool use.
    if !message.tool_calls.is_empty() {
        let tool_calls: Vec<Value> = message
            .tool_calls
            .iter()
            .map(|tc| {
                json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments.to_string(),
                    }
                })
            })
            .collect();
        obj.insert("tool_calls".to_owned(), json!(tool_calls));
    }

    Value::Object(obj)
}

/// Converts a `ToolSchema` into the Ollama tool format (same as OpenAI).
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

/// Builds the Ollama `/api/chat` request body from a `CompletionRequest`.
///
/// Ollama supports tool calling since v0.3.0 (July 2024) using the same
/// format as OpenAI: tools array with function definitions.
pub fn build_request_body(request: &CompletionRequest) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(convert_message).collect();
    let tools: Vec<Value> = request.tools.iter().map(convert_tool).collect();
    let model_name = request.model.model_name();

    let mut body = json!({
        "model": model_name,
        "messages": messages,
        "stream": true,
    });

    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }

    if let Some(temp) = request.temperature {
        body["temperature"] = json!(temp);
    }

    body
}

/// Returns the health check URL for an Ollama instance.
pub fn health_url(base_url: &str) -> String {
    format!("{base_url}/api/tags")
}

/// Parses a single newline-delimited JSON line from the Ollama streaming response.
///
/// Returns zero or more `ProviderEvent`s. For non-final chunks, emits a `Token`
/// event if the message content is non-empty. For the final chunk (`done: true`),
/// emits `Usage` (if eval counts are present) followed by `Done`.
pub fn parse_ndjson_line(line: &str) -> Vec<ProviderEvent> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        debug!("Failed to parse Ollama NDJSON line: {line}");
        return Vec::new();
    };

    let done = value["done"].as_bool().unwrap_or(false);

    if done {
        let mut events = Vec::new();

        let prompt_tokens = value["prompt_eval_count"].as_u64().unwrap_or(0) as u32;
        let completion_tokens = value["eval_count"].as_u64().unwrap_or(0) as u32;

        if prompt_tokens > 0 || completion_tokens > 0 {
            // Ollama's local KV cache is not exposed via the wire protocol.
            // `prompt_eval_count` reflects tokens actually evaluated (cache hits
            // reduce this), but Ollama does not report cache_read/cache_write
            // counts separately, so we leave cache_read as 0.
            events.push(ProviderEvent::Usage(TokenUsage {
                input: prompt_tokens,
                output: completion_tokens,
                cache_read: 0,
            }));
        }

        events.push(ProviderEvent::Done);
        return events;
    }

    // Non-final chunk: check for tool calls first, then content.
    let message = &value["message"];

    // Tool call response — Ollama returns tool_calls in the message.
    if let Some(tool_calls) = message["tool_calls"].as_array() {
        let mut events = Vec::new();
        for tc in tool_calls {
            let name = tc["function"]["name"].as_str().unwrap_or("").to_owned();
            let id = tc["id"]
                .as_str()
                .unwrap_or(&format!("call_{}", name))
                .to_owned();

            // Parse arguments: Ollama may return them as a string or object.
            let arguments = match &tc["function"]["arguments"] {
                Value::String(s) => {
                    serde_json::from_str(s).unwrap_or_else(|_| json!({"raw": s}))
                }
                Value::Object(_) => tc["function"]["arguments"].clone(),
                other => json!({"raw": other.to_string()}),
            };

            if !name.is_empty() {
                events.push(ProviderEvent::ToolCall(ToolCallRequest {
                    id,
                    name,
                    arguments,
                }));
            }
        }
        if !events.is_empty() {
            return events;
        }
    }

    // Content token.
    if let Some(content) = message["content"].as_str() {
        if !content.is_empty() {
            return vec![ProviderEvent::Token { text: content.to_owned() }];
        }
    }

    Vec::new()
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    fn display_name(&self) -> &str {
        "Ollama"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("ollama:qwen2.5-coder:7b"),
                display_name: "Qwen 2.5 Coder 7B".to_owned(),
                context_length: 128_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:qwen2.5-coder:14b"),
                display_name: "Qwen 2.5 Coder 14B".to_owned(),
                context_length: 128_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:llama3.2"),
                display_name: "Llama 3.2".to_owned(),
                context_length: 128_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:llama3.3"),
                display_name: "Llama 3.3 70B".to_owned(),
                context_length: 128_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:mistral"),
                display_name: "Mistral 7B".to_owned(),
                context_length: 32_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:deepseek-coder-v2"),
                display_name: "DeepSeek Coder V2".to_owned(),
                context_length: 128_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:codellama"),
                display_name: "Code Llama".to_owned(),
                context_length: 16_000,
                supports_tools: false,
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
        let url = format!("{}/api/chat", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http {
                provider: "ollama".to_owned(),
                status: 0,
                body: e.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Http {
                provider: "ollama".to_owned(),
                status: status_code,
                body: body_text,
            });
        }

        let byte_stream = response.bytes_stream();

        // Ollama streams newline-delimited JSON, not SSE
        let event_stream = stream::unfold(
            (byte_stream, String::new()),
            |(mut byte_stream, mut buffer)| async move {
                loop {
                    let mut emitted = Vec::new();

                    // Process all complete lines in the buffer
                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer[..pos].trim().to_owned();
                        buffer.drain(..pos + 1);

                        if !line.is_empty() {
                            let parsed = parse_ndjson_line(&line);
                            emitted.extend(parsed);
                        }
                    }

                    if !emitted.is_empty() {
                        return Some((stream::iter(emitted), (byte_stream, buffer)));
                    }

                    match byte_stream.next().await {
                        Some(Ok(chunk)) => match std::str::from_utf8(&chunk) {
                            Ok(text) => buffer.push_str(text),
                            Err(_) => {
                                debug!("Received non-UTF-8 chunk from Ollama stream");
                            }
                        },
                        Some(Err(_)) | None => {
                            // Process any remaining content in the buffer
                            let remaining = buffer.clone();
                            buffer.clear();
                            let mut final_events = Vec::new();
                            for line in remaining.lines() {
                                let line = line.trim();
                                if !line.is_empty() {
                                    let parsed = parse_ndjson_line(line);
                                    final_events.extend(parsed);
                                }
                            }
                            if !final_events.is_empty() {
                                return Some((stream::iter(final_events), (byte_stream, buffer)));
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
        let url = health_url(&self.base_url);

        match self.client.get(&url).send().await {
            Ok(response) if response.status().is_success() => ProviderHealth::Healthy,
            Ok(response) => ProviderHealth::Degraded {
                reason: format!("Unexpected status {}", response.status().as_u16()),
            },
            Err(e) => ProviderHealth::Unavailable {
                reason: format!("Connection refused or network error: {e}"),
            },
        }
    }
}
