use std::io;
use thiserror::Error;

/// Extract a human-readable message from a JSON API error body.
///
/// Handles both Anthropic (`{"error":{"message":"..."}}`) and
/// OpenAI (`{"error":{"message":"..."}}`) response shapes. Falls back to the
/// raw body (truncated to 200 chars) when neither matches.
pub(crate) fn extract_api_error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        // Anthropic: {"type":"error","error":{"message":"..."}}
        // OpenAI:    {"error":{"message":"..."}}
        if let Some(msg) = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
        // Ollama: {"error":"some message"}
        if let Some(msg) = v.get("error").and_then(|e| e.as_str()) {
            return msg.to_string();
        }
    }
    // Fall back: truncate raw body to avoid walls of JSON in the terminal.
    if body.len() > 200 {
        format!("{}…", &body[..200])
    } else {
        body.to_string()
    }
}

/// Errors that can occur when interacting with a model provider.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("{provider} error ({status}): {body}")]
    Http {
        provider: String,
        status: u16,
        body: String,
    },

    #[error("Stream error from provider '{provider}': {source}")]
    Stream { provider: String, source: io::Error },

    #[error("Rate limited by provider '{provider}', retry after {retry_after_secs}s")]
    RateLimit {
        provider: String,
        retry_after_secs: u64,
    },

    #[error("Model '{model}' not found at provider '{provider}'")]
    ModelNotFound { model: String, provider: String },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Timeout waiting for provider '{provider}'")]
    Timeout { provider: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_anthropic_error_message() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens must not exceed 8096"}}"#;
        assert_eq!(
            extract_api_error_message(body),
            "max_tokens must not exceed 8096"
        );
    }

    #[test]
    fn extracts_openai_error_message() {
        let body = r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#;
        assert_eq!(
            extract_api_error_message(body),
            "You exceeded your current quota"
        );
    }

    #[test]
    fn extracts_ollama_flat_error() {
        let body = r#"{"error":"model 'llama99' not found, try ollama pull llama99"}"#;
        assert_eq!(
            extract_api_error_message(body),
            "model 'llama99' not found, try ollama pull llama99"
        );
    }

    #[test]
    fn falls_back_to_raw_body_for_non_json() {
        let body = "Bad Request";
        assert_eq!(extract_api_error_message(body), "Bad Request");
    }

    #[test]
    fn truncates_long_body() {
        let body = "x".repeat(300);
        let result = extract_api_error_message(&body);
        assert!(result.len() < 210, "should be truncated");
        assert!(result.ends_with('…'));
    }

    #[test]
    fn http_error_format_is_concise() {
        let err = ProviderError::Http {
            provider: "anthropic".to_owned(),
            status: 400,
            body: "bad request".to_owned(),
        };
        assert_eq!(err.to_string(), "anthropic error (400): bad request");
    }
}
