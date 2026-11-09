#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Red-phase tests for `docs/plans/tui-approval-fix-spec.md` section 6 (review round 1),
//! hone-core rows R2a/R2b, R3, R5, R7, R8.
//!
//! API assumed from the spec:
//! - `AgentHandle::approve_tools(Vec<String>)`: async, fire-and-forget (R5).
//! - A turn whose event receiver is closed is cancelled: `run_turn` returns
//!   `AgentError::Cancelled` and unanswered tool calls get a tool result containing "cancelled".
//! - Approval gate denial texts: "approval channel closed" (channel dropped) and
//!   "no approval channel" (approval needed, none wired); the latter emits no `ToolCallPaused`.
//!
//! Every wait is bounded by a timeout.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, AgentError, AgentHandle, AgentMode, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StepAction, ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::{Value, json};
use tokio::sync::mpsc;

type StepRx = Arc<tokio::sync::Mutex<mpsc::Receiver<StepAction>>>;

// -- fakes ----------------------------------------------------------------------------------------

struct CountingTool {
    name: &'static str,
    read_only: bool,
    target: Option<PathBuf>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "counts calls"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(t) = &self.target {
            std::fs::write(t, b"written").unwrap();
        }
        Ok(ToolOutput::new(json!({ "ok": true })))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
}

/// First `complete()` emits `calls` then Done; every later call emits "done".
struct Script {
    calls: Vec<ToolCallRequest>,
    n: AtomicUsize,
}

impl Script {
    fn new(calls: Vec<ToolCallRequest>) -> Arc<Self> {
        Arc::new(Self {
            calls,
            n: AtomicUsize::new(0),
        })
    }
    fn completions(&self) -> usize {
        self.n.load(Ordering::SeqCst)
    }
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
        let events = if i == 0 {
            let mut v: Vec<ProviderEvent> = self
                .calls
                .iter()
                .cloned()
                .map(ProviderEvent::ToolCall)
                .collect();
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

fn call(id: &str, name: &str, args: Value) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: args,
    }
}

fn config(
    provider: Arc<dyn ModelProvider>,
    tools: Vec<CountingTool>,
    dir: &Path,
    approval_policy: ToolApprovalPolicy,
    step_rx: Option<StepRx>,
) -> AgentConfig {
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
        working_dir: dir.to_path_buf(),
        memory_store: None,
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy,
        step_rx,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    }
}

fn writer(name: &'static str, dir: &Path, file: &str) -> (CountingTool, Arc<AtomicUsize>, PathBuf) {
    let calls = Arc::new(AtomicUsize::new(0));
    let target = dir.join(file);
    (
        CountingTool {
            name,
            read_only: false,
            target: Some(target.clone()),
            calls: Arc::clone(&calls),
        },
        calls,
        target,
    )
}

fn ask_risky() -> ToolApprovalPolicy {
    ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    }
}

fn channel() -> (mpsc::Sender<StepAction>, StepRx) {
    let (tx, rx) = mpsc::channel(8);
    (tx, Arc::new(tokio::sync::Mutex::new(rx)))
}

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(t.path()).unwrap();
    (t, d)
}

enum Reply {
    Act(StepAction),
    /// Drop the step sender (the UI went away) instead of answering.
    DropSender,
}

/// Run one turn; answer each `ToolCallPaused` with the next reply. Bounded by a timeout.
async fn drive(
    handle: &AgentHandle,
    mut step_tx: Option<mpsc::Sender<StepAction>>,
    mut replies: Vec<Reply>,
) -> (Vec<AgentEvent>, Result<(), String>) {
    replies.reverse();
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let h = handle.clone();
    let turn = tokio::spawn(async move { h.run_turn("go", tx).await });
    let mut events = Vec::new();
    let collect = async {
        while let Some(e) = rx.recv().await {
            if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                match replies.pop() {
                    Some(Reply::Act(a)) => {
                        step_tx.as_ref().unwrap().send(a).await.unwrap();
                    }
                    Some(Reply::DropSender) => {
                        step_tx = None;
                    }
                    None => panic!("unexpected extra pause; events: {events:?}"),
                }
            }
            let terminal = matches!(e, AgentEvent::Done { .. } | AgentEvent::Error { .. });
            events.push(e);
            if terminal {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), collect)
        .await
        .expect("turn hung");
    let res = tokio::time::timeout(Duration::from_secs(5), turn)
        .await
        .expect("turn task hung")
        .unwrap()
        .map(|_| ())
        .map_err(|e| e.to_string());
    (events, res)
}

fn paused(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallPaused { .. }))
        .count()
}

fn end_of(events: &[AgentEvent], id: &str) -> (Value, bool) {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCallEnd {
                id: i,
                result,
                is_error,
                ..
            } if i == id => Some((result.clone(), *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ToolCallEnd for {id}: {events:?}"))
}

async fn always_allowed(handle: &AgentHandle) -> Vec<String> {
    match handle.approval_policy().await {
        ToolApprovalPolicy::AskRisky { always_allowed } => always_allowed,
        other => panic!("expected AskRisky, got {other:?}"),
    }
}

/// Wait for the first `ToolCallPaused` on `rx`.
async fn wait_paused(rx: &mut mpsc::Receiver<AgentEvent>) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = rx.recv().await {
            if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                return;
            }
        }
        panic!("event stream ended before a pause");
    })
    .await
    .expect("never paused");
}

// -- R2a: stale answers --------------------------------------------------------------------------

#[tokio::test]
async fn test_r2a_stale_buffered_proceed_must_not_answer_a_new_pause() {
    let (_t, dir) = tmp();
    let (tool, calls, target) = writer("mock_write", &dir, "out.txt");
    let (step_tx, rx) = channel();
    // A leftover answer from an earlier, cancelled prompt.
    step_tx.send(StepAction::Proceed).await.unwrap();
    let prov = Script::new(vec![call("c1", "mock_write", json!({}))]);
    let handle = AgentHandle::spawn(config(prov, vec![tool], &dir, ask_risky(), Some(rx)));

    let (events, _) = drive(
        &handle,
        Some(step_tx),
        vec![Reply::Act(StepAction::Deny("real answer".to_owned()))],
    )
    .await;

    assert_eq!(paused(&events), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "stale Proceed was applied");
    assert!(!target.exists());
    let (_, is_error) = end_of(&events, "c1");
    assert!(is_error);
}

// -- R2b: cancellation ---------------------------------------------------------------------------

#[tokio::test]
async fn test_r2b_closing_event_receiver_while_paused_should_cancel_turn_and_not_wedge_actor() {
    let (_t, dir) = tmp();
    let (w1, calls1, f1) = writer("mock_write", &dir, "a.txt");
    let (w2, calls2, _f2) = writer("mock_write2", &dir, "b.txt");
    let (_step_tx, rx) = channel();
    let prov = Script::new(vec![
        call("c1", "mock_write", json!({})),
        call("c2", "mock_write2", json!({})),
    ]);
    let handle = AgentHandle::spawn(config(prov, vec![w1, w2], &dir, ask_risky(), Some(rx)));

    let (tx, mut ev_rx) = mpsc::channel::<AgentEvent>(256);
    let h = handle.clone();
    let turn = tokio::spawn(async move { h.run_turn("go", tx).await });
    wait_paused(&mut ev_rx).await;
    drop(ev_rx); // the UI aborted: nobody can answer

    let res = tokio::time::timeout(Duration::from_secs(10), turn)
        .await
        .expect("actor wedged waiting for an answer nobody can give")
        .unwrap();
    assert!(matches!(res, Err(AgentError::Cancelled)), "got {res:?}");

    // No orphan tool_calls: both pending calls have a "cancelled" result.
    let msgs = handle.get_messages().await;
    for id in ["c1", "c2"] {
        let m = msgs
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no tool result for {id}: {msgs:?}"));
        assert!(
            m.content.as_text().contains("cancelled"),
            "{id}: {}",
            m.content.as_text()
        );
    }
    assert_eq!(calls1.load(Ordering::SeqCst), 0);
    assert_eq!(calls2.load(Ordering::SeqCst), 0);
    assert!(!f1.exists());

    // The actor is usable again.
    let (tx2, _rx2) = mpsc::channel::<AgentEvent>(256);
    let again = tokio::time::timeout(Duration::from_secs(10), handle.run_turn("again", tx2))
        .await
        .expect("second turn hung");
    assert!(again.is_ok(), "{again:?}");
}

#[tokio::test]
async fn test_r2b_closed_event_receiver_before_model_call_should_cancel_without_calling_provider() {
    let (_t, dir) = tmp();
    let (tool, calls, _f) = writer("mock_write", &dir, "out.txt");
    let prov = Script::new(vec![call("c1", "mock_write", json!({}))]);
    let handle = AgentHandle::spawn(config(
        Arc::clone(&prov) as Arc<dyn ModelProvider>,
        vec![tool],
        &dir,
        ToolApprovalPolicy::Auto,
        None,
    ));

    let (tx, rx) = mpsc::channel::<AgentEvent>(8);
    drop(rx);
    let res = tokio::time::timeout(Duration::from_secs(10), handle.run_turn("go", tx))
        .await
        .expect("hung");

    assert!(matches!(res, Err(AgentError::Cancelled)), "got {res:?}");
    assert_eq!(prov.completions(), 0, "no model call for a dead turn");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

// -- R3: the gate fails closed -------------------------------------------------------------------

#[tokio::test]
async fn test_r3_dropping_step_sender_at_a_pause_should_deny_not_run() {
    let (_t, dir) = tmp();
    let (tool, calls, target) = writer("mock_write", &dir, "out.txt");
    let (step_tx, rx) = channel();
    let prov = Script::new(vec![call("c1", "mock_write", json!({}))]);
    let handle = AgentHandle::spawn(config(prov, vec![tool], &dir, ask_risky(), Some(rx)));

    let (events, _) = drive(&handle, Some(step_tx), vec![Reply::DropSender]).await;

    assert_eq!(paused(&events), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "must not fail open");
    assert!(!target.exists());
    let (result, is_error) = end_of(&events, "c1");
    assert!(is_error);
    assert!(
        result.to_string().contains("approval channel closed"),
        "{result}"
    );
}

#[tokio::test]
async fn test_r3_approval_required_without_channel_should_deny_and_not_emit_pause() {
    let (_t, dir) = tmp();
    let (tool, calls, target) = writer("mock_write", &dir, "out.txt");
    let prov = Script::new(vec![call("c1", "mock_write", json!({}))]);
    let handle = AgentHandle::spawn(config(prov, vec![tool], &dir, ask_risky(), None));

    let (events, _) = drive(&handle, None, vec![]).await;

    assert_eq!(paused(&events), 0, "phantom pause: {events:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "must not fail open");
    assert!(!target.exists());
    let (result, is_error) = end_of(&events, "c1");
    assert!(is_error);
    assert!(result.to_string().contains("no approval channel"), "{result}");
}

// -- R5: approve_tools is fire-and-forget --------------------------------------------------------

#[tokio::test]
async fn test_r5_approve_tools_while_turn_paused_should_return_immediately_and_apply_after() {
    let (_t, dir) = tmp();
    let (tool, _calls, _f) = writer("mock_write", &dir, "out.txt");
    let (step_tx, rx) = channel();
    let prov = Script::new(vec![call("c1", "mock_write", json!({}))]);
    let handle = AgentHandle::spawn(config(prov, vec![tool], &dir, ask_risky(), Some(rx)));

    let (tx, mut ev_rx) = mpsc::channel::<AgentEvent>(256);
    let h = handle.clone();
    let turn = tokio::spawn(async move { h.run_turn("go", tx).await });
    wait_paused(&mut ev_rx).await;

    tokio::time::timeout(
        Duration::from_secs(2),
        handle.approve_tools(vec!["mock_write".to_owned(), "other".to_owned()]),
    )
    .await
    .expect("approve_tools must not wait for the paused actor");

    step_tx.send(StepAction::Proceed).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), turn)
        .await
        .expect("turn hung")
        .unwrap()
        .unwrap();

    let allowed = always_allowed(&handle).await;
    assert!(allowed.contains(&"mock_write".to_owned()), "{allowed:?}");
    assert!(allowed.contains(&"other".to_owned()), "{allowed:?}");
}

// -- R7: --step keeps the read-before-edit guard informed ---------------------------------------

#[tokio::test]
async fn test_r7_always_ask_approved_read_then_edit_should_not_inject_synthetic_read() {
    let (_t, dir) = tmp();
    let file = dir.join("f.txt").to_string_lossy().into_owned();
    let read_calls = Arc::new(AtomicUsize::new(0));
    let edit_calls = Arc::new(AtomicUsize::new(0));
    let read = CountingTool {
        name: "read_file",
        read_only: true,
        target: None,
        calls: Arc::clone(&read_calls),
    };
    let edit = CountingTool {
        name: "edit_file",
        read_only: false,
        target: None,
        calls: Arc::clone(&edit_calls),
    };
    let (step_tx, rx) = channel();
    let prov = Script::new(vec![
        call("r1", "read_file", json!({ "path": file })),
        call("e1", "edit_file", json!({ "path": file })),
    ]);
    let handle = AgentHandle::spawn(config(
        prov,
        vec![read, edit],
        &dir,
        ToolApprovalPolicy::AlwaysAsk,
        Some(rx),
    ));

    let (events, _) = drive(
        &handle,
        Some(step_tx),
        vec![
            Reply::Act(StepAction::Proceed),
            Reply::Act(StepAction::Proceed),
        ],
    )
    .await;

    assert_eq!(paused(&events), 2, "{events:?}");
    assert_eq!(edit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        read_calls.load(Ordering::SeqCst),
        1,
        "the approved read must count; no synthetic read_file"
    );
}

// -- R8: approve-always scope --------------------------------------------------------------------

#[tokio::test]
async fn test_r8_approve_always_should_allow_the_paused_call_name_not_the_name_sent() {
    let (_t, dir) = tmp();
    let (tool, calls, _f) = writer("mock_write", &dir, "out.txt");
    let (step_tx, rx) = channel();
    let prov = Script::new(vec![call("c1", "mock_write", json!({}))]);
    let handle = AgentHandle::spawn(config(prov, vec![tool], &dir, ask_risky(), Some(rx)));

    let (events, _) = drive(
        &handle,
        Some(step_tx),
        vec![Reply::Act(StepAction::ApproveAlways("other_tool".to_owned()))],
    )
    .await;

    assert_eq!(paused(&events), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "approve-always also approves this call");
    let allowed = always_allowed(&handle).await;
    assert!(allowed.contains(&"mock_write".to_owned()), "{allowed:?}");
    assert!(
        !allowed.contains(&"other_tool".to_owned()),
        "the TUI-sent name must not be trusted: {allowed:?}"
    );
}
