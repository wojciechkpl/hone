#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! K1 and K4 of `docs/plans/context-phase-events-spec.md`, through a real
//! `Agent`: every dispatched tool call feeds the phase tracker once, in order,
//! and the first write or test call after exploration compacts older tool
//! results (outside the recent window, longer than 200 characters) to their
//! structural summary, while results inside the recent window stay verbatim.
//!
//! No new API: it observes `Agent::messages()` and `Agent::metrics_snapshot()`.
//! The model name `mock:claude-test` gives a recent window of 3 (`ContextConfig::for_model`),
//! and no middle-zone reordering. Since K10 the window counts the newest 3 tool
//! RESULTS, not messages. Since K11 compaction is opt-in: this binary turns
//! `HONE_PHASE_COMPACTION=1` on once, before the first agent is built. The default
//! (off) is tested in `phase_compaction_default_off.rs`, a separate binary.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::hooks::{HookRegistry, HookSpec};
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

static ENV: Once = Once::new();

fn agent_with(rounds: Vec<Vec<ToolCallRequest>>, hooks: Option<HookRegistry>) -> Agent {
    ENV.call_once(|| {
        // SAFETY: runs once, before any agent in this binary is built.
        unsafe { std::env::set_var("HONE_PHASE_COMPACTION", "1") };
    });
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

// ---- K4: compaction in a real turn --------------------------------------------------

#[tokio::test]
async fn test_k4_edit_after_reads_should_summarise_old_results_and_keep_recent_ones() {
    // Four reads then an edit: the newest three tool results (r2, r3, r4) are
    // protected, r1 is not.
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![read("r3")],
            vec![read("r4")],
            vec![call("e1", "edit_file", json!({ "tag": "e1" }))],
        ],
        None,
    );

    run(&mut agent).await;

    assert!(
        is_summary(&result_text(&agent, "r1")),
        "the old read is summarised"
    );
    for id in ["r2", "r3", "r4"] {
        assert!(
            is_verbatim(&result_text(&agent, id), id),
            "{id}: a result inside the newest three tool results is unchanged"
        );
    }
    assert_eq!(agent.metrics_snapshot().phase_transitions, 1);
}

#[tokio::test]
async fn test_k10_a_read_of_the_edit_target_should_survive_compaction() {
    // Reads of a.rs (r1) and b.rs (r2) are both outside the newest three; the edit
    // targets a.rs, so only r2 is compacted. The tool results are plain text, so the
    // path can only come from the read call's arguments.
    let read_of = |id: &str, path: &str| call(id, "read_file", json!({ "path": path, "tag": id }));
    let mut agent = agent_with(
        vec![
            vec![read_of("r1", "src/a.rs")],
            vec![read_of("r2", "src/b.rs")],
            vec![read_of("r3", "src/c.rs")],
            vec![read_of("r4", "src/d.rs")],
            vec![read_of("r5", "src/e.rs")],
            vec![call(
                "e1",
                "edit_file",
                json!({ "path": "src/a.rs", "tag": "e1" }),
            )],
        ],
        None,
    );

    run(&mut agent).await;

    assert!(
        is_verbatim(&result_text(&agent, "r1"), "r1"),
        "the read of the edit target stays verbatim"
    );
    assert!(
        is_summary(&result_text(&agent, "r2")),
        "an unrelated old read is compacted"
    );
}

#[tokio::test]
async fn test_k4_apply_patch_should_compact_like_edit_file() {
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![read("r3")],
            vec![read("r4")],
            vec![call("p1", "apply_patch", json!({ "tag": "p1" }))],
        ],
        None,
    );

    run(&mut agent).await;

    assert!(is_summary(&result_text(&agent, "r1")));
    assert_eq!(agent.metrics_snapshot().phase_transitions, 1);
}

#[tokio::test]
async fn test_k4_shell_test_run_should_compact_but_a_plain_shell_call_should_not() {
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![read("r3")],
            vec![read("r4")],
            vec![call(
                "s1",
                "shell",
                json!({ "command": "cd app && cargo test -p foo", "tag": "s1" }),
            )],
        ],
        None,
    );
    run(&mut agent).await;
    assert!(
        is_summary(&result_text(&agent, "r1")),
        "cargo test is a test call"
    );
    assert_eq!(agent.metrics_snapshot().phase_transitions, 1);

    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![read("r3")],
            vec![read("r4")],
            vec![call(
                "s1",
                "shell",
                json!({ "command": "cargo build", "tag": "s1" }),
            )],
        ],
        None,
    );
    run(&mut agent).await;
    assert!(
        is_verbatim(&result_text(&agent, "r1"), "r1"),
        "cargo build is a read"
    );
    assert_eq!(agent.metrics_snapshot().phase_transitions, 0);
}

#[tokio::test]
async fn test_k4_a_read_only_session_should_never_compact() {
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![read("r3")],
            vec![read("r4")],
        ],
        None,
    );

    run(&mut agent).await;

    for id in ["r1", "r2", "r3", "r4"] {
        assert!(is_verbatim(&result_text(&agent, id), id), "{id} untouched");
    }
    assert_eq!(agent.metrics_snapshot().phase_transitions, 0);
}

// ---- K1: which calls feed the tracker -------------------------------------------------

fn deny_edit_hook() -> HookRegistry {
    let mut hooks = HookRegistry::new();
    hooks.register(HookSpec {
        name: "deny-edit".to_owned(),
        event: "PreToolUse".to_owned(),
        command:
            "cat >/dev/null; echo '{\"deny\":true,\"message\":\"no edits\",\"mutated_input\":null}'"
                .to_owned(),
        timeout_ms: 5000,
        tool_name_pattern: Some("edit_file".to_owned()),
    });
    hooks
}

#[tokio::test]
async fn test_k1_a_write_denied_by_a_pre_tool_use_hook_should_not_compact() {
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![call("e1", "edit_file", json!({ "tag": "e1" }))],
        ],
        Some(deny_edit_hook()),
    );

    run(&mut agent).await;

    assert!(
        is_verbatim(&result_text(&agent, "r1"), "r1"),
        "a call that did not run must not trigger compaction"
    );
    assert!(result_text(&agent, "e1").contains("blocked by hook"));
    assert_eq!(agent.metrics_snapshot().phase_transitions, 0);
}

#[tokio::test]
async fn test_k1_a_denied_write_should_not_count_but_a_later_allowed_one_should() {
    // The hook denies edit_file; apply_patch is allowed and compacts.
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![call("e1", "edit_file", json!({ "tag": "e1" }))],
            vec![read("r3")],
            vec![read("r4")],
            vec![call("p1", "apply_patch", json!({ "tag": "p1" }))],
        ],
        Some(deny_edit_hook()),
    );

    run(&mut agent).await;

    assert!(
        is_summary(&result_text(&agent, "r1")),
        "the allowed write compacted r1"
    );
    assert_eq!(
        agent.metrics_snapshot().phase_transitions,
        1,
        "only the allowed write counted"
    );
}

#[tokio::test]
async fn test_k1_parallel_read_batch_should_feed_the_tracker() {
    // read -> test run (Exploration -> Verification, transition 1) -> three reads in
    // ONE parallel batch (Verification -> Exploration only if the batch feeds the
    // tracker) -> edit (Exploration -> Editing, transition 2).
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![call(
                "s1",
                "shell",
                json!({ "command": "pytest", "tag": "s1" }),
            )],
            vec![read("b1"), read("b2"), read("b3")],
            vec![call("e1", "edit_file", json!({ "tag": "e1" }))],
        ],
        None,
    );

    run(&mut agent).await;

    assert_eq!(
        agent.metrics_snapshot().phase_transitions,
        2,
        "the parallel batch of three reads must have returned the tracker to exploration"
    );
}

#[tokio::test]
async fn test_k1_calls_should_feed_the_tracker_in_call_order() {
    // One batch [edit_file, read_file x3]: in order, the write comes first
    // (Exploration -> Editing, 1 transition), then reads change nothing. Fed in
    // another order, or twice, the count would differ or compaction would
    // happen with the wrong window.
    let mut agent = agent_with(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![read("r3")],
            vec![read("r4")],
            vec![
                call("e1", "edit_file", json!({ "tag": "e1" })),
                read("b1"),
                read("b2"),
                read("b3"),
            ],
        ],
        None,
    );

    run(&mut agent).await;

    assert_eq!(agent.metrics_snapshot().phase_transitions, 1);
    assert!(is_summary(&result_text(&agent, "r1")));
}
