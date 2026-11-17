#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Contract (spec E2): `set_model` re-targets the context budget. Existing
//! public API only, so these run (and fail on behaviour) before E1/E3 land.
//! The mock-provider harness is the one from `context_window_override_env.rs`.

use std::sync::Arc;

use hone_core::{AgentConfig, AgentHandle, AgentMode, RetryConfig, estimate_text_tokens};
use hone_proto::{Message, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

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
async fn test_set_model_when_switching_to_200k_model_should_send_request_within_120k() {
    let (h, provider) = spawn("anthropic:claude-opus-4-7");
    h.load_history(history(50)).await;
    assert!(
        total_tokens(&h.get_messages().await) > 120_000,
        "precondition"
    );

    h.set_model(ModelId::new("anthropic:claude-opus-4-5")).await;
    let (tx, _rx) = mpsc::channel(64);
    h.run_turn("next question", tx).await.expect("turn");

    let req = provider
        .last_request
        .lock()
        .unwrap()
        .clone()
        .expect("request");
    let sent = total_tokens(&req.messages);
    assert!(sent <= 120_000, "request carried {sent} tokens (> 120K)");
}

#[tokio::test]
async fn test_set_model_when_switching_to_haiku_5_5_should_trim_to_60k() {
    let (h, _p) = spawn("anthropic:claude-opus-4-7");
    h.load_history(history(50)).await;
    h.set_model(ModelId::new("anthropic:claude-haiku-5-5"))
        .await;
    let kept = total_tokens(&h.get_messages().await);
    assert!(kept <= 60_000, "kept {kept} tokens (> 60K)");
}

#[tokio::test]
async fn test_set_model_when_switching_back_to_1m_model_should_restore_600k_budget() {
    let (h, _p) = spawn("anthropic:claude-haiku-5-5");
    h.set_model(ModelId::new("anthropic:claude-opus-4-7")).await;
    h.load_history(history(50)).await;
    let kept = total_tokens(&h.get_messages().await);
    assert!(
        kept > 120_000,
        "kept only {kept} tokens; 600K budget expected"
    );
}
