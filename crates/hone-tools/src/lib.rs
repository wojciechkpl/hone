mod error;
mod file;
mod git;
pub mod lsp;
mod registry;
mod shell;
mod traits;

pub use error::ToolError;
pub use file::{EditFileTool, GlobTool, ReadFileTool, WriteFileTool};
pub use git::{
    AutoCommitter, GitBranchTool, GitCheckoutTool, GitCommitTool, GitDiffTool, GitRedoTool,
    GitStatusTool, GitUndoTool, UndoStack,
};
pub use lsp::{
    AutoFixLoop, AutoFixResult, CodeAction, Diagnostic, DiagnosticSeverity, Language,
    LspBridge, LspCodeActionTool, LspDiagnosticsTool, LspHoverTool, MockLspBridge,
    StdioLspBridge, detect_language,
};
pub use registry::ToolRegistry;
pub use shell::ShellTool;
pub use traits::{Tool, ToolContext, ToolInput, ToolOutput};
