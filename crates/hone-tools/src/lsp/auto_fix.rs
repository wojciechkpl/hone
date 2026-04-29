use std::path::Path;

use super::bridge::{Diagnostic, DiagnosticSeverity, LspBridge};
use crate::error::ToolError;

/// The outcome of running [`AutoFixLoop::run`].
#[derive(Debug)]
pub struct AutoFixResult {
    /// How many fix iterations were actually performed.
    pub iterations: u32,
    /// Diagnostics with [`DiagnosticSeverity::Error`] severity still present
    /// after the loop terminates.
    pub remaining_errors: Vec<Diagnostic>,
    /// Titles of the code actions that were applied.
    pub applied_fixes: Vec<String>,
}

/// Drives a simple auto-fix loop against an [`LspBridge`].
///
/// The loop:
/// 1. Fetches diagnostics.
/// 2. If there are no errors, it returns success immediately.
/// 3. Picks the first error and asks for code actions at that position.
/// 4. If there are no code actions available, it stops.
/// 5. Records the first action as "applied" and iterates.
/// 6. Stops after `max_retries` iterations regardless.
///
/// "Applying" an action is fully delegated to the bridge: a real bridge
/// would send a `workspace/applyEdit` request; a mock bridge can simply
/// return fewer diagnostics on the next call to simulate a successful fix.
pub struct AutoFixLoop {
    bridge: Box<dyn LspBridge>,
    max_retries: u32,
}

impl AutoFixLoop {
    pub fn new(bridge: Box<dyn LspBridge>, max_retries: u32) -> Self {
        Self {
            bridge,
            max_retries,
        }
    }

    /// Run the auto-fix loop for `file`, returning a summary of what happened.
    pub async fn run(&self, file: &Path) -> Result<AutoFixResult, ToolError> {
        let mut iterations = 0u32;
        let mut applied_fixes: Vec<String> = Vec::new();

        loop {
            let diagnostics = self.bridge.diagnostics(file).await?;
            let errors: Vec<Diagnostic> = diagnostics
                .into_iter()
                .filter(|d| d.severity == DiagnosticSeverity::Error)
                .collect();

            if errors.is_empty() {
                return Ok(AutoFixResult {
                    iterations,
                    remaining_errors: vec![],
                    applied_fixes,
                });
            }

            if iterations >= self.max_retries {
                return Ok(AutoFixResult {
                    iterations,
                    remaining_errors: errors,
                    applied_fixes,
                });
            }

            // Attempt to fix the first error.
            let first_error = &errors[0];
            let actions = self
                .bridge
                .code_actions(file, first_error.line, first_error.col)
                .await?;

            if actions.is_empty() {
                return Ok(AutoFixResult {
                    iterations,
                    remaining_errors: errors,
                    applied_fixes,
                });
            }

            applied_fixes.push(actions[0].title.clone());
            iterations += 1;
        }
    }
}
