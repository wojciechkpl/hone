pub mod auto_fix;
pub mod bridge;
pub mod detect;
pub mod mock;
pub mod tools;

pub use auto_fix::{AutoFixLoop, AutoFixResult};
pub use bridge::{CodeAction, Diagnostic, DiagnosticSeverity, LspBridge};
pub use detect::{Language, detect_language};
pub use mock::MockLspBridge;
pub use tools::{LspCodeActionTool, LspDiagnosticsTool, LspHoverTool};
