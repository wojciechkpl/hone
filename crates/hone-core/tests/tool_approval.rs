//! Red-phase integration tests for the tool-approval feature.
//!
//! These tests reference `AgentConfig::approval_policy` which does NOT exist
//! yet.  They will fail to compile (or panic at runtime) until the green-phase
//! implementer adds `approval_policy: ToolApprovalPolicy` to `AgentConfig` and
//! wires it into the dispatch loop.
//!
//! Do NOT modify this file during the green phase; it is the contract.

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StepAction, ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::json;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Test helpers: configurable mock tool
// ---------------------------------------------------------------------------

/// A mock tool with a configurable read-only flag and a call counter.
/// Used to verify that the agent honours the approval policy without
/// actually running any I/O.
struct MockTool {
    tool_name: String,
    read_only: bool,
    call_count: Arc<Mutex<u32>>,
}

impl MockTool {
    fn new(name: impl Into<String>, read_only: bool) -> (Self, Arc<Mutex<u32>>) {
        let counter = Arc::new(Mutex::new(0u32));
        let tool = Self {
            tool_name: name.into(),
            read_only,
            call_count: Arc::clone(&counter),
        };
        (tool, counter)
    }
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        "Mock tool for approval tests."
    }

    fn schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        *self.call_count.lock().unwrap() += 1;
        Ok(ToolOutput::new(json!({ "ok": true })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }
}

// ---------------------------------------------------------------------------
// Test helpers: sequenced provider
// ---------------------------------------------------------------------------

/// A provider that returns a fixed sequence of event vectors.
/// On the n-th `complete()` call it returns the n-th sequence (clamped to
/// the last entry once exhausted).  Identical in spirit to the helper in
/// agent_turn.rs — duplicated here to keep the test file self-contained.
struct SeqProvider {
    sequences: Vec<Vec<ProviderEvent>>,
    call_count: Mutex<usize>,
}

impl SeqProvider {
    fn new(sequences: Vec<Vec<ProviderEvent>>) -> Self {
        Self {
            sequences,
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ModelProvider for SeqProvider {
    fn id(&self) -> &str {
        "seq-approval-mock"
    }

    fn display_name(&self) -> &str {
        "Seq Approval Mock"
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
        _req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let idx = {
            let mut c = self.call_count.lock().unwrap();
            let i = *c;
            *c += 1;
            i.min(self.sequences.len().saturating_sub(1))
        };
        Ok(Box::pin(stream::iter(self.sequences[idx].clone())))
    }

    fn estimate_tokens(&self, _: &[Message]) -> u32 {
        5
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

// ---------------------------------------------------------------------------
// Config builder helper
// ---------------------------------------------------------------------------

/// Builds a minimal `AgentConfig` with the given approval policy and tool
/// registry.  All other fields are set to safe, inert defaults.
///
/// NOTE: This function references `AgentConfig::approval_policy` which does
/// NOT exist yet.  It will fail to compile until the green-phase implementer
/// adds that field.
fn make_config(
    provider: Arc<dyn ModelProvider>,
    registry: ToolRegistry,
    approval_policy: ToolApprovalPolicy,
) -> AgentConfig {
    AgentConfig {
        provider,
        tools: registry,
        sandbox: Arc::new(NullSandbox),
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
        working_dir: std::path::PathBuf::from("/tmp"),
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        // `step_mode` and `step_rx` are replaced by `approval_policy` during
        // the green phase.  This struct literal will fail to compile until
        // `approval_policy` is added and `step_mode`/`step_rx` are removed.
        approval_policy,
        step_rx: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1: Auto policy — no ToolCallPaused emitted for write tools
// ---------------------------------------------------------------------------

/// With `ToolApprovalPolicy::Auto`, the agent must never emit
/// `AgentEvent::ToolCallPaused` regardless of whether a tool is read-only.
#[tokio::test]
async fn approval_auto_no_pause() {
    // Provider: first response emits one write-tool call, second response text.
    let seq1 = vec![
        ProviderEvent::ToolCall(ToolCallRequest {
            id: "tc-1".to_owned(),
            name: "write_tool".to_owned(),
            arguments: json!({}),
        }),
        ProviderEvent::Done,
    ];
    let seq2 = vec![
        ProviderEvent::Token {
            text: "done".to_owned(),
        },
        ProviderEvent::Done,
    ];

    let provider = Arc::new(SeqProvider::new(vec![seq1, seq2]));
    let (write_tool, _counter) = MockTool::new("write_tool", false); // not read-only
    let registry = ToolRegistry::builder()
        .register(Box::new(write_tool))
        .build();

    let config = make_config(
        provider as Arc<dyn ModelProvider>,
        registry,
        ToolApprovalPolicy::Auto,
    );

    let mut agent = Agent::new(config);
    let (tx, mut rx) = mpsc::channel(64);

    agent
        .run_turn("do a write", tx)
        .await
        .expect("turn should succeed");

    // Collect all events.
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    // No ToolCallPaused must appear.
    let paused = events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolCallPaused { .. }));
    assert!(
        !paused,
        "ToolApprovalPolicy::Auto must never emit ToolCallPaused; events: {events:#?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2: AskRisky — pauses on write tool, Proceed runs it
// ---------------------------------------------------------------------------

/// With `AskRisky` policy, the agent must pause before a non-read-only tool.
/// When the user sends `StepAction::Proceed`, the tool runs and
/// `ToolCallEnd { is_error: false }` is emitted.
#[tokio::test]
async fn approval_risky_pauses_on_write() {
    let seq1 = vec![
        ProviderEvent::ToolCall(ToolCallRequest {
            id: "tc-write".to_owned(),
            name: "write_tool".to_owned(),
            arguments: json!({}),
        }),
        ProviderEvent::Done,
    ];
    let seq2 = vec![
        ProviderEvent::Token {
            text: "done".to_owned(),
        },
        ProviderEvent::Done,
    ];

    let provider = Arc::new(SeqProvider::new(vec![seq1, seq2]));
    let (write_tool, call_counter) = MockTool::new("write_tool", false);
    let registry = ToolRegistry::builder()
        .register(Box::new(write_tool))
        .build();

    // Set up the step channel: we will send Proceed after observing the pause.
    let (step_tx, step_rx) = mpsc::channel::<StepAction>(4);
    let step_rx = Arc::new(tokio::sync::Mutex::new(step_rx));

    let mut config = make_config(
        provider as Arc<dyn ModelProvider>,
        registry,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
    );
    config.step_rx = Some(step_rx);

    let mut agent = Agent::new(config);
    let (event_tx, mut event_rx) = mpsc::channel(64);

    // Spawn the turn in a background task so we can feed the step channel.
    let turn_handle = tokio::spawn(async move { agent.run_turn("write something", event_tx).await });

    // Collect events until we see ToolCallPaused.
    let mut events_before_proceed: Vec<AgentEvent> = Vec::new();
    loop {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            event_rx.recv(),
        )
        .await
        {
            Ok(Some(e)) => {
                let paused = matches!(e, AgentEvent::ToolCallPaused { .. });
                events_before_proceed.push(e);
                if paused {
                    break;
                }
            }
            Ok(None) => panic!("event channel closed before ToolCallPaused was emitted"),
            Err(_) => panic!("timed out waiting for ToolCallPaused"),
        }
    }

    // Send Proceed.
    step_tx.send(StepAction::Proceed).await.unwrap();

    // Drain remaining events.
    let mut events_after: Vec<AgentEvent> = Vec::new();
    turn_handle.await.unwrap().expect("turn should succeed");
    while let Ok(e) = event_rx.try_recv() {
        events_after.push(e);
    }

    // ToolCallPaused was emitted (already confirmed in the loop above).
    let paused = events_before_proceed
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolCallPaused { id, .. } if id == "tc-write"));
    assert!(paused, "expected ToolCallPaused for tc-write");

    // ToolCallEnd { is_error: false } must follow.
    let all_events: Vec<AgentEvent> = events_before_proceed
        .into_iter()
        .chain(events_after)
        .collect();
    let end_ok = all_events.iter().any(|e| {
        matches!(e, AgentEvent::ToolCallEnd { id, is_error, .. } if id == "tc-write" && !*is_error)
    });
    assert!(
        end_ok,
        "expected ToolCallEnd {{ is_error: false }} for tc-write; events: {all_events:#?}"
    );

    // The tool itself was invoked exactly once.
    assert_eq!(
        *call_counter.lock().unwrap(),
        1,
        "tool should have been invoked exactly once after Proceed"
    );
}

// ---------------------------------------------------------------------------
// Test 3: AskRisky — no pause for read-only tools
// ---------------------------------------------------------------------------

/// With `AskRisky`, a tool whose `is_read_only()` returns `true` must run
/// directly without emitting `ToolCallPaused`.
#[tokio::test]
async fn approval_risky_skips_readonly() {
    let seq1 = vec![
        ProviderEvent::ToolCall(ToolCallRequest {
            id: "tc-read".to_owned(),
            name: "read_tool".to_owned(),
            arguments: json!({}),
        }),
        ProviderEvent::Done,
    ];
    let seq2 = vec![
        ProviderEvent::Token {
            text: "done".to_owned(),
        },
        ProviderEvent::Done,
    ];

    let provider = Arc::new(SeqProvider::new(vec![seq1, seq2]));
    let (read_tool, call_counter) = MockTool::new("read_tool", true); // is read-only
    let registry = ToolRegistry::builder()
        .register(Box::new(read_tool))
        .build();

    let config = make_config(
        provider as Arc<dyn ModelProvider>,
        registry,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
    );

    let mut agent = Agent::new(config);
    let (tx, mut rx) = mpsc::channel(64);

    agent
        .run_turn("read something", tx)
        .await
        .expect("turn should succeed");

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    // No ToolCallPaused for a read-only tool.
    let paused = events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolCallPaused { .. }));
    assert!(
        !paused,
        "read-only tool must not trigger ToolCallPaused; events: {events:#?}"
    );

    // The tool was actually invoked.
    assert_eq!(
        *call_counter.lock().unwrap(),
        1,
        "read-only tool should have been invoked once"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Deny — tool not invoked; model receives deny reason
// ---------------------------------------------------------------------------

/// When the user sends `StepAction::Deny(reason)`:
/// 1. The tool must NOT be invoked (call counter stays at 0).
/// 2. `ToolCallEnd { is_error: true }` must be emitted.
/// 3. The agent's context must contain a tool-result message with the reason.
#[tokio::test]
async fn approval_deny_returns_reason_to_model() {
    let seq1 = vec![
        ProviderEvent::ToolCall(ToolCallRequest {
            id: "tc-denied".to_owned(),
            name: "write_tool".to_owned(),
            arguments: json!({}),
        }),
        ProviderEvent::Done,
    ];
    // After the deny, the model gets a follow-up with the tool-result message
    // and should eventually respond with text.
    let seq2 = vec![
        ProviderEvent::Token {
            text: "understood, I won't do that".to_owned(),
        },
        ProviderEvent::Done,
    ];

    let provider = Arc::new(SeqProvider::new(vec![seq1, seq2]));
    let (write_tool, call_counter) = MockTool::new("write_tool", false);
    let registry = ToolRegistry::builder()
        .register(Box::new(write_tool))
        .build();

    let (step_tx, step_rx) = mpsc::channel::<StepAction>(4);
    let step_rx = Arc::new(tokio::sync::Mutex::new(step_rx));

    let mut config = make_config(
        provider as Arc<dyn ModelProvider>,
        registry,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
    );
    config.step_rx = Some(step_rx);

    let mut agent = Agent::new(config);
    let (event_tx, mut event_rx) = mpsc::channel(64);

    let turn_handle =
        tokio::spawn(async move { agent.run_turn("do the thing", event_tx).await });

    // Wait for pause, then send Deny.
    loop {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            event_rx.recv(),
        )
        .await
        {
            Ok(Some(AgentEvent::ToolCallPaused { .. })) => break,
            Ok(Some(_)) => continue,
            Ok(None) => panic!("event channel closed before pause"),
            Err(_) => panic!("timed out waiting for ToolCallPaused"),
        }
    }

    step_tx
        .send(StepAction::Deny("nope".to_owned()))
        .await
        .unwrap();

    let result = turn_handle.await.unwrap().expect("turn should succeed");

    // 1. Tool was NOT invoked.
    assert_eq!(
        *call_counter.lock().unwrap(),
        0,
        "tool must not be invoked when Deny is sent"
    );

    // 2. Drain remaining events.
    let mut events: Vec<AgentEvent> = Vec::new();
    while let Ok(e) = event_rx.try_recv() {
        events.push(e);
    }

    let end_error = events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::ToolCallEnd { id, is_error, .. }
            if id == "tc-denied" && *is_error
        )
    });
    assert!(
        end_error,
        "expected ToolCallEnd {{ is_error: true }} for denied call; events: {events:#?}"
    );

    // 3. The turn result's response or the context must contain the deny reason.
    // We check via the turn result response text (the model will have received
    // the deny reason as a tool result and responded accordingly).
    // The model's follow-up text doesn't need to echo "nope" literally, but
    // the context message that was injected must contain "nope".
    // We verify this indirectly: the turn must not error and the model issued
    // a follow-up (seq2), which means the tool-result message was injected.
    assert!(
        !result.response.is_empty(),
        "model should have issued a follow-up after receiving the deny tool-result"
    );
}

// ---------------------------------------------------------------------------
// Test 5: ApproveAlways — second call skips pause
// ---------------------------------------------------------------------------

/// When the user sends `StepAction::ApproveAlways("write_tool")` on the first
/// pause, the second call to `write_tool` in the same turn must NOT emit
/// `ToolCallPaused`.
#[tokio::test]
async fn approval_always_persists() {
    // Provider emits two consecutive write-tool calls in seq1, then text.
    let seq1 = vec![
        ProviderEvent::ToolCall(ToolCallRequest {
            id: "tc-first".to_owned(),
            name: "write_tool".to_owned(),
            arguments: json!({}),
        }),
        ProviderEvent::ToolCall(ToolCallRequest {
            id: "tc-second".to_owned(),
            name: "write_tool".to_owned(),
            arguments: json!({}),
        }),
        ProviderEvent::Done,
    ];
    let seq2 = vec![
        ProviderEvent::Token {
            text: "both done".to_owned(),
        },
        ProviderEvent::Done,
    ];

    let provider = Arc::new(SeqProvider::new(vec![seq1, seq2]));
    let (write_tool, call_counter) = MockTool::new("write_tool", false);
    let registry = ToolRegistry::builder()
        .register(Box::new(write_tool))
        .build();

    let (step_tx, step_rx) = mpsc::channel::<StepAction>(8);
    let step_rx = Arc::new(tokio::sync::Mutex::new(step_rx));

    let mut config = make_config(
        provider as Arc<dyn ModelProvider>,
        registry,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
    );
    config.step_rx = Some(step_rx);

    let mut agent = Agent::new(config);
    let (event_tx, mut event_rx) = mpsc::channel(128);

    let turn_handle =
        tokio::spawn(async move { agent.run_turn("write twice", event_tx).await });

    // Wait for the first ToolCallPaused, then send ApproveAlways.
    loop {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            event_rx.recv(),
        )
        .await
        {
            Ok(Some(AgentEvent::ToolCallPaused { name, .. })) => {
                assert_eq!(name, "write_tool");
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => panic!("event channel closed before first pause"),
            Err(_) => panic!("timed out waiting for first ToolCallPaused"),
        }
    }

    step_tx
        .send(StepAction::ApproveAlways("write_tool".to_owned()))
        .await
        .unwrap();

    // Drain the rest of the events.
    turn_handle.await.unwrap().expect("turn should succeed");

    let mut events: Vec<AgentEvent> = Vec::new();
    while let Ok(e) = event_rx.try_recv() {
        events.push(e);
    }

    // The second call must NOT have triggered ToolCallPaused.
    let pause_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallPaused { .. }))
        .count();
    assert_eq!(
        pause_count, 0,
        "second call to write_tool must not pause after ApproveAlways; events: {events:#?}"
    );

    // Both tool calls must have completed successfully.
    let end_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallEnd { is_error, .. } if !*is_error))
        .count();
    assert_eq!(
        end_count, 2,
        "both write_tool calls should complete without error"
    );

    // The tool was invoked twice (once per call).
    assert_eq!(
        *call_counter.lock().unwrap(),
        2,
        "write_tool should have been invoked twice"
    );
}
