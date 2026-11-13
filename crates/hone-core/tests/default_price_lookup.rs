#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U5 of `docs/plans/usage-and-pricing-spec.md`: a process-wide default price lookup that every
//! agent uses unless it has its own, so orchestrator leads and workers and ensemble candidates
//! and rankers are priced per model like the session agent.
//!
//! API (new): `hone_core::set_default_price_lookup(lookup: PriceLookup)`, called once at start
//! (the CLI, `honed`, the server and `hone eval` do it). An agent prices with its own lookup
//! (`AgentHandle::set_price_lookup`), else the default, else `AgentConfig.pricing`.
//!
//! The default is process-wide, so this binary sets it once (every test calls `init`) and
//! nothing here relies on the "no default" behaviour.

use std::sync::{Arc, Once};

use hone_core::{
    AgentConfig, AgentEnsemble, AgentHandle, AgentMode, AgentRegistry, Orchestrator, PriceLookup,
    RetryConfig, WorkerStatus, set_default_price_lookup,
};
use hone_proto::{AgentEvent, ModelId, PricingEntry, TokenUsage, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

fn entry(model: &str, input_per_million: f64) -> PricingEntry {
    PricingEntry {
        model: ModelId::new(model),
        input_per_million,
        output_per_million: 0.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    }
}

/// mock:priced costs $10 per 1M input tokens, mock:cheap $1; anything else has no price.
fn lookup() -> PriceLookup {
    Arc::new(|m: &ModelId| match m.as_str() {
        "mock:priced" => Some(entry("mock:priced", 10.0)),
        "mock:cheap" => Some(entry("mock:cheap", 1.0)),
        _ => None,
    })
}

static INIT: Once = Once::new();
fn init() {
    INIT.call_once(|| set_default_price_lookup(lookup()));
}

/// Every call reports 1M input tokens. `text` is what the model says (the orchestrator tests
/// make it a one-subtask plan so a worker runs).
fn provider(text: &str) -> Arc<MockProvider> {
    Arc::new(
        MockProvider::new(vec![text.to_owned()]).with_usage(TokenUsage {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        }),
    )
}

fn config(provider: Arc<MockProvider>, model: &str) -> AgentConfig {
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
        // A placeholder that must NOT be what prices the calls.
        pricing: entry(model, 0.0),
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

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

async fn one_turn(h: &AgentHandle) -> f64 {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let r = h.run_turn("hello", tx).await.expect("turn");
    while rx.try_recv().is_ok() {}
    r.cost_usd
}

#[tokio::test]
async fn test_u5_an_agent_without_its_own_lookup_should_use_the_process_default() {
    init();
    let h = AgentHandle::spawn(config(provider("hi"), "mock:priced"));

    assert!(close(one_turn(&h).await, 10.0), "the default lookup prices the call, not config.pricing (0)");
}

#[tokio::test]
async fn test_u5_an_agents_own_lookup_should_win_over_the_default() {
    init();
    let h = AgentHandle::spawn(config(provider("hi"), "mock:priced"));
    h.set_price_lookup(Arc::new(|_m: &ModelId| Some(entry("own", 2.0)))).await;

    assert!(close(one_turn(&h).await, 2.0));
}

#[tokio::test]
async fn test_u5_a_model_the_default_does_not_know_should_be_unpriced() {
    init();
    let h = AgentHandle::spawn(config(provider("hi"), "ollama:llama3"));

    assert_eq!(one_turn(&h).await, 0.0);
    assert_eq!(h.unpriced_models().await, ["ollama:llama3"]);
}

#[tokio::test]
async fn test_u5_an_orchestrated_run_should_price_the_lead_and_every_worker() {
    init();
    let plan = r#"{"goal":"test","subtasks":[{"id":"1","description":"do thing","agent":"debugger","context":"","depends_on":[]}]}"#;
    let mut registry = AgentRegistry::new();
    registry.load_builtins();
    let mut orchestrator = Orchestrator::new(config(provider(plan), "mock:priced"), registry, 4);
    let (tx, _rx) = mpsc::channel(256);

    let result = orchestrator.run("do thing", tx).await.expect("orchestration");

    assert!(!result.workers.is_empty());
    for w in &result.workers {
        assert_eq!(w.status, WorkerStatus::Completed);
        assert!(w.cost_usd >= 10.0 - 1e-9, "worker {} cost {}", w.task_id, w.cost_usd);
    }
    let workers: f64 = result.workers.iter().map(|w| w.cost_usd).sum();
    assert!(
        result.total_cost_usd >= workers + 20.0 - 1e-9,
        "the lead's decompose and merge calls ($10 each) are in the total too: {} vs workers {workers}",
        result.total_cost_usd
    );
}

#[tokio::test]
async fn test_u5_ensemble_candidates_and_the_ranker_should_be_priced() {
    init();
    let ensemble = AgentEnsemble::new(config(provider("alpha"), "mock:priced"), vec![0.0, 0.5]);
    let (tx, _rx) = mpsc::channel(256);

    let result = ensemble.run("question", tx).await.expect("ensemble");

    assert_eq!(result.all_candidates.len(), 2);
    for c in &result.all_candidates {
        assert!(close(c.cost_usd, 10.0), "candidate cost {}", c.cost_usd);
    }
    assert!(close(result.total_cost_usd, 30.0), "two candidates + the ranker: {}", result.total_cost_usd);
}
