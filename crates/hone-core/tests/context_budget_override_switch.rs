#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Spec E1: `HONE_CONTEXT_WINDOW` replaces the window for every model, also
//! after `set_model`. Own test binary with ONE test so nothing races on the
//! variable; the guard restores it even if an assertion panics.

use std::sync::Arc;

use hone_core::{AgentConfig, AgentHandle, AgentMode, RetryConfig, estimate_text_tokens};
use hone_proto::{Message, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;

const VAR: &str = "HONE_CONTEXT_WINDOW";

struct EnvGuard;

impl EnvGuard {
    fn set(value: &str) -> Self {
        // SAFETY: this binary has a single test touching the variable, and it is
        // set before the agent (and any task it spawns) is created.
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

fn config(model: &str) -> AgentConfig {
    AgentConfig {
        provider: Arc::new(MockProvider::new(vec!["ok".to_owned()])),
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new(model),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new(model),
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

/// `n` distinct messages of roughly 6k tokens each.
fn history(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| {
            let text: String = (0..1500).map(|j| format!("w{i}x{j} ")).collect();
            if i % 2 == 0 {
                Message::user(text)
            } else {
                Message::assistant(text)
            }
        })
        .collect()
}

#[tokio::test]
async fn test_set_model_when_env_override_set_should_apply_override_to_new_model() {
    let _guard = EnvGuard::set("300000");
    let h = AgentHandle::spawn(config("anthropic:claude-opus-4-7"));

    h.set_model(ModelId::new("anthropic:claude-haiku-4-5"))
        .await;
    h.load_history(history(50)).await; // ~325K tokens

    let kept: u32 = h
        .get_messages()
        .await
        .iter()
        .map(|m| estimate_text_tokens(&m.content.as_text()))
        .sum();
    // 0.6 x 300K = 180K: above Haiku 4.5's own 120K budget, below 300K.
    assert!(kept <= 180_000, "kept {kept} tokens (> 180K)");
    assert!(
        kept > 120_000,
        "kept {kept} tokens; override should win over Haiku 4.5's own 200K window"
    );
}
