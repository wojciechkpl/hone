//! Contract tests for `AgentHandle::set_budget_limit` and
//! `AgentHandle::set_approval_policy` (sub-slice 4.5a — red phase).
//!
//! These tests are the specification.  The rust-expert implementer reads them
//! to understand the required API surface and MUST NOT modify this file.
//!
//! All tests use Option A observability: getter methods (`budget_limit()` and
//! `approval_policy()`) that round-trip through the actor via oneshot channels,
//! mirroring the existing `GetCost` pattern.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use hone_core::{AgentConfig, AgentError, AgentHandle, RetryConfig};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy, TokenUsage};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Zero-cost pricing so `run_turn` never accrues unexpected spend.
fn zero_pricing() -> PricingEntry {
    PricingEntry {
        model: ModelId::new("mock:test"),
        input_per_million: 0.0,
        output_per_million: 0.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    }
}

/// Pricing that makes every token cost real money so a tiny budget is quickly
/// consumed: $3/M input, $15/M output.
fn real_pricing() -> PricingEntry {
    PricingEntry {
        model: ModelId::new("mock:test"),
        input_per_million: 3.0,
        output_per_million: 15.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    }
}

fn base_config(provider: Arc<MockProvider>) -> AgentConfig {
    AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: zero_pricing(),
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: hone_core::AgentMode::Build,
    }
}

// ---------------------------------------------------------------------------
// Test 1 — set_budget_limit(Some(…)) is reflected by the getter
// ---------------------------------------------------------------------------

/// Setting a `Some` budget limit on a handle that started with `None` must be
/// visible via `budget_limit().await` immediately after the setter awaits.
#[tokio::test]
async fn set_budget_limit_some_then_get_returns_value() {
    // Given — handle with no budget limit
    let provider = Arc::new(MockProvider::new(vec![]));
    let handle = AgentHandle::spawn(base_config(provider));

    // When
    handle.set_budget_limit(Some(0.5)).await;

    // Then
    assert_eq!(handle.budget_limit().await, Some(0.5));
}

// ---------------------------------------------------------------------------
// Test 2 — set_budget_limit(None) clears an existing value
// ---------------------------------------------------------------------------

/// Passing `None` to `set_budget_limit` must clear a previously set budget so
/// the getter returns `None`.
#[tokio::test]
async fn set_budget_limit_none_clears_existing_value() {
    // Given — handle pre-configured with a budget
    let provider = Arc::new(MockProvider::new(vec![]));
    let mut config = base_config(provider);
    config.budget_limit = Some(1.0);
    let handle = AgentHandle::spawn(config);

    // Confirm the initial value is visible through the getter
    assert_eq!(handle.budget_limit().await, Some(1.0));

    // When
    handle.set_budget_limit(None).await;

    // Then
    assert_eq!(handle.budget_limit().await, None);
}

// ---------------------------------------------------------------------------
// Test 3 — set_approval_policy swaps to Auto
// ---------------------------------------------------------------------------

/// After calling `set_approval_policy(Auto)` on a handle whose initial policy
/// is `AskRisky`, the getter must return `Auto`.
#[tokio::test]
async fn set_approval_policy_swaps_to_auto() {
    // Given — default AskRisky policy (set in base_config)
    let provider = Arc::new(MockProvider::new(vec![]));
    let handle = AgentHandle::spawn(base_config(provider));

    // When
    handle
        .set_approval_policy(ToolApprovalPolicy::Auto)
        .await;

    // Then
    assert_eq!(handle.approval_policy().await, ToolApprovalPolicy::Auto);
}

// ---------------------------------------------------------------------------
// Test 4 — set_approval_policy preserves AskRisky allowlist entries
// ---------------------------------------------------------------------------

/// The allowlist entries inside an `AskRisky` policy must survive the round-
/// trip through the actor unchanged.
#[tokio::test]
async fn set_approval_policy_to_ask_risky_with_allowlist_preserves_entries() {
    // Given
    let provider = Arc::new(MockProvider::new(vec![]));
    let handle = AgentHandle::spawn(base_config(provider));

    let policy = ToolApprovalPolicy::AskRisky {
        always_allowed: vec!["shell".to_owned()],
    };

    // When
    handle.set_approval_policy(policy.clone()).await;

    // Then
    assert_eq!(handle.approval_policy().await, policy);
}

// ---------------------------------------------------------------------------
// Test 5 — set_budget_limit takes effect for subsequent run_turn calls
// ---------------------------------------------------------------------------

/// After running one turn that accumulates cost, reducing the budget below the
/// accumulated cost via `set_budget_limit` must cause the *next* `run_turn` to
/// return `AgentError::BudgetExceeded`.
///
/// The budget check in `Agent::run_turn` fires at the very start of the turn
/// (`agent.rs:477`), so the sequence is:
///   1. Turn 1 — budget is None → passes, cost accumulates.
///   2. `set_budget_limit(Some(tiny))` — new limit is below current cost.
///   3. Turn 2 — budget check fires → `BudgetExceeded`.
///
/// We use `real_pricing()` with a `TokenUsage` emission so turn 1 genuinely
/// accrues cost, and `budget_limit = Some(0.0)` as the tiny value (zero means
/// any positive accumulated cost exceeds it).
#[tokio::test]
async fn set_budget_limit_takes_effect_in_run_turn() {
    // Usage event that turn 1 will emit: 100 input + 100 output tokens.
    // At $3/M input + $15/M output = $0.0003 + $0.0015 = $0.0018 USD.
    let usage = TokenUsage {
        input: 100,
        output: 100,
        cache_read: 0,
        cache_creation: 0,
    };

    let provider = Arc::new(
        MockProvider::new(vec!["hello".to_owned()]).with_usage(usage),
    );

    let mut config = base_config(provider);
    config.pricing = real_pricing();
    // Start with no budget limit so turn 1 passes.
    config.budget_limit = None;

    let handle = AgentHandle::spawn(config);

    // Turn 1 — must succeed and accumulate cost.
    let (tx1, _rx1) = mpsc::channel(32);
    handle
        .run_turn("first turn", tx1)
        .await
        .expect("first turn should succeed");

    // Set a budget of $0.0 — any accumulated cost exceeds this.
    handle.set_budget_limit(Some(0.0)).await;

    // Turn 2 — the budget check at the top of run_turn must fire.
    let (tx2, _rx2) = mpsc::channel(32);
    let result = handle.run_turn("second turn", tx2).await;

    assert!(
        matches!(result, Err(AgentError::BudgetExceeded { .. })),
        "expected BudgetExceeded after set_budget_limit, got: {result:?}",
    );
}

// ---------------------------------------------------------------------------
// Test 6 — setters are thread-safe via the actor (no data races)
// ---------------------------------------------------------------------------

/// Ten concurrent tasks each call `set_budget_limit(Some(i as f64))`.  After
/// all tasks complete, `budget_limit().await` must return *one* of the set
/// values (the actor's FIFO serialisation means the last command wins, but we
/// cannot predict which one completes last in a concurrent scenario).  The
/// important property is that there are no panics and the value is one of the
/// submitted candidates.
///
/// This test documents the actor-as-mutex pattern: because every write goes
/// through the command channel, no unsafe concurrency is possible.
#[tokio::test]
async fn setters_are_thread_safe_via_actor() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let handle = AgentHandle::spawn(base_config(provider));

    let mut join_set = tokio::task::JoinSet::new();
    for i in 0u32..10 {
        let h = handle.clone();
        join_set.spawn(async move {
            h.set_budget_limit(Some(i as f64)).await;
        });
    }

    // Wait for all concurrent setters to complete.
    while let Some(res) = join_set.join_next().await {
        res.expect("setter task must not panic");
    }

    // The final value must be one of 0.0 … 9.0 — no panic, no corruption.
    let final_value = handle.budget_limit().await;
    let candidates: Vec<Option<f64>> = (0u32..10).map(|i| Some(i as f64)).collect();
    assert!(
        candidates.contains(&final_value),
        "expected one of the 10 set values, got: {final_value:?}",
    );
}
