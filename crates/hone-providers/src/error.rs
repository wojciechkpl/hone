use std::io;
use thiserror::Error;

/// Errors that can occur when interacting with a model provider.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP error from provider '{provider}': status {status}, body: {body}")]
    Http {
        provider: String,
        status: u16,
        body: String,
    },

    #[error("Stream error from provider '{provider}': {source}")]
    Stream {
        provider: String,
        source: io::Error,
    },

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
