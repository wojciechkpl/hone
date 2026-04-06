use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, Role, TokenUsage};
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
/// Ollama uses the same roles as OpenAI: system, user, assistant.
pub fn convert_message(message: &Message) -> Value {
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        // Ollama has no tool role; represent as user with context
        Role::Tool => "user",
    };
    json!({ "role": role, "content": message.content.as_text() })
}

/// Builds the Ollama `/api/chat` request body from a `CompletionRequest`.
/// Ollama does not support tools natively, so the tools field is always omitted.
pub fn build_request_body(request: &CompletionRequest) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(convert_message).collect();
    let model_name = request.model.model_name();

    json!({
        "model": model_name,
        "messages": messages,
        "stream": true,
    })
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
            events.push(ProviderEvent::Usage(TokenUsage {
                input: prompt_tokens,
                output: completion_tokens,
                cache_read: 0,
            }));
        }

        events.push(ProviderEvent::Done);
        return events;
    }

    // Non-final chunk: extract content token
    if let Some(content) = value["message"]["content"].as_str() {
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
                id: ModelId::new("ollama:llama3.2"),
                display_name: "Llama 3.2".to_owned(),
                context_length: 128_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:codellama"),
                display_name: "Code Llama".to_owned(),
                context_length: 16_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:mistral"),
                display_name: "Mistral".to_owned(),
                context_length: 32_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("ollama:deepseek-coder"),
                display_name: "DeepSeek Coder".to_owned(),
                context_length: 16_000,
                supports_tools: false,
                supports_streaming: true,
            },
        ]
    }

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
