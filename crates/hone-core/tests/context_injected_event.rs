#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! K5 of `docs/plans/context-phase-events-spec.md`: `AgentEvent::ContextInjected`
//! is sent once when turn-1 injection adds a block, before the first `Token`.
//!
//! API required (new): `SkeletonStrategy::as_str(&self) -> &'static str`, the CLI
//! spelling: `flat`, `dependency_graph`, `call_graph`, `hybrid`, `adaptive`. It
//! is the inverse of `SkeletonStrategy::from_str`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig, SkeletonStrategy};
use hone_index::SqliteIndex;
use hone_proto::{AgentEvent, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

#[test]
fn test_k5_skeleton_strategy_should_have_its_cli_spelling() {
    let table = [
        (SkeletonStrategy::Flat, "flat"),
        (SkeletonStrategy::DependencyGraph, "dependency_graph"),
        (SkeletonStrategy::CallGraph, "call_graph"),
        (SkeletonStrategy::Hybrid, "hybrid"),
        (SkeletonStrategy::Adaptive, "adaptive"),
    ];
    for (strategy, spelling) in table {
        assert_eq!(strategy.as_str(), spelling);
        assert_eq!(SkeletonStrategy::from_str(spelling), Some(strategy));
    }
}

struct Setup {
    agent: Agent,
    _dir: tempfile::TempDir,
}

/// A working directory with a few Rust files, and an index holding `login`.
async fn setup(inject: bool, strategy: SkeletonStrategy, with_index: bool) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/auth.rs"),
        "pub fn login(user: &str) -> bool { true }\npub fn logout() {}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    let index = if with_index {
        let idx = SqliteIndex::in_memory().await.unwrap();
        idx.index_file(&PathBuf::from("src/auth.rs"), "pub fn login() {}\n")
            .await
            .unwrap();
        Some(Arc::new(idx))
    } else {
        None
    };

    let agent = Agent::new(AgentConfig {
        provider: Arc::new(MockProvider::new(vec![
            "ok".to_owned(),
            "ok again".to_owned(),
        ])),
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
        working_dir: dir.path().to_path_buf(),
        memory_store: None,
        retry_config: RetryConfig::default(),
        index,
        inject_context: inject,
        skeleton_strategy: strategy,
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    });
    Setup { agent, _dir: dir }
}

async fn turn(agent: &mut Agent, input: &str) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    tokio::time::timeout(Duration::from_secs(20), agent.run_turn(input, tx))
        .await
        .expect("turn hung")
        .expect("turn failed");
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn injected(events: &[AgentEvent]) -> Vec<&AgentEvent> {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ContextInjected { .. }))
        .collect()
}

#[tokio::test]
async fn test_k5_first_turn_should_send_one_context_injected_before_the_first_token() {
    let mut s = setup(true, SkeletonStrategy::Flat, true).await;

    let events = turn(&mut s.agent, "fix the login").await;

    let all = injected(&events);
    assert_eq!(all.len(), 1, "exactly one ContextInjected: {events:?}");
    let AgentEvent::ContextInjected {
        strategy,
        tokens_used,
        bm25_hits,
        confidence,
    } = all[0]
    else {
        unreachable!()
    };
    assert_eq!(strategy, "flat");
    assert!(*tokens_used > 0);
    assert_eq!(confidence, "High", "one clear index hit");
    assert!(!bm25_hits.is_empty(), "tier 2 fired");
    assert!(
        bm25_hits[0].0.ends_with("auth.rs"),
        "hits are (path, score) in order: {bm25_hits:?}"
    );

    let pos_injected = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ContextInjected { .. }))
        .unwrap();
    let pos_token = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Token { .. }))
        .expect("the model answered");
    assert!(pos_injected < pos_token, "sent before the first Token");
}

#[tokio::test]
async fn test_k5_strategy_field_should_use_the_configured_strategys_cli_spelling() {
    for (strategy, spelling) in [
        (SkeletonStrategy::Hybrid, "hybrid"),
        (SkeletonStrategy::CallGraph, "call_graph"),
        (SkeletonStrategy::Adaptive, "adaptive"),
    ] {
        let mut s = setup(true, strategy, true).await;
        let events = turn(&mut s.agent, "fix the login").await;
        let all = injected(&events);
        assert_eq!(all.len(), 1, "{spelling}");
        let AgentEvent::ContextInjected { strategy, .. } = all[0] else {
            unreachable!()
        };
        assert_eq!(strategy, spelling);
    }
}

#[tokio::test]
async fn test_k5_without_an_index_tier_two_should_not_fire() {
    let mut s = setup(true, SkeletonStrategy::Flat, false).await;

    let events = turn(&mut s.agent, "fix the login").await;

    let all = injected(&events);
    assert_eq!(all.len(), 1, "the skeleton block is still non-empty");
    let AgentEvent::ContextInjected {
        bm25_hits,
        confidence,
        ..
    } = all[0]
    else {
        unreachable!()
    };
    assert!(bm25_hits.is_empty());
    assert_eq!(confidence, "Low");
}

#[tokio::test]
async fn test_k5_second_turn_should_send_none() {
    let mut s = setup(true, SkeletonStrategy::Flat, true).await;
    let first = turn(&mut s.agent, "fix the login").await;
    assert_eq!(injected(&first).len(), 1);

    let second = turn(&mut s.agent, "and the logout").await;

    assert!(injected(&second).is_empty(), "{second:?}");
}

#[tokio::test]
async fn test_k5_injection_off_should_send_none() {
    let mut s = setup(false, SkeletonStrategy::Flat, true).await;

    let events = turn(&mut s.agent, "fix the login").await;

    assert!(injected(&events).is_empty(), "{events:?}");
}
