use std::sync::Arc;

use hone_core::{Agent, AgentConfig, AgentError};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy, TokenUsage};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

/// Pricing that makes each token cost real money so budget is quickly consumed.
/// Input: $3/M, Output: $15/M.
fn real_pricing() -> PricingEntry {
    PricingEntry {
        model: ModelId::new("mock:test"),
        input_per_million: 3.0,
        output_per_million: 15.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    }
}

/// A very tight budget — $0.001 USD.
const TIGHT_BUDGET: f64 = 0.001;

#[tokio::test]
async fn test_budget_cap_blocks_new_turn() {
    // The first turn emits a usage event that exceeds the budget.
    // 100 input tokens @ $3/M = $0.0003
    // 100 output tokens @ $15/M = $0.0015
    // Total = $0.0018 which is > $0.001
    let usage = TokenUsage {
        input: 100,
        output: 100,
        cache_read: 0,
        cache_creation: 0,
    };
    let provider = Arc::new(MockProvider::new(vec!["hello".to_owned()]).with_usage(usage));

    let config = AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "You are a helpful assistant.".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: Some(TIGHT_BUDGET),
        pricing: real_pricing(),
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
    };

    let mut agent = Agent::new(config);

    // First turn should succeed — budget check at start passes (cost is 0).
    let (tx1, _rx1) = mpsc::channel(32);
    agent
        .run_turn("first turn", tx1)
        .await
        .expect("first turn should succeed");

    // After the first turn the accumulated cost exceeds the budget.
    assert!(agent.session_cost() > TIGHT_BUDGET);

    // Second turn must be blocked by the budget check at turn start.
    let (tx2, _rx2) = mpsc::channel(32);
    let result = agent.run_turn("second turn", tx2).await;

    assert!(
        matches!(result, Err(AgentError::BudgetExceeded { .. })),
        "expected BudgetExceeded, got: {:?}",
        result
    );
}
