#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Contract: Claude compaction budget (spec sections B, C and D).

use std::sync::Arc;

use hone_core::{Agent, AgentConfig, AgentMode, ContextConfig, ContextManager, RetryConfig};
use hone_proto::{AgentEvent, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

const ONE_M_FULL_BUDGET: &[&str] = &[
    "claude-fable-5-2",
    "claude-mythos-5-2",
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-fable-5",
    "claude-mythos-5",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-5-5",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
];

const LEGACY_200K: &[&str] = &[
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-haiku-4-5",
    "claude-haiku-4-5-20251001",
    // B4: unknown Claude names stay conservative.
    "claude-future-model",
];

fn assert_ratio(cfg: &ContextConfig, want: f32, model: &str) {
    assert!(
        (cfg.effective_budget_ratio - want).abs() < 0.001,
        "model {model}: ratio {} != {want}",
        cfg.effective_budget_ratio
    );
}

// B1
#[test]
fn test_for_model_when_1m_claude_should_use_full_window_at_60_percent() {
    for name in ONE_M_FULL_BUDGET {
        let cfg = ContextConfig::for_model(name);
        assert_eq!(cfg.advertised_context_length, 1_000_000, "model {name}");
        assert_ratio(&cfg, 0.60, name);
        assert_eq!(cfg.effective_budget(), 600_000, "model {name}");
    }
}

// B2
#[test]
fn test_for_model_when_haiku_5_5_should_cap_budget_at_60k() {
    let cfg = ContextConfig::for_model("claude-haiku-5-5");
    assert_eq!(cfg.advertised_context_length, 1_000_000);
    assert_eq!(cfg.effective_budget(), 60_000);
}

// B3, B4
#[test]
fn test_for_model_when_legacy_or_unknown_claude_should_keep_200k_and_120k_budget() {
    for name in LEGACY_200K {
        let cfg = ContextConfig::for_model(name);
        assert_eq!(cfg.advertised_context_length, 200_000, "model {name}");
        assert_eq!(cfg.effective_budget(), 120_000, "model {name}");
    }
}

// B5
#[test]
fn test_for_model_when_non_claude_should_be_unchanged() {
    let cases = [
        ("deepseek-v3", 128_000, 0.45),
        ("llama3.2", 128_000, 0.40),
        ("gemini-2.5-pro", 1_000_000, 0.50),
        ("some-unknown-model", 128_000, 0.50),
    ];
    for (name, window, ratio) in cases {
        let cfg = ContextConfig::for_model(name);
        assert_eq!(cfg.advertised_context_length, window, "model {name}");
        assert_ratio(&cfg, ratio, name);
    }
}

// B6
#[test]
fn test_with_context_window_when_override_on_opus_4_7_should_use_ratio_of_override() {
    let cfg = ContextConfig::for_model("claude-opus-4-7").with_context_window(Some(200_000));
    assert_eq!(cfg.effective_budget(), 120_000);
}

// B6
#[test]
fn test_with_context_window_when_override_on_haiku_5_5_should_keep_cap() {
    let cfg = ContextConfig::for_model("claude-haiku-5-5").with_context_window(Some(200_000));
    assert_eq!(cfg.effective_budget(), 60_000);
}

// B7
#[test]
fn test_for_model_when_claude_should_keep_recent_window_and_compression_settings() {
    let baseline = ContextConfig::for_model("claude-sonnet-4-5");
    for name in ONE_M_FULL_BUDGET
        .iter()
        .chain(["claude-haiku-5-5"].iter())
        .chain(LEGACY_200K.iter())
    {
        let cfg = ContextConfig::for_model(name);
        assert_eq!(cfg.recent_window, 3, "model {name}");
        assert!(!cfg.reorder_middle_zone, "model {name}");
        assert_eq!(
            cfg.tool_compress_age, baseline.tool_compress_age,
            "model {name}"
        );
        assert_eq!(
            cfg.tool_oneliner_age_offset, baseline.tool_oneliner_age_offset,
            "model {name}"
        );
    }
}

fn manager(model: &str) -> ContextManager {
    ContextManager::with_config(ContextConfig::for_model(model))
}

// C1
#[test]
fn test_context_manager_when_opus_4_7_should_budget_600k() {
    assert_eq!(manager("claude-opus-4-7").max_context_tokens(), 600_000);
}

// C2
#[test]
fn test_context_manager_when_haiku_5_5_should_budget_60k() {
    assert_eq!(manager("claude-haiku-5-5").max_context_tokens(), 60_000);
}

// C3
#[test]
fn test_set_budget_ratio_when_haiku_5_5_should_drop_default_cap() {
    let mut ctx = manager("claude-haiku-5-5");
    ctx.set_budget_ratio(0.30);
    assert_eq!(ctx.max_context_tokens(), 300_000);
}

// C4
#[test]
fn test_cap_budget_ratio_when_haiku_5_5_and_cap_above_budget_should_keep_60k() {
    let mut ctx = manager("claude-haiku-5-5");
    ctx.cap_budget_ratio(0.30);
    assert_eq!(ctx.max_context_tokens(), 60_000);
}

// C4
#[test]
fn test_cap_budget_ratio_when_haiku_5_5_and_cap_below_budget_should_lower_to_30k() {
    let mut ctx = manager("claude-haiku-5-5");
    ctx.cap_budget_ratio(0.03);
    assert_eq!(ctx.max_context_tokens(), 30_000);
}

// C5
#[test]
fn test_cap_budget_ratio_when_opus_4_7_should_lower_to_300k() {
    let mut ctx = manager("claude-opus-4-7");
    ctx.cap_budget_ratio(0.30);
    assert_eq!(ctx.max_context_tokens(), 300_000);
}

// D1: reuses the agent + MockProvider harness of context_window_override_env.rs.
fn agent_config(model: &str) -> AgentConfig {
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

#[tokio::test]
async fn test_agent_when_model_is_opus_4_7_should_report_1m_in_model_info() {
    let mut agent = Agent::new(agent_config("anthropic:claude-opus-4-7"));
    let (tx, mut rx) = mpsc::channel(64);
    agent.run_turn("hi", tx).await.expect("turn should succeed");
    let mut first = None;
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::ModelInfo { context_window } = ev {
            first.get_or_insert(context_window);
        }
    }
    assert_eq!(first, Some(1_000_000));
}
