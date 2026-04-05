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
}

impl DisplayMessage {
    pub fn new(role: DisplayRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            is_streaming: false,
        }
    }

    pub fn streaming(role: DisplayRole) -> Self {
        Self {
            role,
            content: String::new(),
            is_streaming: true,
        }
    }
}
