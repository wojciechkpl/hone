/// Errors produced by the TUI layer.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Render error: {0}")]
    Render(String),
}
