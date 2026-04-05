use thiserror::Error;

/// Errors from the proto crate (primarily deserialization).
#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid model ID format: {0}")]
    InvalidModelId(String),
}
