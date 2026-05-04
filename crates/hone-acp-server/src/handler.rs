use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::types::{AcpToolSchema, PromptContent, UpdatePayload};

/// Trait implemented by the application logic that powers an ACP server.
/// The server-side protocol layer dispatches incoming `session/prompt`
/// requests through this trait.
#[async_trait]
pub trait AcpHandler: Send + Sync {
    /// Handle one prompt. The handler streams [`UpdatePayload`] events on
    /// `updates` (which the server forwards as `session/update` notifications)
    /// and returns the final stop_reason (`"end_turn"`, `"error"`, etc.) to be
    /// echoed in the JSON-RPC response.
    async fn handle_prompt(
        &self,
        session_id: String,
        prompt: Vec<PromptContent>,
        tools: Vec<AcpToolSchema>,
        updates: mpsc::Sender<UpdatePayload>,
    ) -> Result<String, HandlerError>;
}

#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("handler error: {0}")]
    Other(String),
}

/// Test-only handler that emits a canned sequence of [`UpdatePayload`]s then
/// returns a fixed stop reason.
pub struct MockHandler {
    canned_updates: Vec<UpdatePayload>,
    stop_reason: String,
}

impl MockHandler {
    pub fn new(canned_updates: Vec<UpdatePayload>, stop_reason: impl Into<String>) -> Self {
        Self {
            canned_updates,
            stop_reason: stop_reason.into(),
        }
    }
}

#[async_trait]
impl AcpHandler for MockHandler {
    async fn handle_prompt(
        &self,
        _session_id: String,
        _prompt: Vec<PromptContent>,
        _tools: Vec<AcpToolSchema>,
        updates: mpsc::Sender<UpdatePayload>,
    ) -> Result<String, HandlerError> {
        for payload in &self.canned_updates {
            updates
                .send(payload.clone())
                .await
                .map_err(|e| HandlerError::Other(e.to_string()))?;
        }
        Ok(self.stop_reason.clone())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MessageContent, UpdatePayload};

    fn make_chunk(text: &str) -> UpdatePayload {
        UpdatePayload::MessageChunk {
            content: MessageContent {
                content_type: "text".to_owned(),
                text: text.to_owned(),
            },
        }
    }

    #[tokio::test]
    async fn mock_handler_emits_canned_updates_and_stop_reason() {
        let updates = vec![make_chunk("a"), make_chunk("b"), make_chunk("c")];
        let handler = MockHandler::new(updates.clone(), "end_turn");

        let (tx, mut rx) = mpsc::channel(16);
        let stop = handler
            .handle_prompt(
                "sess-1".to_owned(),
                vec![],
                vec![],
                tx,
            )
            .await
            .expect("handle_prompt should succeed");

        assert_eq!(stop, "end_turn");

        // All three updates must arrive in order.
        for expected in &updates {
            let got = rx.recv().await.expect("channel should yield a value");
            // Compare via serialisation so we do not need PartialEq on the enum.
            assert_eq!(
                serde_json::to_string(&got).unwrap(),
                serde_json::to_string(expected).unwrap()
            );
        }

        // Channel should be empty now (sender dropped inside handle_prompt scope).
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn mock_handler_empty_updates_returns_stop_reason_only() {
        let handler = MockHandler::new(vec![], "stop");

        let (tx, mut rx) = mpsc::channel(16);
        let stop = handler
            .handle_prompt("s".to_owned(), vec![], vec![], tx)
            .await
            .expect("handle_prompt should succeed");

        assert_eq!(stop, "stop");
        assert!(rx.recv().await.is_none(), "no updates expected");
    }
}
