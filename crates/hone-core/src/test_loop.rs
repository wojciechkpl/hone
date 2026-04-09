use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::agent::{Agent, AgentHandle};
use crate::error::AgentError;
use hone_proto::AgentEvent;

// ---------------------------------------------------------------------------
// GreenLevel
// ---------------------------------------------------------------------------

/// Tiered classification of test coverage depth.
///
/// Levels are ordered from cheapest (`TargetedTests`) to most thorough
/// (`MergeReady`).  A [`GreenContract`] specifies the `required_level` that
/// must be reached before the verification loop is considered satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GreenLevel {
    /// Just the changed file's tests (e.g., `cargo test <test_name>` or
    /// `pytest path/to/test.py`).
    TargetedTests,
    /// The whole package/crate (`cargo test --lib`).
    Package,
    /// The whole workspace (`cargo test --workspace`).
    Workspace,
    /// Workspace + lint + fmt — ready for merge.
    MergeReady,
}

impl GreenLevel {
    /// Return the next level up, or `None` if already at the maximum.
    pub fn escalate(self) -> Option<Self> {
        match self {
            Self::TargetedTests => Some(Self::Package),
            Self::Package => Some(Self::Workspace),
            Self::Workspace => Some(Self::MergeReady),
            Self::MergeReady => None,
        }
    }

    /// Return `true` if `self` has reached or exceeded `target`.
    fn has_reached(self, target: Self) -> bool {
        self as u8 >= target as u8
    }
}

// ---------------------------------------------------------------------------
// GreenCommands
// ---------------------------------------------------------------------------

/// Language/build-system specific commands for each [`GreenLevel`].
///
/// Commands are detected automatically from the project layout via
/// [`GreenCommands::detect`].  A `None` value means the level is not
/// applicable for the detected project type; the loop skips that level and
/// escalates to the next one with a command.
#[derive(Debug, Clone)]
pub struct GreenCommands {
    /// Optional: requires a test name supplied by the caller.
    pub targeted: Option<String>,
    pub package: Option<String>,
    pub workspace: Option<String>,
    pub merge_ready: Option<String>,
}

impl GreenCommands {
    /// Detect commands for the project rooted at `path`.
    ///
    /// Precedence: Rust (`Cargo.toml`) > Node (`package.json`) > Python
    /// (`pyproject.toml` / `requirements.txt`).  Returns empty `Option`s
    /// for all levels when nothing is recognised.
    pub fn detect(path: &Path) -> Self {
        if path.join("Cargo.toml").exists() {
            Self {
                targeted: None, // requires test name — caller provides
                package: Some("cargo test --lib".to_string()),
                workspace: Some("cargo test --workspace".to_string()),
                merge_ready: Some(
                    "cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check"
                        .to_string(),
                ),
            }
        } else if path.join("package.json").exists() {
            Self {
                targeted: None,
                package: Some("npm test".to_string()),
                workspace: Some("npm test".to_string()),
                merge_ready: Some("npm test && npm run lint".to_string()),
            }
        } else if path.join("pyproject.toml").exists() || path.join("requirements.txt").exists() {
            Self {
                targeted: None,
                package: Some("pytest".to_string()),
                workspace: Some("pytest".to_string()),
                merge_ready: Some("pytest && ruff check".to_string()),
            }
        } else {
            Self {
                targeted: None,
                package: None,
                workspace: None,
                merge_ready: None,
            }
        }
    }

    /// Return the command string for `level`, if one is configured.
    pub fn for_level(&self, level: GreenLevel) -> Option<&str> {
        match level {
            GreenLevel::TargetedTests => self.targeted.as_deref(),
            GreenLevel::Package => self.package.as_deref(),
            GreenLevel::Workspace => self.workspace.as_deref(),
            GreenLevel::MergeReady => self.merge_ready.as_deref(),
        }
    }
}

// ---------------------------------------------------------------------------
// GreenContract
// ---------------------------------------------------------------------------

/// Describes what level of "green" must be achieved for a task to be
/// considered complete.
pub struct GreenContract {
    /// The level the loop must reach before declaring success.
    pub required_level: GreenLevel,
    /// The level the loop starts verifying at.
    pub current_level: GreenLevel,
    /// Build-system commands for each level.
    pub commands: GreenCommands,
}

impl GreenContract {
    /// Build a contract from a single command string at `Package` level.
    ///
    /// This is the backwards-compatible constructor: callers that used the old
    /// `TestVerificationLoop::new(cmd, …)` API internally end up here.
    pub fn from_package_cmd(cmd: impl Into<String>) -> Self {
        Self {
            required_level: GreenLevel::Package,
            current_level: GreenLevel::Package,
            commands: GreenCommands {
                targeted: None,
                package: Some(cmd.into()),
                workspace: None,
                merge_ready: None,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// TestVerificationLoop
// ---------------------------------------------------------------------------

/// Iterative test-verification loop.
///
/// After code generation, runs test commands according to a [`GreenContract`]
/// and feeds any failures back to the agent so it can attempt a fix.
/// Repeats up to `max_iterations` times per level, escalating through levels
/// until `contract.required_level` is satisfied.
pub struct TestVerificationLoop {
    /// The contract describing which test levels are required.
    contract: GreenContract,
    /// Legacy single-command field — kept for the backwards-compatible
    /// `new(cmd, iterations, dir)` constructor.  When `Some`, the contract
    /// uses only this command at `Package` level.
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
    /// Create a new loop using the legacy single-command API.
    ///
    /// * `test_cmd` — shell command to run (executed via `sh -c`).
    /// * `max_iterations` — maximum number of fix attempts before giving up.
    /// * `working_dir` — working directory for the test subprocess.
    ///
    /// This constructor is backwards-compatible: internally it builds a
    /// [`GreenContract`] with `required_level = Package`.
    pub fn new(
        test_cmd: impl Into<String>,
        max_iterations: u32,
        working_dir: PathBuf,
    ) -> Self {
        let cmd = test_cmd.into();
        Self {
            contract: GreenContract::from_package_cmd(&cmd),
            test_cmd: cmd,
            max_iterations,
            working_dir,
        }
    }

    /// Create a new loop driven by a full [`GreenContract`].
    pub fn with_contract(
        contract: GreenContract,
        max_iterations: u32,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            test_cmd: String::new(),
            contract,
            max_iterations,
            working_dir,
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Return the effective command to run for the given level.
    ///
    /// When the legacy `test_cmd` field is set it takes precedence at
    /// `Package` level so that the old `new()` path is not affected.
    fn cmd_for_level(&self, level: GreenLevel) -> Option<&str> {
        if !self.test_cmd.is_empty() && level == GreenLevel::Package {
            return Some(&self.test_cmd);
        }
        self.contract.commands.for_level(level)
    }

    /// Return the first level (at or above `start`) that has a command.
    fn first_level_with_cmd(&self, start: GreenLevel) -> Option<GreenLevel> {
        let mut level = start;
        loop {
            if self.cmd_for_level(level).is_some() {
                return Some(level);
            }
            level = level.escalate()?;
        }
    }

    /// Execute one shell command and return (success, combined_output).
    async fn run_cmd(&self, cmd: &str) -> Result<(bool, String), AgentError> {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&self.working_dir)
            .output()
            .await
            .map_err(|e| AgentError::CodeMode {
                detail: format!("failed to run test command: {e}"),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Ok((output.status.success(), format!("{stdout}{stderr}")))
    }

    // -----------------------------------------------------------------------
    // Public run methods
    // -----------------------------------------------------------------------

    /// Run the verification loop against `agent`.
    ///
    /// Steps:
    /// 1. Find the effective starting level (first level with a command at or
    ///    above `contract.current_level`).
    /// 2. Run the command for that level up to `max_iterations` times,
    ///    feeding failures to the agent.
    /// 3. On pass: escalate to the next level (up to `required_level`).
    /// 4. Return when `current_level >= required_level` and tests pass, or
    ///    when the iteration budget is exhausted.
    pub async fn run(
        &self,
        agent: &mut Agent,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<TestLoopResult, AgentError> {
        // Quick-exit: no command available anywhere.
        let Some(start) = self.first_level_with_cmd(self.contract.current_level) else {
            return Ok(TestLoopResult {
                iterations: 0,
                final_status: TestStatus::NoTestCommand,
                test_output: String::new(),
            });
        };

        let mut current = start;
        let mut total_iterations: u32 = 0;
        let mut last_output = String::new();

        loop {
            // Find the command for the current level (may skip levels with no cmd).
            let Some(effective) = self.first_level_with_cmd(current) else {
                // No command at this or any higher level — done.
                break;
            };
            current = effective;

            let cmd = self.cmd_for_level(current).unwrap().to_owned();

            // Try up to max_iterations at this level.
            let mut passed = false;
            for iteration in 1..=self.max_iterations {
                total_iterations += 1;
                let (ok, out) = self.run_cmd(&cmd).await?;
                last_output = out;

                if ok {
                    passed = true;
                    break;
                }

                if iteration < self.max_iterations {
                    let fix_prompt = format!(
                        "The tests failed with this output:\n\n{last_output}\n\nPlease fix the code to make the tests pass."
                    );
                    agent.run_turn(&fix_prompt, events.clone()).await?;
                }
            }

            if !passed {
                return Ok(TestLoopResult {
                    iterations: total_iterations,
                    final_status: TestStatus::FailedAfterMax {
                        last_output: last_output.clone(),
                    },
                    test_output: last_output,
                });
            }

            // Passed at this level — check if we've satisfied the contract.
            if current.has_reached(self.contract.required_level) {
                return Ok(TestLoopResult {
                    iterations: total_iterations,
                    final_status: TestStatus::Passed,
                    test_output: last_output,
                });
            }

            // Escalate.
            match current.escalate() {
                Some(next) => current = next,
                None => break,
            }
        }

        Ok(TestLoopResult {
            iterations: total_iterations,
            final_status: TestStatus::Passed,
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
        let Some(start) = self.first_level_with_cmd(self.contract.current_level) else {
            return Ok(TestLoopResult {
                iterations: 0,
                final_status: TestStatus::NoTestCommand,
                test_output: String::new(),
            });
        };

        let mut current = start;
        let mut total_iterations: u32 = 0;
        let mut last_output = String::new();

        loop {
            let Some(effective) = self.first_level_with_cmd(current) else {
                break;
            };
            current = effective;

            let cmd = self.cmd_for_level(current).unwrap().to_owned();

            let mut passed = false;
            for iteration in 1..=self.max_iterations {
                total_iterations += 1;
                let (ok, out) = self.run_cmd(&cmd).await?;
                last_output = out;

                if ok {
                    passed = true;
                    break;
                }

                if iteration < self.max_iterations {
                    let fix_prompt = format!(
                        "The tests failed with this output:\n\n{last_output}\n\nPlease fix the code to make the tests pass."
                    );
                    handle.run_turn(&fix_prompt, events.clone()).await?;
                }
            }

            if !passed {
                return Ok(TestLoopResult {
                    iterations: total_iterations,
                    final_status: TestStatus::FailedAfterMax {
                        last_output: last_output.clone(),
                    },
                    test_output: last_output,
                });
            }

            if current.has_reached(self.contract.required_level) {
                return Ok(TestLoopResult {
                    iterations: total_iterations,
                    final_status: TestStatus::Passed,
                    test_output: last_output,
                });
            }

            match current.escalate() {
                Some(next) => current = next,
                None => break,
            }
        }

        Ok(TestLoopResult {
            iterations: total_iterations,
            final_status: TestStatus::Passed,
            test_output: last_output,
        })
    }
}
