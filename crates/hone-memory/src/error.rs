use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("memory not found: {id}")]
    NotFound { id: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
