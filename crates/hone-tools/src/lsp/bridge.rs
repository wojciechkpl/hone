use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ToolError;

/// Severity level of an LSP diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A single diagnostic produced by a language server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The file the diagnostic belongs to.
    pub file: PathBuf,
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based column number.
    pub col: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// Optional diagnostic code (e.g. `"E0308"` for rustc).
    pub code: Option<String>,
}

/// A code action suggested by a language server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeAction {
    pub title: String,
    /// Optional kind string (e.g. `"quickfix"`, `"refactor"`).
    pub kind: Option<String>,
}

/// Abstraction over a language server process.
///
/// Implementations may talk to a real LSP server over JSON-RPC or be a
/// test double that returns canned data.
#[async_trait]
pub trait LspBridge: Send + Sync {
    /// Return all diagnostics for the given file.
    async fn diagnostics(&self, file: &std::path::Path) -> Result<Vec<Diagnostic>, ToolError>;

    /// Return hover documentation at the given position, if any.
    async fn hover(
        &self,
        file: &std::path::Path,
        line: u32,
        col: u32,
    ) -> Result<Option<String>, ToolError>;

    /// Return code actions available at the given position.
    async fn code_actions(
        &self,
        file: &std::path::Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<CodeAction>, ToolError>;
}
