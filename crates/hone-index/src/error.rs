use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Directory walk error: {0}")]
    WalkDir(#[from] walkdir::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Path not found: {path}")]
    NotFound { path: PathBuf },
}
