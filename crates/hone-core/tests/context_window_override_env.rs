//! End-to-end tests that need the REAL `HONE_CONTEXT_WINDOW` env var.
//!
//! Own test binary (separate process) so nothing else races on the variable.
//! Everything that touches the variable lives in ONE test function, and the
//! variable is restored by a drop guard even if an assertion panics.

use std::sync::Arc;

use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig};
use hone_proto::{AgentEvent, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

const VAR: &str = "HONE_CONTEXT_WINDOW";

/// Sets the env var on creation; removes it on drop (panic-safe).
struct EnvGuard;

impl EnvGuard {
    fn set(value: &str) -> Self {
        // SAFETY: `set_var` is unsafe because any concurrent access to the
        // process environment (including from C code or other threads) is
        // undefined behaviour. This binary has a single test that touches the
        // variable, and it is set before the agent (and any task it spawns)
        // is created.
        unsafe { std::env::set_var(VAR, value) };
        EnvGuard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: see `EnvGuard::set`.
        unsafe { std::env::remove_var(VAR) };
    }
}

fn agent_config() -> AgentConfig {
    AgentConfig {
        provider: Arc::new(MockProvider::new(vec!["ok".to_owned()])),
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: std::path::PathBuf::from("."),
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
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    }
}

/// A prompt of roughly 6-10k tokens: far over 95% of a 4096-token budget,
/// far under the 64k budget an unknown model gets without an override.
fn big_prompt() -> String {
    (0..3000).map(|i| format!("word{i} ")).collect()
}

const NEARLY_FULL: &str = "CONTEXT NEARLY FULL";

async fn run(prompt: &str) -> (Vec<String>, Option<u32>) {
    let mut agent = Agent::new(agent_config());
    let (tx, mut rx) = mpsc::channel(64);
    agent
        .run_turn(prompt, tx)
        .await
        .expect("turn should succeed");
    let mut model_info = None;
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::ModelInfo { context_window } = ev {
            model_info = Some(context_window);
        }
    }
    let texts = agent.messages().iter().map(|m| format!("{m:?}")).collect();
    (texts, model_info)
}

#[tokio::test]
async fn test_agent_when_env_override_set_should_use_it_for_budget_and_model_info() {
    // Baseline, no override: unknown model => 128k * 0.5 budget, so the big
    // prompt must NOT trip the near-full path.
    let (msgs, _) = run(&big_prompt()).await;
    assert!(
        !msgs.iter().any(|m| m.contains(NEARLY_FULL)),
        "baseline must not hit the near-full path"
    );

    // Override 8192 => 4096-token budget (ratio 0.5): the same prompt is now
    // over 95% of budget, so the agent injects the near-full notice. This
    // fails if Agent::new ignores the override for its ContextConfig.
    let _guard = EnvGuard::set("8192");
    let (msgs, _) = run(&big_prompt()).await;
    assert!(
        msgs.iter().any(|m| m.contains(NEARLY_FULL)),
        "override must shrink the agent's real budget"
    );

    // ModelInfo reports the override (small prompt, so unrelated to budget).
    let (_, model_info) = run("hi").await;
    assert_eq!(model_info, Some(8192));
}
