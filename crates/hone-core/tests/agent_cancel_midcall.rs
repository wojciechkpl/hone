#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W50 (agent part) of `docs/plans/worker-tools-spec.md`: a tool call cancelled
//! mid-dispatch (W48: the caller went away during a slow tool) is recorded in
//! the audit log as cancelled mid-call, since it may have partly run; and the
//! next turn on the same agent succeeds because the history has no orphan call.

mod worker_tools_support;

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::AgentHandle;
use hone_core::egress_log::EgressLog;
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, Role,
    ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use worker_tools_support::*;

/// Sleeps for 10 s (virtual or real) when called: "a slow tool".
struct SlowTool {
    read_only: bool,
}

#[async_trait]
impl Tool for SlowTool {
    fn name(&self) -> &str {
        "slow_tool"
    }
    fn description(&self) -> &str {
        "slow"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(ToolOutput::new(json!({ "ok": true })))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
}

/// First request: one `slow_tool` call. Later requests: "done", after
/// checking that every assistant tool call in the history has a tool result.
struct Checking {
    n: Mutex<usize>,
    orphans: Mutex<Vec<String>>,
}

#[async_trait]
impl ModelProvider for Checking {
    fn id(&self) -> &str {
        "checking"
    }
    fn display_name(&self) -> &str {
        "Checking"
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
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let first = {
            let mut n = self.n.lock().unwrap();
            *n += 1;
            *n == 1
        };
        let answered: Vec<&String> = request
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| m.tool_call_id.as_ref())
            .collect();
        for m in &request.messages {
            for call in &m.tool_calls {
                if !answered.contains(&&call.id) {
                    self.orphans.lock().unwrap().push(call.id.clone());
                }
            }
        }
        let events = if first {
            vec![
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "slow1".to_owned(),
                    name: "slow_tool".to_owned(),
                    arguments: json!({}),
                }),
                ProviderEvent::Done,
            ]
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
    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        10
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

struct Cancelled {
    handle: AgentHandle,
    provider: Arc<Checking>,
    egress: Arc<tokio::sync::Mutex<EgressLog>>,
    log_path: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

/// Start a turn that calls `slow_tool`, then cancel it mid-call (abort the
/// waiting task and drop the receiver, as Esc does) and wait for the actor.
async fn cancel_mid_call() -> Cancelled {
    cancel_mid_call_with(false).await
}

async fn cancel_mid_call_with(read_only: bool) -> Cancelled {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("audit.jsonl");
    let egress = Arc::new(tokio::sync::Mutex::new(EgressLog::new(log_path.clone())));
    let provider = Arc::new(Checking {
        n: Mutex::new(0),
        orphans: Mutex::new(vec![]),
    });
    let tools = ToolRegistry::builder()
        .register(Box::new(SlowTool { read_only }))
        .build();
    let mut config = lead_config(ScriptedProvider::new(&[]), tools, dir.path());
    config.provider = Arc::clone(&provider) as Arc<dyn ModelProvider>;
    config.approval_policy = ToolApprovalPolicy::Auto;
    config.egress_log = Some(Arc::clone(&egress));
    let handle = AgentHandle::spawn(config);

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let h = handle.clone();
    let task = tokio::spawn(async move { h.run_turn("go", tx).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(e) = rx.recv().await {
            if matches!(e, AgentEvent::ToolCallStart { .. }) {
                return;
            }
        }
        panic!("events closed before the tool started");
    })
    .await
    .expect("the slow tool should start");
    tokio::time::sleep(Duration::from_millis(100)).await;

    task.abort();
    drop(rx);
    tokio::time::timeout(Duration::from_secs(3), handle.get_messages())
        .await
        .expect("the actor is free after the cancel (W48)");

    Cancelled {
        handle,
        provider,
        egress,
        log_path,
        _dir: dir,
    }
}

#[tokio::test]
async fn test_w50_a_call_cancelled_mid_dispatch_should_be_recorded_as_cancelled_in_the_audit_log() {
    let c = cancel_mid_call().await;

    c.egress.lock().await.flush().unwrap();
    let entries: Vec<Value> = std::fs::read_to_string(&c.log_path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let entry = entries
        .iter()
        .find(|e| e["tool"] == "slow_tool")
        .unwrap_or_else(|| panic!("the cancelled call is not in the audit log: {entries:?}"));
    let text = format!("{} {}", entry["result_preview"], entry["block_reason"]).to_lowercase();
    assert!(
        text.contains("cancel"),
        "the entry must say the call was cancelled mid-call: {entry}"
    );
}

#[tokio::test]
async fn test_w50_next_turn_after_a_mid_call_cancel_should_succeed_with_no_orphan_tool_call() {
    let c = cancel_mid_call().await;

    let (tx, _rx) = mpsc::channel::<AgentEvent>(256);
    let result = tokio::time::timeout(Duration::from_secs(5), c.handle.run_turn("again", tx))
        .await
        .expect("the next turn should not hang")
        .expect("the next turn should succeed");

    assert_eq!(result.response, "done");
    assert!(
        c.provider.orphans.lock().unwrap().is_empty(),
        "assistant tool calls without a tool result reached the provider: {:?}",
        c.provider.orphans.lock().unwrap()
    );
    // And the history itself has a result for the cancelled call.
    let messages = c.handle.get_messages().await;
    assert!(
        messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("slow1")),
        "the cancelled call has a (cancelled) tool result in the history"
    );
}

/// W48 says it applies to every tool call; a read-only tool takes the agent
/// loop's parallel dispatch path, which must also drop the call when the
/// caller goes away.
#[tokio::test]
async fn test_w48_a_slow_read_only_tool_should_also_free_the_actor_on_cancel() {
    let c = cancel_mid_call_with(true).await;

    let (tx, _rx) = mpsc::channel::<AgentEvent>(256);
    let result = tokio::time::timeout(Duration::from_secs(5), c.handle.run_turn("again", tx))
        .await
        .expect("the next turn should not hang")
        .expect("the next turn should succeed");
    assert_eq!(result.response, "done");
}
