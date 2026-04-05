use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::bridge::{CodeAction, Diagnostic, LspBridge};
use crate::error::ToolError;

/// Internal mutable state shared between clones of [`MockLspBridge`].
struct MockState {
    /// Call count for `diagnostics()`.
    diagnostics_call_count: u32,
    /// Sequence of diagnostic batches to return on successive calls.
    /// Once exhausted, the last batch is repeated.
    diagnostics_sequence: Vec<Vec<Diagnostic>>,
    /// Fixed hover text to return (if any).
    hover_text: Option<String>,
    /// Fixed code actions to return.
    code_actions: Vec<CodeAction>,
}

/// A test-double implementation of [`LspBridge`] that returns configurable
/// canned responses and tracks call counts.
///
/// ```text
/// let bridge = MockLspBridge::builder()
///     .diagnostics_sequence(vec![
///         vec![diag1.clone()],  // first call returns one error
///         vec![],               // second call: fixed
///     ])
///     .hover_text("fn foo() -> u32")
///     .build();
/// ```
pub struct MockLspBridge {
    state: Arc<Mutex<MockState>>,
}

impl MockLspBridge {
    /// Create a builder for configuring the mock.
    pub fn builder() -> MockLspBridgeBuilder {
        MockLspBridgeBuilder::new()
    }

    /// How many times `diagnostics()` has been called.
    pub fn diagnostics_call_count(&self) -> u32 {
        self.state.lock().unwrap().diagnostics_call_count
    }
}

/// Builder for [`MockLspBridge`].
pub struct MockLspBridgeBuilder {
    diagnostics_sequence: Vec<Vec<Diagnostic>>,
    hover_text: Option<String>,
    code_actions: Vec<CodeAction>,
}

impl MockLspBridgeBuilder {
    fn new() -> Self {
        Self {
            diagnostics_sequence: vec![vec![]],
            hover_text: None,
            code_actions: vec![],
        }
    }

    /// Set a fixed list of diagnostics that is returned on every call.
    pub fn diagnostics(mut self, diags: Vec<Diagnostic>) -> Self {
        self.diagnostics_sequence = vec![diags];
        self
    }

    /// Set a sequence of diagnostic batches returned on successive calls.
    /// After all batches are consumed the last one is repeated.
    pub fn diagnostics_sequence(mut self, seq: Vec<Vec<Diagnostic>>) -> Self {
        self.diagnostics_sequence = seq;
        self
    }

    /// Set the hover text returned by [`LspBridge::hover`].
    pub fn hover_text(mut self, text: impl Into<String>) -> Self {
        self.hover_text = Some(text.into());
        self
    }

    /// Set the code actions returned by [`LspBridge::code_actions`].
    pub fn code_actions(mut self, actions: Vec<CodeAction>) -> Self {
        self.code_actions = actions;
        self
    }

    pub fn build(self) -> MockLspBridge {
        let seq = if self.diagnostics_sequence.is_empty() {
            vec![vec![]]
        } else {
            self.diagnostics_sequence
        };
        MockLspBridge {
            state: Arc::new(Mutex::new(MockState {
                diagnostics_call_count: 0,
                diagnostics_sequence: seq,
                hover_text: self.hover_text,
                code_actions: self.code_actions,
            })),
        }
    }
}

#[async_trait]
impl LspBridge for MockLspBridge {
    async fn diagnostics(&self, _file: &Path) -> Result<Vec<Diagnostic>, ToolError> {
        let mut state = self.state.lock().unwrap();
        let idx = (state.diagnostics_call_count as usize)
            .min(state.diagnostics_sequence.len().saturating_sub(1));
        let batch = state.diagnostics_sequence[idx].clone();
        state.diagnostics_call_count += 1;
        Ok(batch)
    }

    async fn hover(&self, _file: &Path, _line: u32, _col: u32) -> Result<Option<String>, ToolError> {
        let state = self.state.lock().unwrap();
        Ok(state.hover_text.clone())
    }

    async fn code_actions(
        &self,
        _file: &Path,
        _line: u32,
        _col: u32,
    ) -> Result<Vec<CodeAction>, ToolError> {
        let state = self.state.lock().unwrap();
        Ok(state.code_actions.clone())
    }
}
