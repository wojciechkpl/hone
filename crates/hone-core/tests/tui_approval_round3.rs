#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Red-phase tests for `docs/plans/tui-approval-fix-spec.md` section 8: R12 and R13.
//!
//! R12: once the turn's event receiver is closed, calls that need no prompt must not run
//! either (checked before each sequential call and before a parallel batch); the rest of the
//! batch gets "cancelled" tool results and `run_turn` returns `Cancelled`.
//! R13: a `run_turn` / `run_turn_content` whose receiver is already closed returns `Cancelled`
//! before touching state: history is unchanged and the provider is not called.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, AgentError, AgentHandle, AgentMode, RetryConfig};
use hone_proto::{
    AgentEvent, Content, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

/// Counts calls. If `gate` is set, signals `started` and waits for `release` before returning.
struct T {
    name: &'static str,
    calls: Arc<AtomicUsize>,
    gate: Option<(Arc<Notify>, Arc<Notify>)>,
}

#[async_trait]
impl Tool for T {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "t"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((started, release)) = &self.gate {
            started.notify_one();
            release.notified().await;
        }
        Ok(ToolOutput::new(json!({ "ok": true })))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        false
    }
}

struct Script {
    calls: Vec<ToolCallRequest>,
    n: AtomicUsize,
}

#[async_trait]
impl ModelProvider for Script {
    fn id(&self) -> &str {
        "script"
    }
    fn display_name(&self) -> &str {
        "Script"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Test".to_owned(),
            context_length: 4096,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let events = if i == 0 && !self.calls.is_empty() {
            let mut v: Vec<_> = self.calls.iter().cloned().map(ProviderEvent::ToolCall).collect();
            v.push(ProviderEvent::Done);
            v
        } else {
            vec![
                ProviderEvent::Token {
                    text: "done".to_owned(),
                },
                ProviderEvent::Done,
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn call(id: &str, name: &str) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: json!({}),
    }
}

fn config(provider: Arc<Script>, tools: Vec<T>, mode: AgentMode) -> AgentConfig {
    let mut b = ToolRegistry::builder();
    for t in tools {
        b = b.register(Box::new(t));
    }
    AgentConfig {
        provider,
        tools: b.build(),
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
        working_dir: std::env::temp_dir(),
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
        mode,
        policy: None,
    }
}

// -- R12 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_r12_auto_approve_batch_should_stop_after_receiver_closes_and_cancel_the_rest() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let c1 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));
    let c3 = Arc::new(AtomicUsize::new(0));
    let tools = vec![
        T {
            name: "w1",
            calls: Arc::clone(&c1),
            gate: Some((Arc::clone(&started), Arc::clone(&release))),
        },
        T {
            name: "w2",
            calls: Arc::clone(&c2),
            gate: None,
        },
        T {
            name: "w3",
            calls: Arc::clone(&c3),
            gate: None,
        },
    ];
    let prov = Arc::new(Script {
        calls: vec![call("c1", "w1"), call("c2", "w2"), call("c3", "w3")],
        n: AtomicUsize::new(0),
    });
    let handle = AgentHandle::spawn(config(prov, tools, AgentMode::AutoApprove));

    let (tx, rx) = mpsc::channel::<AgentEvent>(256);
    let h = handle.clone();
    let turn = tokio::spawn(async move { h.run_turn("go", tx).await });

    // Call 1 is executing: the user hits Esc now (receiver dropped), then call 1 finishes.
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("first write never started");
    drop(rx);
    release.notify_one();

    let res = tokio::time::timeout(Duration::from_secs(10), turn)
        .await
        .expect("turn hung")
        .unwrap();
    assert!(matches!(res, Err(AgentError::Cancelled)), "got {res:?}");
    assert_eq!(c1.load(Ordering::SeqCst), 1);
    assert_eq!(c2.load(Ordering::SeqCst), 0, "call 2 ran after Esc");
    assert_eq!(c3.load(Ordering::SeqCst), 0, "call 3 ran after Esc");

    let msgs = handle.get_messages().await;
    for id in ["c2", "c3"] {
        let m = msgs
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no tool result for {id}: {msgs:?}"));
        assert!(m.content.as_text().contains("cancelled"), "{id}");
    }
}

// -- R13 ----------------------------------------------------------------------------------------

async fn history_snapshot(handle: &AgentHandle) -> Vec<String> {
    handle
        .get_messages()
        .await
        .iter()
        .map(|m| format!("{m:?}"))
        .collect()
}

#[tokio::test]
async fn test_r13_run_turn_with_closed_receiver_should_cancel_and_leave_history_unchanged() {
    let prov = Arc::new(Script {
        calls: vec![],
        n: AtomicUsize::new(0),
    });
    let handle = AgentHandle::spawn(config(Arc::clone(&prov), vec![], AgentMode::Build));
    handle
        .load_history(vec![Message::user("earlier"), Message::assistant("reply")])
        .await;
    let before = history_snapshot(&handle).await;
    assert_eq!(before.len(), 2);

    let (tx, rx) = mpsc::channel::<AgentEvent>(8);
    drop(rx);
    let res = tokio::time::timeout(Duration::from_secs(10), handle.run_turn("new prompt", tx))
        .await
        .expect("hung");

    assert!(matches!(res, Err(AgentError::Cancelled)), "got {res:?}");
    assert_eq!(history_snapshot(&handle).await, before, "user message leaked in");
    assert_eq!(prov.n.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_r13_run_turn_content_with_closed_receiver_should_cancel_and_leave_history_unchanged()
{
    let prov = Arc::new(Script {
        calls: vec![],
        n: AtomicUsize::new(0),
    });
    let handle = AgentHandle::spawn(config(Arc::clone(&prov), vec![], AgentMode::Build));
    handle
        .load_history(vec![Message::user("earlier"), Message::assistant("reply")])
        .await;
    let before = history_snapshot(&handle).await;

    let (tx, rx) = mpsc::channel::<AgentEvent>(8);
    drop(rx);
    let res = tokio::time::timeout(
        Duration::from_secs(10),
        handle.run_turn_content(Content::from("new prompt"), tx),
    )
    .await
    .expect("hung");

    assert!(matches!(res, Err(AgentError::Cancelled)), "got {res:?}");
    assert_eq!(history_snapshot(&handle).await, before, "user message leaked in");
    assert_eq!(prov.n.load(Ordering::SeqCst), 0);
}
