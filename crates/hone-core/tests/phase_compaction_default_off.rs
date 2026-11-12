#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! K11 of `docs/plans/context-phase-events-spec.md`: with `HONE_PHASE_COMPACTION`
//! unset the phase tracker still runs (K1-K3, the `phase_transitions` metric counts)
//! but nothing is compacted. This binary never sets the variable, so it is the
//! default case; run it without `HONE_PHASE_COMPACTION` in the environment.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::hooks::HookRegistry;
use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, Role,
    ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::{Value, json};
use tokio::sync::mpsc;

// ---- fakes ----------------------------------------------------------------------

/// A long (> 200 chars) plain-text result, unique per call so it can be found.
fn long_output(tag: &str) -> String {
    format!("{tag} {}", "lorem ipsum dolor sit amet ".repeat(20))
}

struct FakeTool {
    name: &'static str,
    read_only: bool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for FakeTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "fake"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, input: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let tag = input.0["tag"].as_str().unwrap_or("none").to_owned();
        Ok(ToolOutput::new(json!(long_output(&tag))))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
}

/// Each `complete()` plays the next round: tool calls, or a final answer.
struct Script {
    rounds: Mutex<Vec<Vec<ToolCallRequest>>>,
    n: AtomicUsize,
}

impl Script {
    fn new(rounds: Vec<Vec<ToolCallRequest>>) -> Arc<Self> {
        Arc::new(Self {
            rounds: Mutex::new(rounds),
            n: AtomicUsize::new(0),
        })
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
            id: ModelId::new("mock:claude-test"),
            display_name: "Test".to_owned(),
            context_length: 200_000,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let round = self.rounds.lock().unwrap().get(i).cloned();
        let mut events: Vec<ProviderEvent> = match round {
            Some(calls) => calls.into_iter().map(ProviderEvent::ToolCall).collect(),
            None => vec![ProviderEvent::Token {
                text: "all done".to_owned(),
            }],
        };
        events.push(ProviderEvent::Done);
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

/// A read call whose result carries `tag`.
fn read(id: &str) -> ToolCallRequest {
    call(id, "read_file", json!({ "tag": id }))
}

fn agent_with(rounds: Vec<Vec<ToolCallRequest>>, hooks: Option<HookRegistry>) -> Agent {
    let mut b = ToolRegistry::builder();
    for (name, read_only) in [
        ("read_file", true),
        ("edit_file", false),
        ("apply_patch", false),
        ("shell", false),
    ] {
        b = b.register(Box::new(FakeTool {
            name,
            read_only,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
    }
    Agent::new(AgentConfig {
        provider: Script::new(rounds),
        tools: b.build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:claude-test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:claude-test"),
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
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: hooks.map(Arc::new),
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    })
}

async fn run(agent: &mut Agent) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(512);
    tokio::time::timeout(Duration::from_secs(20), agent.run_turn("go", tx))
        .await
        .expect("turn hung")
        .expect("turn failed");
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

/// The stored text of the tool result for call `id`.
fn result_text(agent: &Agent, id: &str) -> String {
    agent
        .messages()
        .iter()
        .find(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no tool result for {id}"))
        .content
        .as_text()
        .to_string()
}

fn is_summary(text: &str) -> bool {
    text.starts_with("[tool result summarized")
}

fn is_verbatim(text: &str, id: &str) -> bool {
    text.contains(&long_output(id))
}

fn assert_env_unset() {
    assert!(
        std::env::var_os("HONE_PHASE_COMPACTION").is_none(),
        "run this binary without HONE_PHASE_COMPACTION in the environment"
    );
}

async fn run_edit_session(final_call: ToolCallRequest) -> Agent {
    assert_env_unset();
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![read("r3")],
            vec![read("r4")],
            vec![read("r5")],
            vec![final_call],
        ],
        None,
    );
    run(&mut agent).await;
    agent
}

#[tokio::test]
async fn test_k11_an_edit_after_reads_should_count_a_transition_but_compact_nothing_by_default() {
    let agent = run_edit_session(call("e1", "edit_file", json!({ "tag": "e1" }))).await;

    assert_eq!(
        agent.metrics_snapshot().phase_transitions,
        1,
        "the tracker still runs"
    );
    for id in ["r1", "r2", "r3", "r4", "r5"] {
        assert!(
            is_verbatim(&result_text(&agent, id), id),
            "{id} must stay verbatim while phase compaction is off"
        );
    }
}

#[tokio::test]
async fn test_k11_a_shell_test_run_should_count_a_transition_but_compact_nothing_by_default() {
    let agent = run_edit_session(call(
        "s1",
        "shell",
        json!({ "command": "cargo test", "tag": "s1" }),
    ))
    .await;

    assert_eq!(agent.metrics_snapshot().phase_transitions, 1);
    for id in ["r1", "r2", "r3", "r4", "r5"] {
        assert!(is_verbatim(&result_text(&agent, id), id), "{id}");
    }
    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.role == Role::Tool
                && m.content.as_text().starts_with("[tool result summarized")),
        "no result is summarised"
    );
}
