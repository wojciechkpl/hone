mod error;
mod file;
mod git;
pub mod handoff;
pub mod lsp;
mod registry;
mod screen;
mod shell;
mod traits;
pub mod web;

pub use error::ToolError;
pub use handoff::HandoffTool;
pub use file::{EditFileTool, GlobTool, GrepTool, MultiEditTool, PatchTool, ReadFileTool, WriteFileTool};
pub use git::{
    AutoCommitter, GitBranchTool, GitCheckoutTool, GitCommitTool, GitDiffTool, GitRedoTool,
    GitStatusTool, GitUndoTool, UndoStack,
};
pub use lsp::{
    AutoFixLoop, AutoFixResult, CodeAction, Diagnostic, DiagnosticSeverity, Language, LspBridge,
    LspCodeActionTool, LspDiagnosticsTool, LspHoverTool, MockLspBridge, StdioLspBridge,
    detect_language,
};
pub use registry::{ToolRegistry, ToolRegistryBuilder};
pub use screen::{ComputerControlTool, ScreenCaptureTool};
pub use shell::ShellTool;
pub use shell::validation::{CommandIntent, ValidationResult};
pub use traits::{Tool, ToolContext, ToolInput, ToolOutput};
pub use web::{SearchResult, WebFetchTool, WebSearchTool};
