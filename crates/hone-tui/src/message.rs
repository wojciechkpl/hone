/// The originator of a displayed message.
#[derive(Debug, Clone, PartialEq)]
pub enum DisplayRole {
    User,
    Assistant,
    System,
    ToolCall { name: String },
    ToolResult,
    Error,
}

/// A single message rendered in the conversation view.
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub role: DisplayRole,
    pub content: String,
    /// `true` while tokens are still being received for this message.
    pub is_streaming: bool,
    /// `true` when the message body is collapsed to a single summary line.
    /// Defaults to `true` for `ToolResult` messages, `false` for all others.
    pub is_collapsed: bool,
    /// `true` while this tool call is currently executing (ToolCallStart received,
    /// ToolCallEnd not yet received). Drives the animated spinner in the tool box.
    pub is_active: bool,
}

impl DisplayMessage {
    pub fn new(role: DisplayRole, content: impl Into<String>) -> Self {
        let is_collapsed = matches!(role, DisplayRole::ToolResult);
        Self {
            role,
            content: content.into(),
            is_streaming: false,
            is_collapsed,
            is_active: false,
        }
    }

    pub fn streaming(role: DisplayRole) -> Self {
        let is_collapsed = matches!(role, DisplayRole::ToolResult);
        Self {
            role,
            content: String::new(),
            is_streaming: true,
            is_collapsed,
            is_active: false,
        }
    }
}
