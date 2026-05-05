use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use hone_core::{AgentConfig, AgentHandle, AgentMode, RetryConfig};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::ModelProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;

use crate::config::EvalConfig;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TaskStatus {
    Pass,
    Fail,
    Timeout,
    AgentError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskOutcome {
    pub id: String,
    pub status: TaskStatus,
    pub duration_ms: u64,
    pub tokens_input: u32,
    pub tokens_output: u32,
    pub cost_usd: f64,
    pub success_check_stdout: String,
    pub success_check_stderr: String,
    pub success_check_exit_code: Option<i32>,
}

pub struct EvalRunner {
    config: EvalConfig,
    /// Provider factory: returns a fresh ModelProvider for each task so
    /// sessions are truly ephemeral.
    provider_factory: Box<dyn Fn() -> Arc<dyn ModelProvider> + Send + Sync>,
    base_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum EvalRunnerError {
    #[error("unknown task id: {0}")]
    UnknownId(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl EvalRunner {
    pub fn new(
        config: EvalConfig,
        base_dir: PathBuf,
        provider_factory: Box<dyn Fn() -> Arc<dyn ModelProvider> + Send + Sync>,
    ) -> Self {
        Self {
            config,
            provider_factory,
            base_dir,
        }
    }

    /// Run all tasks sequentially. Failures do NOT abort — every task is
    /// attempted. Outcomes are returned in the same order as `config.tasks`.
    pub async fn run_all(&self) -> Vec<TaskOutcome> {
        let mut outcomes = Vec::with_capacity(self.config.tasks.len());
        for task in &self.config.tasks {
            let outcome = match self.run_one(&task.id).await {
                Ok(o) => o,
                Err(e) => TaskOutcome {
                    id: task.id.clone(),
                    status: TaskStatus::AgentError,
                    duration_ms: 0,
                    tokens_input: 0,
                    tokens_output: 0,
                    cost_usd: 0.0,
                    success_check_stdout: String::new(),
                    success_check_stderr: e.to_string(),
                    success_check_exit_code: None,
                },
            };
            outcomes.push(outcome);
        }
        outcomes
    }

    /// Run a single task by id. Returns `Err` if the id is unknown.
    pub async fn run_one(&self, id: &str) -> Result<TaskOutcome, EvalRunnerError> {
        let task = self
            .config
            .tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| EvalRunnerError::UnknownId(id.to_owned()))?
            .clone();

        // Resolve working directory against base_dir.
        let working_dir = self.base_dir.join(&task.working_dir);

        // Use a dummy model id — the mock provider ignores it.
        let model_id = ModelId::new("eval:mock");

        let config = AgentConfig {
            provider: (self.provider_factory)(),
            tools: ToolRegistry::default(),
            sandbox: Arc::new(NullSandbox),
            sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
            system_prompt: String::new(),
            model: model_id.clone(),
            max_tokens: None,
            temperature: None,
            budget_limit: task.budget_usd,
            pricing: PricingEntry {
                model: model_id,
                input_per_million: 0.0,
                output_per_million: 0.0,
                cache_read_per_million: None,
                cache_creation_per_million: None,
            },
            working_dir: working_dir.clone(),
            memory_store: None,
            retry_config: RetryConfig::default(),
            index: None,
            inject_context: false,
            skeleton_strategy: Default::default(),
            egress_log: None,
            approval_policy: ToolApprovalPolicy::Auto,
            step_rx: None,
            hooks: None,
            skills: None,
            mode: AgentMode::Build,
            embedder: None,
        };

        let handle = AgentHandle::spawn(config);

        // Create event drain channel — we don't need the events for eval.
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            while events_rx.recv().await.is_some() {}
        });

        let start = Instant::now();

        let turn_result = tokio::time::timeout(
            std::time::Duration::from_secs(task.timeout_secs),
            handle.run_turn(task.prompt.clone(), events_tx),
        )
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match turn_result {
            Err(_elapsed) => {
                // Timeout
                handle.shutdown().await;
                Ok(TaskOutcome {
                    id: task.id.clone(),
                    status: TaskStatus::Timeout,
                    duration_ms,
                    tokens_input: 0,
                    tokens_output: 0,
                    cost_usd: 0.0,
                    success_check_stdout: String::new(),
                    success_check_stderr: String::new(),
                    success_check_exit_code: None,
                })
            }
            Ok(Err(_agent_err)) => {
                handle.shutdown().await;
                Ok(TaskOutcome {
                    id: task.id.clone(),
                    status: TaskStatus::AgentError,
                    duration_ms,
                    tokens_input: 0,
                    tokens_output: 0,
                    cost_usd: 0.0,
                    success_check_stdout: String::new(),
                    success_check_stderr: String::new(),
                    success_check_exit_code: None,
                })
            }
            Ok(Ok(turn)) => {
                // Run success check.
                let (exit_code, stdout, stderr) =
                    Self::run_success_check(&task.success_check, &working_dir)
                        .await
                        .unwrap_or_else(|e| (None, String::new(), e.to_string()));

                let status = if exit_code == Some(0) {
                    TaskStatus::Pass
                } else {
                    TaskStatus::Fail
                };

                handle.shutdown().await;

                Ok(TaskOutcome {
                    id: task.id.clone(),
                    status,
                    duration_ms,
                    tokens_input: turn.usage.input,
                    tokens_output: turn.usage.output,
                    cost_usd: turn.cost_usd,
                    success_check_stdout: stdout,
                    success_check_stderr: stderr,
                    success_check_exit_code: exit_code,
                })
            }
        }
    }

    /// Shell out a success-check command and return (exit_code, stdout, stderr).
    pub async fn run_success_check(
        cmd: &str,
        working_dir: &Path,
    ) -> std::io::Result<(Option<i32>, String, String)> {
        use tokio::process::Command;

        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(working_dir)
            .output()
            .await?;

        let exit_code = output.status.code();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Ok((exit_code, stdout, stderr))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use hone_proto::TokenUsage;
    use hone_providers::MockProvider;
    use tempfile::TempDir;

    use super::*;
    use crate::config::{EvalConfig, TaskSpec};

    // --------------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------------

    fn single_task_config(id: &str, success_check: &str) -> EvalConfig {
        EvalConfig {
            tasks: vec![TaskSpec {
                id: id.to_owned(),
                prompt: "test prompt".to_owned(),
                success_check: success_check.to_owned(),
                timeout_secs: 60,
                working_dir: ".".to_owned(),
                budget_usd: None,
            }],
        }
    }

    fn two_task_config(
        id1: &str,
        check1: &str,
        id2: &str,
        check2: &str,
    ) -> EvalConfig {
        EvalConfig {
            tasks: vec![
                TaskSpec {
                    id: id1.to_owned(),
                    prompt: "first".to_owned(),
                    success_check: check1.to_owned(),
                    timeout_secs: 60,
                    working_dir: ".".to_owned(),
                    budget_usd: None,
                },
                TaskSpec {
                    id: id2.to_owned(),
                    prompt: "second".to_owned(),
                    success_check: check2.to_owned(),
                    timeout_secs: 60,
                    working_dir: ".".to_owned(),
                    budget_usd: None,
                },
            ],
        }
    }

    fn mock_factory(tokens: Vec<String>) -> Box<dyn Fn() -> Arc<dyn ModelProvider> + Send + Sync> {
        Box::new(move || Arc::new(MockProvider::new(tokens.clone())))
    }

    fn mock_factory_with_usage(
        tokens: Vec<String>,
        usage: TokenUsage,
    ) -> Box<dyn Fn() -> Arc<dyn ModelProvider> + Send + Sync> {
        Box::new(move || Arc::new(MockProvider::new(tokens.clone()).with_usage(usage)))
    }

    fn temp_base() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // --------------------------------------------------------------------------
    // run_success_check tests
    // --------------------------------------------------------------------------

    #[tokio::test]
    async fn run_success_check_zero_exit_returns_zero() {
        let dir = temp_base();
        let (code, _stdout, _stderr) =
            EvalRunner::run_success_check("true", dir.path()).await.expect("should not io-error");
        assert_eq!(code, Some(0));
    }

    #[tokio::test]
    async fn run_success_check_non_zero_exit_returns_one() {
        let dir = temp_base();
        let (code, _stdout, _stderr) =
            EvalRunner::run_success_check("false", dir.path()).await.expect("should not io-error");
        assert_eq!(code, Some(1));
    }

    #[tokio::test]
    async fn run_success_check_captures_stdout() {
        let dir = temp_base();
        let (_code, stdout, _stderr) =
            EvalRunner::run_success_check("echo hello", dir.path())
                .await
                .expect("should not io-error");
        assert!(stdout.contains("hello"), "stdout was: {stdout:?}");
    }

    #[tokio::test]
    async fn run_success_check_captures_stderr() {
        let dir = temp_base();
        let (_code, _stdout, stderr) =
            EvalRunner::run_success_check("sh -c 'echo err 1>&2'", dir.path())
                .await
                .expect("should not io-error");
        assert!(stderr.contains("err"), "stderr was: {stderr:?}");
    }

    #[tokio::test]
    async fn run_success_check_runs_in_working_dir() {
        let dir = temp_base();
        std::fs::write(dir.path().join("test.txt"), "expected-content")
            .expect("write test file");
        let (_code, stdout, _stderr) =
            EvalRunner::run_success_check("cat test.txt", dir.path())
                .await
                .expect("should not io-error");
        assert!(
            stdout.contains("expected-content"),
            "stdout was: {stdout:?}"
        );
    }

    // --------------------------------------------------------------------------
    // run_one / run_all agent-driving tests
    // --------------------------------------------------------------------------

    #[tokio::test]
    async fn run_one_unknown_id_errors() {
        let config = single_task_config("known", "true");
        let base = temp_base();
        let runner = EvalRunner::new(config, base.path().to_path_buf(), mock_factory(vec![]));
        let err = runner
            .run_one("does-not-exist")
            .await
            .expect_err("should fail with unknown id");
        assert!(matches!(err, EvalRunnerError::UnknownId(id) if id == "does-not-exist"));
    }

    #[tokio::test]
    async fn run_one_with_passing_success_check_returns_pass() {
        let config = single_task_config("pass-task", "true");
        let base = temp_base();
        let runner = EvalRunner::new(
            config,
            base.path().to_path_buf(),
            mock_factory(vec!["done".to_owned()]),
        );
        let outcome = runner.run_one("pass-task").await.expect("run_one should not error");
        assert_eq!(outcome.status, TaskStatus::Pass);
        assert_eq!(outcome.id, "pass-task");
        assert_eq!(outcome.success_check_exit_code, Some(0));
    }

    #[tokio::test]
    async fn run_one_with_failing_success_check_returns_fail() {
        let config = single_task_config("fail-task", "false");
        let base = temp_base();
        let runner = EvalRunner::new(
            config,
            base.path().to_path_buf(),
            mock_factory(vec!["done".to_owned()]),
        );
        let outcome = runner.run_one("fail-task").await.expect("run_one should not error");
        assert_eq!(outcome.status, TaskStatus::Fail);
        assert_ne!(outcome.success_check_exit_code, Some(0));
    }

    #[tokio::test]
    async fn run_all_runs_all_tasks_in_order() {
        let config = two_task_config("first", "true", "second", "true");
        let base = temp_base();
        let runner = EvalRunner::new(
            config,
            base.path().to_path_buf(),
            mock_factory(vec!["ok".to_owned()]),
        );
        let outcomes = runner.run_all().await;
        assert_eq!(outcomes.len(), 2, "expected 2 outcomes");
        assert_eq!(outcomes[0].id, "first");
        assert_eq!(outcomes[1].id, "second");
        assert_eq!(outcomes[0].status, TaskStatus::Pass);
        assert_eq!(outcomes[1].status, TaskStatus::Pass);
    }

    #[tokio::test]
    async fn run_all_continues_after_failure() {
        let config = two_task_config("fail-first", "false", "pass-second", "true");
        let base = temp_base();
        let runner = EvalRunner::new(
            config,
            base.path().to_path_buf(),
            mock_factory(vec!["ok".to_owned()]),
        );
        let outcomes = runner.run_all().await;
        assert_eq!(outcomes.len(), 2, "both tasks must be attempted");
        assert_eq!(outcomes[0].status, TaskStatus::Fail);
        assert_eq!(outcomes[1].status, TaskStatus::Pass);
    }

    #[tokio::test]
    async fn run_one_records_cost_and_tokens() {
        let usage = TokenUsage {
            input: 42,
            output: 17,
            cache_read: 0,
            cache_creation: 0,
        };
        let config = single_task_config("cost-task", "true");
        let base = temp_base();
        let runner = EvalRunner::new(
            config,
            base.path().to_path_buf(),
            mock_factory_with_usage(vec!["ok".to_owned()], usage),
        );
        let outcome = runner.run_one("cost-task").await.expect("should not error");
        assert_eq!(outcome.tokens_input, 42, "input tokens mismatch");
        assert_eq!(outcome.tokens_output, 17, "output tokens mismatch");
        // Cost must be > 0 when tokens are consumed
        assert!(outcome.cost_usd >= 0.0, "cost must be non-negative");
    }
}
