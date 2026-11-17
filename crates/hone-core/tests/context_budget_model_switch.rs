#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Contract: the context budget follows model switches and fallbacks
//! (spec section E).
//!
//! New public API assumed (the implementer must match these signatures):
//! - `ContextManager::retarget(&mut self, model_name: &str)` (E1; name without
//!   provider prefix, like `ContextConfig::for_model`)
//! - `AgentHandle::set_fallback_models(&self, models: Vec<ModelId>)` (E3;
//!   async, fire-and-forget like `set_model`)
//!
//! E4 (`model_context_limit` in `bins/hone/src/main.rs`) is private to a binary
//! crate and cannot be driven from an integration test; see the report.

use std::sync::Arc;

use hone_core::{
    AgentConfig, AgentHandle, AgentMode, ContextConfig, ContextManager, RetryConfig,
    estimate_text_tokens,
};
use hone_proto::{Message, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;

fn msg_tokens(m: &Message) -> u32 {
    estimate_text_tokens(&m.content.as_text())
}

fn total_tokens(msgs: &[Message]) -> u32 {
    msgs.iter().map(msg_tokens).sum()
}

/// `n` distinct messages of roughly 3k tokens each (alternating roles).
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

// ---------------------------------------------------------------------------
// E1: ContextManager::retarget
// ---------------------------------------------------------------------------

fn manager(model: &str) -> ContextManager {
    ContextManager::with_config(ContextConfig::for_model(model))
}

#[test]
fn test_retarget_when_new_model_should_recompute_budget_as_for_new_agent() {
    let cases = [
        ("claude-opus-4-7", "claude-opus-4-5", 120_000),
        ("claude-opus-4-7", "claude-haiku-5-5", 60_000),
        ("claude-haiku-5-5", "claude-opus-4-7", 600_000),
        ("claude-sonnet-4-5", "claude-sonnet-5", 600_000),
    ];
    for (from, to, want) in cases {
        let mut ctx = manager(from);
        ctx.retarget(to);
        assert_eq!(ctx.max_context_tokens(), want, "{from} -> {to}");
    }
}

#[test]
fn test_retarget_when_user_ratio_set_should_keep_ratio_without_cap() {
    let mut ctx = manager("claude-opus-4-7");
    ctx.set_budget_ratio(0.30);
    ctx.retarget("claude-haiku-5-5");
    assert_eq!(ctx.max_context_tokens(), 300_000, "explicit ratio, no cap");
    ctx.retarget("claude-opus-4-5");
    assert_eq!(ctx.max_context_tokens(), 60_000, "0.30 of 200K");
}

#[test]
fn test_retarget_when_project_cap_set_should_only_lower() {
    let mut ctx = manager("claude-opus-4-7");
    ctx.cap_budget_ratio(0.30);
    ctx.retarget("claude-opus-4-5");
    // min(model ratio 0.60, cap 0.30) of 200K
    assert_eq!(ctx.max_context_tokens(), 60_000);
    ctx.retarget("claude-opus-4-8");
    assert_eq!(ctx.max_context_tokens(), 300_000);
}

#[test]
fn test_retarget_when_budget_shrinks_should_trim_history_at_once() {
    let mut ctx = manager("claude-opus-4-7");
    for m in history(50) {
        ctx.add_message(m);
    }
    assert!(total_tokens(&ctx.messages()) > 120_000, "precondition");
    ctx.retarget("claude-opus-4-5");
    let kept = total_tokens(&ctx.messages());
    assert!(kept <= 120_000, "kept {kept} tokens > 120K after retarget");
    assert!(!ctx.messages().is_empty());
}

#[test]
fn test_retarget_when_budget_grows_should_keep_history() {
    let mut ctx = manager("claude-opus-4-5");
    for m in history(10) {
        ctx.add_message(m);
    }
    let before = ctx.messages().len();
    ctx.retarget("claude-opus-4-7");
    assert_eq!(ctx.messages().len(), before);
}

// ---------------------------------------------------------------------------
// E2/E3: agent harness (mock provider, as in context_window_override_env.rs)
// ---------------------------------------------------------------------------

fn agent_config(model: &str, provider: Arc<MockProvider>) -> AgentConfig {
    AgentConfig {
        provider,
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

fn spawn(model: &str) -> (AgentHandle, Arc<MockProvider>) {
    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    let handle = AgentHandle::spawn(agent_config(model, Arc::clone(&provider)));
    (handle, provider)
}

#[tokio::test]
async fn test_fallback_models_when_smaller_window_should_set_budget_to_minimum() {
    let (h, _p) = spawn("anthropic:claude-opus-4-7");
    h.set_fallback_models(vec![ModelId::new("anthropic:claude-haiku-4-5")])
        .await;
    h.load_history(history(50)).await;
    let kept = total_tokens(&h.get_messages().await);
    assert!(kept <= 120_000, "kept {kept} tokens (> 120K)");
    assert!(
        kept > 60_000,
        "kept {kept}; budget should be 120K, not less"
    );
}

#[tokio::test]
async fn test_fallback_models_when_model_switched_should_keep_fallback_in_minimum() {
    let (h, _p) = spawn("anthropic:claude-opus-4-7");
    h.set_fallback_models(vec![ModelId::new("anthropic:claude-haiku-4-5")])
        .await;
    h.set_model(ModelId::new("anthropic:claude-opus-4-6")).await;
    h.load_history(history(50)).await;
    let kept = total_tokens(&h.get_messages().await);
    assert!(kept <= 120_000, "kept {kept} tokens (> 120K)");
}

#[tokio::test]
async fn test_fallback_models_when_fallback_is_haiku_5_5_should_apply_its_cap() {
    let (h, _p) = spawn("anthropic:claude-opus-4-7");
    h.set_fallback_models(vec![ModelId::new("anthropic:claude-haiku-5-5")])
        .await;
    h.load_history(history(50)).await;
    let kept = total_tokens(&h.get_messages().await);
    assert!(kept <= 60_000, "kept {kept} tokens (> 60K)");
}
