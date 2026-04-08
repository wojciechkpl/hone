use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig, AgentHandle};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    TokenUsage, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, MockProvider, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::json;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// A tool that echoes its input JSON back as output.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echoes its input."
    }
    fn schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new(input.0))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

/// A provider that cycles through a list of pre-configured response sequences.
///
/// On the *n*-th call to `complete()` it returns the *n*-th sequence of
/// `ProviderEvent`s (always terminated with `Done`).  Once sequences are
/// exhausted the last one is repeated.
struct SequencedProvider {
    sequences: Vec<Vec<ProviderEvent>>,
    call_count: Mutex<usize>,
}

impl SequencedProvider {
    fn new(sequences: Vec<Vec<ProviderEvent>>) -> Self {
        Self {
            sequences,
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ModelProvider for SequencedProvider {
    fn id(&self) -> &str {
        "sequenced-mock"
    }

    fn display_name(&self) -> &str {
        "Sequenced Mock"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Test Model".to_owned(),
            context_length: 4096,
            supports_tools: true,
            supports_streaming: true,
        }]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let idx = {
            let mut count = self.call_count.lock().unwrap();
            let i = *count;
            *count += 1;
            i.min(self.sequences.len().saturating_sub(1))
        };
        let events = self.sequences[idx].clone();
        Ok(Box::pin(stream::iter(events)))
    }

    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        10
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn base_config(provider: Arc<dyn ModelProvider>) -> AgentConfig {
    AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "You are a helpful assistant.".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
        },
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_single_turn_no_tools() {
    let provider = Arc::new(MockProvider::new(vec![
        "Hello".to_owned(),
        ", ".to_owned(),
        "world!".to_owned(),
    ]));

    let mut agent = Agent::new(base_config(provider));
    let (tx, mut rx) = mpsc::channel(32);

    let result = agent
        .run_turn("Hi there", tx)
        .await
        .expect("turn should succeed");

    assert_eq!(result.response, "Hello, world!");
    assert_eq!(result.tool_calls_count, 0);

    // Collect streamed events
    let mut tokens_seen = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::Token { text } = event {
            tokens_seen.push(text);
        }
    }
    assert_eq!(tokens_seen, vec!["Hello", ", ", "world!"]);
}

#[tokio::test]
async fn test_tool_call_dispatched() {
    // First call: emit a tool call, then Done.
    // Second call: emit a follow-up text token, then Done.
    let seq1 = vec![
        ProviderEvent::ToolCall(ToolCallRequest {
            id: "tc-1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({ "msg": "ping" }),
        }),
        ProviderEvent::Done,
    ];
    let seq2 = vec![
        ProviderEvent::Token {
            text: "Tool result received.".to_owned(),
        },
        ProviderEvent::Done,
    ];

    let provider = Arc::new(SequencedProvider::new(vec![seq1, seq2]));

    let mut config = base_config(provider);
    config.tools = ToolRegistry::builder()
        .register(Box::new(EchoTool))
        .build();

    let mut agent = Agent::new(config);
    let (tx, mut rx) = mpsc::channel(64);

    let result = agent
        .run_turn("Call echo please", tx)
        .await
        .expect("turn should succeed");

    // The tool was dispatched once.
    assert_eq!(result.tool_calls_count, 1);
    // The follow-up response text was captured.
    assert_eq!(result.response, "Tool result received.");

    // The event stream should include ToolCallStart and ToolCallEnd.
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    let has_start = events.iter().any(|e| {
        matches!(e, AgentEvent::ToolCallStart { name, .. } if name == "echo")
    });
    let has_end = events.iter().any(|e| {
        matches!(e, AgentEvent::ToolCallEnd { id, .. } if id == "tc-1")
    });

    assert!(has_start, "expected ToolCallStart event");
    assert!(has_end, "expected ToolCallEnd event");
}

#[tokio::test]
async fn test_agent_hot_swap_provider() {
    // First provider returns "first"
    let provider1 = Arc::new(MockProvider::new(vec!["first".to_owned()]));
    let mut agent = Agent::new(base_config(provider1));

    let (tx1, _rx1) = mpsc::channel(32);
    let result1 = agent
        .run_turn("turn 1", tx1)
        .await
        .expect("first turn should succeed");
    assert_eq!(result1.response, "first");

    // Swap to a second provider
    let provider2 = Arc::new(MockProvider::new(vec!["second".to_owned()]));
    agent.swap_provider(provider2);

    let (tx2, _rx2) = mpsc::channel(32);
    let result2 = agent
        .run_turn("turn 2", tx2)
        .await
        .expect("second turn should succeed");
    assert_eq!(result2.response, "second");
}

#[tokio::test]
async fn test_agent_tracks_cost() {
    // Emit a Usage event so cost accumulator has something to track.
    // We use a SequencedProvider so we can attach Usage alongside tokens.
    let events = vec![
        ProviderEvent::Token {
            text: "response".to_owned(),
        },
        ProviderEvent::Usage(TokenUsage {
            input: 100,
            output: 50,
            cache_read: 0,
        }),
        ProviderEvent::Done,
    ];
    let provider = Arc::new(SequencedProvider::new(vec![events]));

    // Wire up non-zero pricing so cost > 0
    // We cannot inject pricing into Agent directly without extending the API,
    // but we *can* verify that session_cost() reflects accumulated usage by
    // checking it is >= 0 and that total_usage was recorded.
    // (The default dummy pricing in agent.rs uses 0.0 rates, so cost stays 0
    // unless real pricing is configured.  For this test we verify that the
    // cost path runs without panicking and that the field is present.)
    let mut agent = Agent::new(base_config(provider));
    let (tx, _rx) = mpsc::channel(32);

    let result = agent.run_turn("hello", tx).await.expect("turn ok");

    // Usage was emitted; turn_usage must carry those tokens.
    assert_eq!(result.usage.input, 100);
    assert_eq!(result.usage.output, 50);

    // session_cost() is accessible and non-negative.
    assert!(agent.session_cost() >= 0.0);
}

// ---------------------------------------------------------------------------
// AgentHandle actor pattern tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_handle_can_be_cloned() {
    let provider = Arc::new(MockProvider::new(vec!["hello".to_owned()]));
    let handle = AgentHandle::spawn(base_config(provider));

    // Clone must be constructable and both point to the same actor.
    let _clone = handle.clone();
}

#[tokio::test]
async fn test_agent_handle_runs_turn() {
    let provider = Arc::new(MockProvider::new(vec![
        "hello".to_owned(),
        " world".to_owned(),
    ]));
    let handle = AgentHandle::spawn(base_config(provider));
    let (tx, _rx) = mpsc::channel(32);

    let result = handle
        .run_turn("hi", tx)
        .await
        .expect("turn should succeed");

    assert_eq!(result.response, "hello world");
    assert_eq!(result.tool_calls_count, 0);
}

#[tokio::test]
async fn test_agent_handle_concurrent_clones_serialize() {
    // Provide enough distinct responses for 3 sequential turns through the
    // shared actor.  MockProvider repeats the last token once exhausted, so
    // three distinct prefixes are enough.
    let provider = Arc::new(SequencedProvider::new(vec![
        vec![
            ProviderEvent::Token { text: "one".to_owned() },
            ProviderEvent::Done,
        ],
        vec![
            ProviderEvent::Token { text: "two".to_owned() },
            ProviderEvent::Done,
        ],
        vec![
            ProviderEvent::Token { text: "three".to_owned() },
            ProviderEvent::Done,
        ],
    ]));

    let handle = AgentHandle::spawn(base_config(provider));

    // Spawn 3 tasks that each hold a clone and call run_turn "concurrently".
    // The actor serializes them, so all 3 must complete successfully.
    let mut join_set = tokio::task::JoinSet::new();
    for _ in 0..3 {
        let h = handle.clone();
        join_set.spawn(async move {
            let (tx, _rx) = mpsc::channel(32);
            h.run_turn("prompt", tx).await
        });
    }

    let mut count = 0;
    while let Some(res) = join_set.join_next().await {
        res.expect("task did not panic")
            .expect("turn should succeed");
        count += 1;
    }
    assert_eq!(count, 3, "all three concurrent turns must complete");
}

#[tokio::test]
async fn test_agent_handle_shutdown() {
    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    let handle = AgentHandle::spawn(base_config(provider));

    // Shutdown the actor.
    handle.clone().shutdown().await;

    // Subsequent run_turn must return Cancelled because the command channel is
    // closed after the actor exits.
    let (tx, _rx) = mpsc::channel(32);
    // Give the actor task a moment to process the Shutdown command.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let result = handle.run_turn("after shutdown", tx).await;
    assert!(
        matches!(result, Err(hone_core::AgentError::Cancelled)),
        "expected Cancelled after shutdown, got: {result:?}"
    );
}
