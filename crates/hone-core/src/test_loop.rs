use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::agent::{Agent, AgentHandle};
use crate::error::AgentError;
use hone_proto::AgentEvent;

/// Iterative test-verification loop.
///
/// After code generation, runs the configured test command and feeds any
/// failures back to the agent so it can attempt a fix.  Repeats up to
/// `max_iterations` times (research consensus default: 3).
pub struct TestVerificationLoop {
    test_cmd: String,
    max_iterations: u32,
    working_dir: PathBuf,
}

/// The outcome of a [`TestVerificationLoop::run`] call.
pub struct TestLoopResult {
    /// Number of iterations executed (including the final one).
    pub iterations: u32,
    /// Whether the tests ultimately passed or why they stopped.
    pub final_status: TestStatus,
    /// The raw stdout+stderr from the last test run.
    pub test_output: String,
}

/// The terminal state of the test-verification loop.
pub enum TestStatus {
    /// All tests passed.
    Passed,
    /// Tests kept failing after exhausting the iteration budget.
    FailedAfterMax { last_output: String },
    /// No test command was configured; the loop was a no-op.
    NoTestCommand,
}

impl TestVerificationLoop {
    /// Create a new loop.
    ///
    /// * `test_cmd` — shell command to run (executed via `sh -c`).
    /// * `max_iterations` — maximum number of fix attempts before giving up.
    /// * `working_dir` — working directory for the test subprocess.
    pub fn new(
        test_cmd: impl Into<String>,
        max_iterations: u32,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            test_cmd: test_cmd.into(),
            max_iterations,
            working_dir,
        }
    }

    /// Run the verification loop against `agent`.
    ///
    /// Steps per iteration:
    /// 1. Execute `test_cmd` via `sh -c` with stdout and stderr captured.
    /// 2. If exit code 0 → return [`TestStatus::Passed`].
    /// 3. If exit code != 0 → send failure output to the agent with a fix
    ///    prompt and run another agent turn.
    /// 4. Repeat until the tests pass or `max_iterations` is exhausted.
    pub async fn run(
        &self,
        agent: &mut Agent,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<TestLoopResult, AgentError> {
        if self.test_cmd.is_empty() {
            return Ok(TestLoopResult {
                iterations: 0,
                final_status: TestStatus::NoTestCommand,
                test_output: String::new(),
            });
        }

        let mut last_output = String::new();

        for iteration in 1..=self.max_iterations {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&self.test_cmd)
                .current_dir(&self.working_dir)
                .output()
                .await
                .map_err(|e| AgentError::CodeMode {
                    detail: format!("failed to run test command: {e}"),
                })?;

            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            last_output = format!("{stdout}{stderr}");

            if output.status.success() {
                return Ok(TestLoopResult {
                    iterations: iteration,
                    final_status: TestStatus::Passed,
                    test_output: last_output,
                });
            }

            // Tests failed — ask the agent to fix the code, unless this was
            // the last iteration.
            if iteration < self.max_iterations {
                let fix_prompt = format!(
                    "The tests failed with this output:\n\n{last_output}\n\nPlease fix the code to make the tests pass."
                );
                agent.run_turn(&fix_prompt, events.clone()).await?;
            }
        }

        Ok(TestLoopResult {
            iterations: self.max_iterations,
            final_status: TestStatus::FailedAfterMax {
                last_output: last_output.clone(),
            },
            test_output: last_output,
        })
    }

    /// Run the verification loop against an [`AgentHandle`] actor.
    ///
    /// Semantics are identical to [`Self::run`]; fix-attempt turns are
    /// dispatched through the handle's command channel.
    pub async fn run_with_handle(
        &self,
        handle: &AgentHandle,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<TestLoopResult, AgentError> {
        if self.test_cmd.is_empty() {
            return Ok(TestLoopResult {
                iterations: 0,
                final_status: TestStatus::NoTestCommand,
                test_output: String::new(),
            });
        }

        let mut last_output = String::new();

        for iteration in 1..=self.max_iterations {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&self.test_cmd)
                .current_dir(&self.working_dir)
                .output()
                .await
                .map_err(|e| AgentError::CodeMode {
                    detail: format!("failed to run test command: {e}"),
                })?;

            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            last_output = format!("{stdout}{stderr}");

            if output.status.success() {
                return Ok(TestLoopResult {
                    iterations: iteration,
                    final_status: TestStatus::Passed,
                    test_output: last_output,
                });
            }

            if iteration < self.max_iterations {
                let fix_prompt = format!(
                    "The tests failed with this output:\n\n{last_output}\n\nPlease fix the code to make the tests pass."
                );
                handle.run_turn(&fix_prompt, events.clone()).await?;
            }
        }

        Ok(TestLoopResult {
            iterations: self.max_iterations,
            final_status: TestStatus::FailedAfterMax {
                last_output: last_output.clone(),
            },
            test_output: last_output,
        })
    }
}
