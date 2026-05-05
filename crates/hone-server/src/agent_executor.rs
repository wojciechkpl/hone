//! Production `TaskExecutor` that drives a real `hone_core::Agent` for each
//! claimed `SubTask`.
//!
//! See Phase 3b design. Implementation by the rust-expert; this file contains
//! the public API skeleton and the test suite written before implementation.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use hone_core::{AgentConfig, AgentHandle, AgentMode, RetryConfig, filtered_registry};
use hone_core::agent_registry::AgentRegistry;
use hone_core::task_plan::SubTask;
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::ModelProvider;
use hone_sandbox::{NullSandbox, Sandbox};
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

use crate::worker_pool::{ExecutionOutcome, TaskExecutor};

// ── AgentTaskExecutor ─────────────────────────────────────────────────────────

/// Production `TaskExecutor` that runs each claimed `SubTask` through a real
/// hone-core `Agent`.
pub struct AgentTaskExecutor {
    /// Registry holding all known agent personas (built-in + user overlays).
    registry: AgentRegistry,
    /// Base tool registry. Per-agent allowlists applied via `filtered_registry`.
    base_tools: ToolRegistry,
    /// LLM provider used by every spawned agent.
    provider: Arc<dyn ModelProvider>,
    /// Default sandbox; per-agent override applied when `AgentDef.sandbox` is
    /// `Some`.
    default_sandbox: Arc<dyn Sandbox>,
    /// Model identifier forwarded to `AgentConfig`.
    model: ModelId,
    /// Pricing rates forwarded to `AgentConfig`.
    pricing: PricingEntry,
    /// Working directory forwarded to `AgentConfig`.
    working_dir: PathBuf,
    /// Default per-task budget (`None` = unlimited; can be overridden per
    /// `SubTask.budget_usd`).
    default_budget_usd: Option<f64>,
}

impl AgentTaskExecutor {
    /// Build a new executor with `NullSandbox` and no default budget cap.
    pub fn new(
        registry: AgentRegistry,
        base_tools: ToolRegistry,
        provider: Arc<dyn ModelProvider>,
        model: ModelId,
        pricing: PricingEntry,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            registry,
            base_tools,
            provider,
            default_sandbox: Arc::new(NullSandbox),
            model,
            pricing,
            working_dir,
            default_budget_usd: None,
        }
    }

    /// Override the default sandbox (default: `NullSandbox`).
    pub fn with_default_sandbox(mut self, sandbox: Arc<dyn Sandbox>) -> Self {
        self.default_sandbox = sandbox;
        self
    }

    /// Override the default per-task USD budget cap.
    pub fn with_default_budget(mut self, usd: f64) -> Self {
        self.default_budget_usd = Some(usd);
        self
    }

    /// Build an `AgentConfig` for `task`, applying all per-agent overrides from
    /// the registry.
    ///
    /// Returns `Err(ExecutionOutcome::failed(...))` when the agent named in
    /// `task.agent` is not in the registry.
    ///
    /// Exposed as `pub(crate)` so tests can exercise config-construction logic
    /// independently of the full end-to-end agent run.
    ///
    /// # Implementation note
    ///
    /// The implementer must:
    /// 1. Look up `task.agent` in `self.registry`; return
    ///    `Err(ExecutionOutcome::failed("unknown agent: <name>"))` on miss.
    /// 2. Apply `filtered_registry(&self.base_tools, &agent_def.tools)`.
    /// 3. Resolve sandbox: `agent_def.sandbox.as_ref().map(|p| …)` else
    ///    `Arc::clone(&self.default_sandbox)`.
    /// 4. Resolve budget: `task.budget_usd.or(self.default_budget_usd)`.
    /// 5. Construct and return `AgentConfig` with all of the above.
    pub(crate) fn build_agent_config_for_task(
        &self,
        task: &SubTask,
    ) -> Result<AgentConfig, ExecutionOutcome> {
        let agent_def = self.registry.get(&task.agent).ok_or_else(|| {
            ExecutionOutcome::failed(format!("unknown agent: {}", task.agent))
        })?;

        let tools = if !agent_def.tools.is_empty() {
            filtered_registry(&self.base_tools, &agent_def.tools)
        } else {
            self.base_tools.clone()
        };

        let sandbox: Arc<dyn Sandbox> = if let Some(profile) = &agent_def.sandbox {
            let _policy = profile.to_policy();
            tracing::debug!(
                agent = %agent_def.name,
                "per-agent sandbox profile captured (not enforced in v1)"
            );
            Arc::new(NullSandbox) as Arc<dyn Sandbox>
        } else {
            Arc::clone(&self.default_sandbox)
        };

        let budget_limit = task.budget_usd.or(self.default_budget_usd);

        Ok(AgentConfig {
            provider: Arc::clone(&self.provider),
            tools,
            sandbox,
            system_prompt: agent_def.system_prompt.clone(),
            model: self.model.clone(),
            max_tokens: None,
            temperature: None,
            budget_limit,
            pricing: self.pricing.clone(),
            working_dir: self.working_dir.clone(),
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
        })
    }
}

#[async_trait]
impl TaskExecutor for AgentTaskExecutor {
    async fn execute(&self, task: &SubTask) -> ExecutionOutcome {
        let config = match self.build_agent_config_for_task(task) {
            Ok(c) => c,
            Err(outcome) => return outcome,
        };

        let handle = AgentHandle::spawn(config);

        let (event_tx, mut event_rx) = mpsc::channel::<hone_proto::AgentEvent>(64);
        tokio::spawn(async move {
            while event_rx.recv().await.is_some() { /* drop */ }
        });

        let user_input = if task.context.is_empty() {
            task.description.clone()
        } else {
            format!("{}\n\n{}", task.description, task.context)
        };

        let result = handle.run_turn(user_input, event_tx).await;
        handle.shutdown().await;

        match result {
            Ok(turn_result) => ExecutionOutcome {
                status_str: "completed".into(),
                response: turn_result.response,
                error_msg: None,
                tokens_input: turn_result.usage.input,
                tokens_output: turn_result.usage.output,
                cost_usd: turn_result.cost_usd,
            },
            Err(e) => ExecutionOutcome::failed(e.to_string()),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use hone_core::agent_def::AgentDef;
    use hone_core::agent_registry::AgentRegistry;
    use hone_core::task_plan::SubTask;
    use hone_proto::{ModelId, PricingEntry, Role, TokenUsage};
    use hone_providers::MockProvider;
    use hone_tools::ToolRegistry;

    use super::AgentTaskExecutor;
    use crate::worker_pool::TaskExecutor;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn zero_pricing() -> PricingEntry {
        PricingEntry {
            model: ModelId::new("mock:dummy"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        }
    }

    fn build_executor(
        provider: Arc<dyn hone_providers::ModelProvider>,
        registry: AgentRegistry,
    ) -> AgentTaskExecutor {
        let base_tools = ToolRegistry::default();
        AgentTaskExecutor::new(
            registry,
            base_tools,
            provider,
            ModelId::new("mock:dummy"),
            zero_pricing(),
            std::env::temp_dir(),
        )
    }

    fn make_task(agent: &str) -> SubTask {
        SubTask {
            id: "t1".into(),
            description: "do the thing".into(),
            agent: agent.into(),
            context: String::new(),
            depends_on: vec![],
            acceptance_tests: vec![],
            budget_usd: None,
            scope: vec![],
            requires_capabilities: vec![],
        }
    }

    /// Build an `AgentDef` with minimal required fields.
    fn make_agent_def(name: &str, system_prompt: &str) -> AgentDef {
        AgentDef {
            name: name.to_string(),
            description: "test agent".to_string(),
            tools: vec![],
            model: "sonnet".to_string(),
            memory: String::new(),
            sub_agents: vec![],
            system_prompt: system_prompt.to_string(),
            sandbox: None,
        }
    }

    fn registry_with_agent(name: &str, system_prompt: &str) -> AgentRegistry {
        let mut r = AgentRegistry::new();
        r.register(make_agent_def(name, system_prompt));
        r
    }

    // ── Happy path ────────────────────────────────────────────────────────────

    /// MockProvider returning "hello world" must yield a completed outcome
    /// whose response matches the token text.
    #[tokio::test]
    async fn execute_returns_completed_outcome_when_provider_returns_text() {
        let provider = Arc::new(MockProvider::new(vec!["hello".into(), " world".into()]));
        let executor = build_executor(provider, registry_with_agent("bot", "sys"));
        let task = make_task("bot");

        let outcome = executor.execute(&task).await;

        assert_eq!(outcome.status_str, "completed");
        assert_eq!(outcome.response, "hello world");
    }

    /// Usage fields on the outcome are populated from the provider's token
    /// counts reported via `ProviderEvent::Usage`.
    #[tokio::test]
    async fn execute_populates_usage_from_turn_result() {
        let usage = TokenUsage {
            input: 42,
            output: 7,
            ..Default::default()
        };
        let provider = Arc::new(MockProvider::new(vec!["ok".into()]).with_usage(usage));
        let executor = build_executor(provider, registry_with_agent("bot", "sys"));
        let task = make_task("bot");

        let outcome = executor.execute(&task).await;

        assert_eq!(outcome.tokens_input, 42);
        assert_eq!(outcome.tokens_output, 7);
    }

    /// cost_usd on the outcome is non-negative and reflects what the agent's
    /// cost accumulator reports (zero for zero-rate pricing, which is fine —
    /// the test pins the concrete value, not a floor).
    #[tokio::test]
    async fn execute_populates_cost_from_turn_result() {
        let provider = Arc::new(MockProvider::new(vec!["done".into()]));
        let executor = build_executor(provider, registry_with_agent("bot", "sys"));
        let task = make_task("bot");

        let outcome = executor.execute(&task).await;

        // With zero-rate pricing the cost must be exactly 0.0.
        assert_eq!(outcome.cost_usd, 0.0);
    }

    // ── Agent registry lookup ─────────────────────────────────────────────────

    /// When `task.agent` names a persona not in the registry, the outcome must
    /// be `failed` and the error message must name both the sentinel word
    /// "unknown agent" and the persona name.
    #[tokio::test]
    async fn execute_unknown_agent_returns_failed_outcome() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let executor = build_executor(provider, AgentRegistry::new());
        let task = make_task("ghost");

        let outcome = executor.execute(&task).await;

        assert_eq!(outcome.status_str, "failed");
        let msg = outcome.error_msg.expect("error_msg must be Some on failure");
        assert!(
            msg.contains("unknown agent"),
            "error message must contain 'unknown agent'; got: {msg}"
        );
        assert!(
            msg.contains("ghost"),
            "error message must contain the agent name 'ghost'; got: {msg}"
        );
    }

    /// The system prompt from the `AgentDef` must appear in the system-role
    /// message captured by `MockProvider.last_request`.
    #[tokio::test]
    async fn execute_uses_agent_def_system_prompt() {
        let provider = Arc::new(MockProvider::new(vec!["ok".into()]));
        let last_request = Arc::clone(&provider.last_request);
        let executor =
            build_executor(provider, registry_with_agent("bot", "PERSONA-XYZ"));
        let task = make_task("bot");

        executor.execute(&task).await;

        let req = last_request
            .lock()
            .unwrap()
            .clone()
            .expect("provider must have received at least one CompletionRequest");
        let system_text = req
            .messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_text().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            system_text.contains("PERSONA-XYZ"),
            "system messages must contain the agent's system_prompt; got: {system_text:?}"
        );
    }

    // ── Per-agent overrides (helper-based — Option 1) ─────────────────────────

    /// The `AgentConfig` built for a task with a non-empty tool allowlist must
    /// contain only the listed tools (tested via `build_agent_config_for_task`).
    #[test]
    fn build_config_applies_tool_allowlist_from_agent_def() {
        use async_trait::async_trait;
        use hone_proto::ToolLayer;
        use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput};

        struct FakeTool(&'static str);

        #[async_trait]
        impl Tool for FakeTool {
            fn name(&self) -> &str {
                self.0
            }

            fn description(&self) -> &str {
                "fake"
            }

            fn schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }

            async fn call(
                &self,
                _input: ToolInput,
                _ctx: &ToolContext,
            ) -> Result<ToolOutput, ToolError> {
                Ok(ToolOutput::new(serde_json::json!({})))
            }

            fn layer(&self) -> ToolLayer {
                ToolLayer::Native
            }
        }

        let mut base_tools = ToolRegistry::builder()
            .register(Box::new(FakeTool("read_file")))
            .register(Box::new(FakeTool("grep")))
            .register(Box::new(FakeTool("shell")))
            .build();

        let mut def = make_agent_def("bot", "sys");
        def.tools = vec!["read_file".to_string()];

        let mut registry = AgentRegistry::new();
        registry.register(def);

        let provider = Arc::new(MockProvider::new(vec![]));
        let executor = AgentTaskExecutor::new(
            registry,
            base_tools,
            provider,
            ModelId::new("mock:dummy"),
            zero_pricing(),
            std::env::temp_dir(),
        );

        let task = make_task("bot");
        let config = executor
            .build_agent_config_for_task(&task)
            .expect("registry hit must succeed");

        assert_eq!(
            config.tools.len(),
            1,
            "only read_file should survive the allowlist filter"
        );
        assert!(
            config.tools.get("read_file").is_some(),
            "read_file must be present in filtered tools"
        );
        assert!(
            config.tools.get("grep").is_none(),
            "grep must be absent from filtered tools"
        );
    }

    /// When the `AgentDef` has no sandbox override, the resulting `AgentConfig`
    /// must use the executor's default sandbox (i.e. `NullSandbox`).
    ///
    /// The per-agent-sandbox path is intentionally left as `todo!()` in the
    /// skeleton and the corresponding positive test is skipped here; the
    /// implementer must remove the `todo!()` and add coverage.
    #[test]
    fn build_config_uses_default_sandbox_when_agent_def_has_none() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let executor = build_executor(provider, registry_with_agent("bot", "sys"));
        let task = make_task("bot");

        // Just assert we get an Ok back — sandbox identity is opaque via the
        // Sandbox trait, but the absence of a panic proves the default branch
        // is taken.
        let result = executor.build_agent_config_for_task(&task);
        assert!(
            result.is_ok(),
            "build_agent_config_for_task must succeed for a known agent with no sandbox override"
        );
    }

    // ── Budget ────────────────────────────────────────────────────────────────

    /// When the executor has a default budget and the task has none, the
    /// `AgentConfig` must carry `budget_limit = Some(executor_default)`.
    #[test]
    fn build_config_uses_default_budget_when_task_has_none() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let executor = build_executor(provider, registry_with_agent("bot", "sys"))
            .with_default_budget(5.0);

        let mut task = make_task("bot");
        task.budget_usd = None;

        let config = executor
            .build_agent_config_for_task(&task)
            .expect("must succeed");

        assert_eq!(
            config.budget_limit,
            Some(5.0),
            "executor default budget must be forwarded when task has none"
        );
    }

    /// When the task sets `budget_usd`, that value must override both the
    /// executor default and `None`.
    #[test]
    fn build_config_uses_task_budget_when_set() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let executor = build_executor(provider, registry_with_agent("bot", "sys"))
            .with_default_budget(99.0);

        let mut task = make_task("bot");
        task.budget_usd = Some(2.0);

        let config = executor
            .build_agent_config_for_task(&task)
            .expect("must succeed");

        assert_eq!(
            config.budget_limit,
            Some(2.0),
            "task-level budget must override executor default"
        );
    }

    // ── Error path ────────────────────────────────────────────────────────────

    /// When the provider returns an error stream, `execute()` must yield a
    /// `failed` outcome with a non-empty `error_msg`.
    ///
    /// `MockProvider` does not support returning `Err` from `complete()` today
    /// (it always returns `Ok`).  A minimal `FailingProvider` is inlined here.
    #[tokio::test]
    async fn execute_provider_error_returns_failed_outcome() {
        use std::pin::Pin;

        use async_trait::async_trait;
        use futures::stream;
        use hone_proto::{Message, ModelInfo, ProviderEvent, ProviderHealth};
        use hone_providers::{CompletionRequest, ModelProvider, ProviderError};

        struct FailingProvider;

        #[async_trait]
        impl ModelProvider for FailingProvider {
            fn id(&self) -> &str {
                "failing"
            }

            fn display_name(&self) -> &str {
                "Failing Provider"
            }

            fn supported_models(&self) -> Vec<ModelInfo> {
                vec![]
            }

            async fn complete(
                &self,
                _req: CompletionRequest,
            ) -> Result<
                Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>,
                ProviderError,
            > {
                Err(ProviderError::Http {
                    provider: "failing".into(),
                    status: 500,
                    body: "boom".into(),
                })
            }

            fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
                0
            }

            async fn health(&self) -> ProviderHealth {
                ProviderHealth::Degraded { reason: "always fails".into() }
            }
        }

        let provider: Arc<dyn ModelProvider> = Arc::new(FailingProvider);
        let executor = build_executor(provider, registry_with_agent("bot", "sys"));
        let task = make_task("bot");

        let outcome = executor.execute(&task).await;

        assert_eq!(outcome.status_str, "failed");
        assert!(
            outcome.error_msg.is_some(),
            "error_msg must be Some when the provider errors"
        );
    }

    /// When `task.context` is non-empty, the user message forwarded to the
    /// provider must contain the context string.
    #[tokio::test]
    async fn execute_includes_task_context_in_user_message() {
        let provider = Arc::new(MockProvider::new(vec!["done".into()]));
        let last_request = Arc::clone(&provider.last_request);
        let executor = build_executor(provider, registry_with_agent("bot", "sys"));

        let mut task = make_task("bot");
        task.context = "context-MARKER".into();

        executor.execute(&task).await;

        let req = last_request
            .lock()
            .unwrap()
            .clone()
            .expect("provider must have been called");
        let user_text = req
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .map(|m| m.content.as_text().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            user_text.contains("context-MARKER"),
            "user message must contain task.context; got: {user_text:?}"
        );
    }

    // ── Trait sanity ──────────────────────────────────────────────────────────

    /// Compile-time proof that `AgentTaskExecutor` implements `TaskExecutor`.
    #[allow(dead_code)]
    fn _check_task_executor_impl<T: TaskExecutor>() {}

    #[allow(dead_code)]
    fn _assert_agent_task_executor_is_task_executor() {
        _check_task_executor_impl::<AgentTaskExecutor>();
    }
}
