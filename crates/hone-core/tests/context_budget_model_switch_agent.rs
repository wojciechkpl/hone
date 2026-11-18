#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Contract (spec F1/F2): a model switch never trims the stored history; each
//! request is fitted to the model that serves it. Existing public API only.
//! Mock-provider harness as in `context_window_override_env.rs`.

use std::sync::Arc;

use hone_core::{AgentConfig, AgentHandle, AgentMode, RetryConfig, estimate_text_tokens};
use hone_proto::{Message, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

fn total_tokens(msgs: &[Message]) -> u32 {
    msgs.iter()
        .map(|m| estimate_text_tokens(&m.content.as_text()))
        .sum()
}

/// `n` distinct messages of roughly 6.5k tokens each (alternating roles,
/// starting with a user message).
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

/// About 400K estimated tokens.
const BIG: usize = 62;

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
            long_prompt: None,
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

struct Rig {
    h: AgentHandle,
    provider: Arc<MockProvider>,
}

fn spawn(model: &str) -> Rig {
    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    let h = AgentHandle::spawn(agent_config(model, Arc::clone(&provider)));
    Rig { h, provider }
}

impl Rig {
    async fn switch(&self, model: &str) {
        self.h.set_model(ModelId::new(model)).await;
    }

    /// Run a turn and return the messages the provider received.
    async fn turn(&self) -> Vec<Message> {
        let (tx, _rx) = mpsc::channel(64);
        self.h.run_turn("next question", tx).await.expect("turn");
        self.provider
            .last_request
            .lock()
            .unwrap()
            .clone()
            .expect("request")
            .messages
    }

    async fn stored_tokens(&self) -> u32 {
        total_tokens(&self.h.get_messages().await)
    }
}

// F1: stored history

#[tokio::test]
async fn test_f1_set_model_when_switching_to_smaller_model_should_not_trim_stored_history() {
    for target in ["anthropic:claude-haiku-5-5", "anthropic:claude-opus-4-5"] {
        let r = spawn("anthropic:claude-opus-4-7");
        r.h.load_history(history(BIG)).await;
        let before = r.stored_tokens().await;
        assert!(before > 380_000, "precondition: {before}");
        r.switch(target).await;
        assert_eq!(r.stored_tokens().await, before, "switch to {target}");
    }
}

#[tokio::test]
async fn test_f1_stored_budget_when_model_set_to_larger_later_should_be_largest_seen() {
    // Starts on a 120K model, then is set to a 600K one: the stored budget is 600K.
    let r = spawn("anthropic:claude-opus-4-5");
    r.switch("anthropic:claude-opus-4-7").await;
    r.h.load_history(history(BIG)).await;
    let kept = r.stored_tokens().await;
    assert!(kept > 380_000, "kept {kept}; 600K budget expected");
}

#[tokio::test]
async fn test_f1_stored_budget_when_switched_back_to_small_model_should_stay_largest_seen() {
    // Opus 4.7 (600K) -> Haiku 4.5 (120K): later loads still use 600K.
    let r = spawn("anthropic:claude-opus-4-7");
    r.switch("anthropic:claude-opus-4-5").await;
    r.h.load_history(history(BIG)).await;
    let kept = r.stored_tokens().await;
    assert!(kept > 380_000, "kept {kept}; largest budget seen is 600K");
}

#[tokio::test]
async fn test_f1_round_trip_opus_haiku_5_5_opus_should_restore_full_history_in_request() {
    let r = spawn("anthropic:claude-opus-4-7");
    r.h.load_history(history(BIG)).await;

    r.switch("anthropic:claude-haiku-5-5").await;
    let _ = r.turn().await;

    r.switch("anthropic:claude-opus-4-7").await;
    let req = r.turn().await;
    let sent = total_tokens(&req);
    assert!(
        sent > 380_000,
        "last Opus request carried only {sent} tokens"
    );
    assert!(
        req.len() >= BIG,
        "expected the full history, got {} messages",
        req.len()
    );
}

// F2: each request is fitted to its model; stored history unchanged

#[tokio::test]
async fn test_f2_request_when_current_model_is_haiku_5_5_should_carry_at_most_60k() {
    let r = spawn("anthropic:claude-opus-4-7");
    let h = history(BIG);
    r.h.load_history(h.clone()).await;
    r.switch("anthropic:claude-haiku-5-5").await;

    let req = r.turn().await;
    let sent = total_tokens(&req);
    assert!(sent <= 60_000, "request carried {sent} tokens (> 60K)");
    assert!(sent > 30_000, "request over-trimmed: {sent} tokens");
    assert!(
        req.iter()
            .any(|m| m.content.as_text() == h[0].content.as_text()),
        "first user message must be kept"
    );
    assert!(
        req.iter()
            .any(|m| m.content.as_text() == h[BIG - 1].content.as_text()),
        "most recent stored message must be kept"
    );
}

#[tokio::test]
async fn test_f2_request_when_current_model_is_200k_model_should_carry_at_most_120k() {
    let r = spawn("anthropic:claude-opus-4-7");
    r.h.load_history(history(BIG)).await;
    r.switch("anthropic:claude-opus-4-5").await;

    let sent = total_tokens(&r.turn().await);
    assert!(sent <= 120_000, "request carried {sent} tokens (> 120K)");
    assert!(sent > 60_000, "request over-trimmed: {sent} tokens");
}

#[tokio::test]
async fn test_f2_request_when_trimmed_should_leave_stored_history_unchanged() {
    let r = spawn("anthropic:claude-opus-4-7");
    r.h.load_history(history(BIG)).await;
    let before = r.h.get_messages().await.len();
    r.switch("anthropic:claude-haiku-5-5").await;

    let _ = r.turn().await;
    // The turn adds the user message and the reply; nothing is dropped.
    assert_eq!(r.h.get_messages().await.len(), before + 2);
}
