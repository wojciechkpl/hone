mod error;
mod file;
mod git;
pub mod handoff;
pub mod lsp;
pub mod memory;
mod registry;
mod screen;
mod shell;
mod subjects;
mod traits;
pub mod web;

pub use error::ToolError;
pub use memory::MemoryReadTool;
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
pub use shell::{
    EXTRA_READ_ENV, HOME_READ_DIRS, SANDBOX_ENFORCE_ENV, SYSTEM_READ_ROOTS, WRITE_DEVICES,
    parse_extra_read, sandbox_enforced, shell_sandbox_policy,
};
pub use subjects::{ShellParse, SubjectContext, ToolSubjects, parse_shell, subject_path};
pub use traits::{Tool, ToolContext, ToolInput, ToolOutput};
pub use web::{SearchResult, WebFetchTool, WebSearchTool};
