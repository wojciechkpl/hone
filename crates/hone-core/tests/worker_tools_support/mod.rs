//! Shared fakes for the worker-tools tests (`docs/plans/worker-tools-spec.md`).
//!
//! Test-only support code: a scripted provider that records every request
//! (including the tool names sent) and can emit tool calls per worker, fake
//! tools that log their invocations, and a driver that plays the TUI side of
//! the step channel.
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{
    AgentConfig, AgentMode, AgentRegistry, Orchestrator, RetryConfig, SubTask, TaskPlan,
    WorkerResult,
};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StepAction, ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::{NullSandbox, SandboxPolicy};
use hone_tools::{
    SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry, ToolSubjects,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Fake tools
// ---------------------------------------------------------------------------

/// One executed tool call.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub tool: String,
    pub input: Value,
    /// `allow_env` of the sandbox policy the tool was given (a marker the tests
    /// use to tell the session policy from any other).
    pub sandbox_allow_env: Vec<String>,
}

pub type InvocationLog = Arc<Mutex<Vec<Invocation>>>;

pub struct FakeTool {
    pub name: &'static str,
    pub read_only: bool,
    /// When set, every call reports this command as its policy subject.
    pub command_subject: Option<&'static str>,
    pub log: InvocationLog,
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
    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.log.lock().unwrap().push(Invocation {
            tool: self.name.to_owned(),
            input: input.0.clone(),
            sandbox_allow_env: ctx.sandbox_policy.allow_env.clone(),
        });
        Ok(ToolOutput::new(json!({ "ok": true })))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn subjects(&self, _input: &ToolInput, _sctx: &SubjectContext) -> ToolSubjects {
        match self.command_subject {
            Some(c) => ToolSubjects {
                subjects: vec![hone_config::policy::Subject::Command(c.to_owned())],
                unparsed: false,
                ..Default::default()
            },
            None => ToolSubjects::default(),
        }
    }
}

/// The session's tool set: the six tools the built-in specialists list (with
/// the real names and read-only flags) plus two extras no built-in agent lists.
pub const SESSION_TOOL_NAMES: [&str; 8] = [
    "read_file",
    "edit_file",
    "write_file",
    "shell",
    "grep",
    "glob",
    "web_fetch",
    "git_commit",
];

pub fn session_tools(log: &InvocationLog) -> ToolRegistry {
    session_tools_with_shell_subject(log, None)
}

pub fn session_tools_with_shell_subject(
    log: &InvocationLog,
    shell_subject: Option<&'static str>,
) -> ToolRegistry {
    let mut b = ToolRegistry::builder();
    for (name, ro) in [
        ("read_file", true),
        ("edit_file", false),
        ("write_file", false),
        ("shell", false),
        ("grep", true),
        ("glob", true),
        ("web_fetch", true),
        ("git_commit", false),
    ] {
        b = b.register(Box::new(FakeTool {
            name,
            read_only: ro,
            command_subject: if name == "shell" { shell_subject } else { None },
            log: Arc::clone(log),
        }));
    }
    b.build()
}

pub fn sorted(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------
// Scripted provider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Decompose,
    Merge,
    /// A worker, identified by the marker in its task description.
    Worker(String),
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Seen {
    pub kind: Kind,
    /// Sorted names of the tool definitions sent with the request.
    pub tools: Vec<String>,
}

/// Answers lead requests (decompose with `decompose_json`, merge with
/// "merged") and worker requests: the first request of a worker issues its
/// scripted tool calls (if any), the next answers `done-<marker>`.
pub struct ScriptedProvider {
    markers: Vec<String>,
    calls: Mutex<HashMap<String, Vec<ToolCallRequest>>>,
    counts: Mutex<HashMap<String, usize>>,
    pub seen: Mutex<Vec<Seen>>,
    pub decompose_json: Mutex<String>,
    /// Sleep (tokio clock) before answering a worker request.
    pub worker_delay: Mutex<Option<Duration>>,
}

impl ScriptedProvider {
    pub fn new(markers: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            markers: markers.iter().map(|s| (*s).to_owned()).collect(),
            calls: Mutex::new(HashMap::new()),
            counts: Mutex::new(HashMap::new()),
            seen: Mutex::new(Vec::new()),
            decompose_json: Mutex::new(String::new()),
            worker_delay: Mutex::new(None),
        })
    }

    pub fn script(&self, marker: &str, calls: Vec<ToolCallRequest>) {
        self.calls.lock().unwrap().insert(marker.to_owned(), calls);
    }

    /// Tool names of the first request the worker `marker` sent.
    pub fn worker_tools(&self, marker: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.kind == Kind::Worker(marker.to_owned()))
            .unwrap_or_else(|| panic!("worker {marker} never called the provider"))
            .tools
            .clone()
    }

    pub fn requests_of(&self, pred: impl Fn(&Kind) -> bool) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| pred(&s.kind))
            .cloned()
            .collect()
    }

    fn classify(&self, text: &str) -> Kind {
        if text.contains("You are a lead agent. Decompose") {
            return Kind::Decompose;
        }
        if text.contains("You are the lead agent. Based on") {
            return Kind::Merge;
        }
        for m in &self.markers {
            if text.contains(&format!("## Task\n{m}")) {
                return Kind::Worker(m.clone());
            }
        }
        Kind::Unknown
    }
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted-worker-tools"
    }
    fn display_name(&self) -> &str {
        "Scripted"
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
        let text = request
            .messages
            .iter()
            .map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join("\n");
        let kind = self.classify(&text);
        let mut tools: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
        tools.sort();
        self.seen.lock().unwrap().push(Seen {
            kind: kind.clone(),
            tools,
        });

        let token = |t: String| vec![ProviderEvent::Token { text: t }, ProviderEvent::Done];
        let events = match &kind {
            Kind::Decompose => token(self.decompose_json.lock().unwrap().clone()),
            Kind::Merge | Kind::Unknown => token("merged".to_owned()),
            Kind::Worker(marker) => {
                let delay = *self.worker_delay.lock().unwrap();
                if let Some(d) = delay {
                    tokio::time::sleep(d).await;
                }
                let n = {
                    let mut c = self.counts.lock().unwrap();
                    let e = c.entry(marker.clone()).or_insert(0);
                    *e += 1;
                    *e
                };
                let scripted = self
                    .calls
                    .lock()
                    .unwrap()
                    .get(marker)
                    .cloned()
                    .unwrap_or_default();
                if n == 1 && !scripted.is_empty() {
                    let mut evs: Vec<ProviderEvent> =
                        scripted.into_iter().map(ProviderEvent::ToolCall).collect();
                    evs.push(ProviderEvent::Done);
                    evs
                } else {
                    token(format!("done-{marker}"))
                }
            }
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

pub fn tool_call(id: &str, name: &str, args: Value) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: args,
    }
}

// ---------------------------------------------------------------------------
// Config, registry, plans
// ---------------------------------------------------------------------------

pub fn lead_config(
    provider: Arc<ScriptedProvider>,
    tools: ToolRegistry,
    working_dir: &Path,
) -> AgentConfig {
    AgentConfig {
        provider,
        tools,
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(SandboxPolicy::permissive()),
        system_prompt: "You are a lead orchestrator agent.".to_owned(),
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
        working_dir: working_dir.to_path_buf(),
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

pub fn builtin_registry() -> AgentRegistry {
    let mut r = AgentRegistry::new();
    r.load_builtins();
    r
}

pub fn subtask(id: &str, marker: &str, agent: &str) -> SubTask {
    SubTask {
        id: id.to_owned(),
        description: marker.to_owned(),
        agent: agent.to_owned(),
        context: String::new(),
        depends_on: vec![],
        acceptance_tests: vec![],
        budget_usd: None,
        scope: vec![],
        requires_capabilities: vec![],
    }
}

pub fn plan(subtasks: Vec<SubTask>) -> TaskPlan {
    TaskPlan {
        goal: "goal".to_owned(),
        subtasks,
    }
}

pub type StepRx = Arc<tokio::sync::Mutex<mpsc::Receiver<StepAction>>>;

pub fn step_channel() -> (mpsc::Sender<StepAction>, StepRx) {
    let (tx, rx) = mpsc::channel(16);
    (tx, Arc::new(tokio::sync::Mutex::new(rx)))
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

pub struct RunOutcome {
    pub events: Vec<AgentEvent>,
    pub workers: Vec<WorkerResult>,
    /// Pauses that arrived while an earlier pause was still unanswered.
    pub overlapping_pauses: usize,
}

/// How the simulated UI answers a `ToolCallPaused` (by call id).
pub type Answerer = Box<dyn Fn(&str) -> StepAction + Send>;

/// Run `orchestrator.run_plan(plan)` while playing the TUI: every
/// `ToolCallPaused` is answered through `step_tx` after a short pause (during
/// which any further `ToolCallPaused` counts as an overlapping prompt).
/// Without `answer`/`step_tx` nothing is answered. Panics if the run does not
/// finish within `limit` of the tokio clock (e.g. an unanswered prompt).
pub async fn run_plan_with_ui(
    orchestrator: Orchestrator,
    plan: TaskPlan,
    step_tx: Option<mpsc::Sender<StepAction>>,
    answer: Answerer,
) -> RunOutcome {
    run_plan_with_ui_limit(orchestrator, plan, step_tx, answer, Duration::from_secs(20)).await
}

/// Like [`run_plan_with_ui`] with an explicit give-up limit (use a large one
/// with `start_paused = true` tests whose workers legitimately run for hours).
pub async fn run_plan_with_ui_limit(
    mut orchestrator: Orchestrator,
    plan: TaskPlan,
    step_tx: Option<mpsc::Sender<StepAction>>,
    answer: Answerer,
    limit: Duration,
) -> RunOutcome {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let run = tokio::spawn(async move { orchestrator.run_plan(plan, tx).await });

    let mut events = Vec::new();
    let mut overlapping = 0usize;
    let drive = async {
        while let Some(ev) = rx.recv().await {
            let paused_id = match &ev {
                AgentEvent::ToolCallPaused { id, .. } => Some(id.clone()),
                _ => None,
            };
            events.push(ev);
            if let (Some(id), Some(step_tx)) = (paused_id, step_tx.as_ref()) {
                tokio::time::sleep(Duration::from_millis(50)).await;
                while let Ok(more) = rx.try_recv() {
                    if matches!(more, AgentEvent::ToolCallPaused { .. }) {
                        overlapping += 1;
                    }
                    events.push(more);
                }
                let _ = step_tx.send(answer(&id)).await;
            }
        }
    };
    if tokio::time::timeout(limit, drive).await.is_err() {
        panic!("run did not finish (unanswered prompt?); events so far: {events:?}");
    }
    let result = run.await.expect("run task").expect("run_plan");
    RunOutcome {
        events,
        workers: result.workers,
        overlapping_pauses: overlapping,
    }
}

pub fn proceed_all() -> Answerer {
    Box::new(|_| StepAction::Proceed)
}

pub fn paused_ids(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallPaused { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// The `ToolCallEnd` for call `id`: (result, is_error).
pub fn call_end(events: &[AgentEvent], id: &str) -> Option<(Value, bool)> {
    events.iter().find_map(|e| match e {
        AgentEvent::ToolCallEnd {
            id: i,
            result,
            is_error,
            ..
        } if i == id => Some((result.clone(), *is_error)),
        _ => None,
    })
}

pub fn invoked(log: &InvocationLog) -> Vec<String> {
    log.lock().unwrap().iter().map(|i| i.tool.clone()).collect()
}

pub fn new_log() -> InvocationLog {
    Arc::new(Mutex::new(Vec::new()))
}

// ---------------------------------------------------------------------------
// Single-agent helpers (shared "always", W13-W16)
// ---------------------------------------------------------------------------

/// Answers the first request with `calls`, every later one with "done".
pub struct CallsOnce {
    calls: Vec<ToolCallRequest>,
    n: Mutex<usize>,
}

impl CallsOnce {
    pub fn new(calls: Vec<ToolCallRequest>) -> Arc<Self> {
        Arc::new(Self {
            calls,
            n: Mutex::new(0),
        })
    }
}

#[async_trait]
impl ModelProvider for CallsOnce {
    fn id(&self) -> &str {
        "calls-once"
    }
    fn display_name(&self) -> &str {
        "Calls once"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }
    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let first = {
            let mut n = self.n.lock().unwrap();
            *n += 1;
            *n == 1
        };
        let events = if first {
            let mut evs: Vec<ProviderEvent> = self
                .calls
                .iter()
                .cloned()
                .map(ProviderEvent::ToolCall)
                .collect();
            evs.push(ProviderEvent::Done);
            evs
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

/// A permission policy with one command rule.
pub fn rule_policy(
    workdir: &Path,
    pattern: &str,
    decision: hone_config::Decision,
    id: &str,
) -> hone_core::PolicyConfig {
    use hone_config::policy::{CompiledPolicy, PolicyContext, PolicyRule};
    let context = PolicyContext::for_platform(workdir.to_path_buf(), None);
    let rules = vec![PolicyRule::command(pattern, decision).with_id(id)];
    let compiled = CompiledPolicy::compile(&context, &[], &rules, &[]).expect("policy compiles");
    hone_core::PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: vec![],
        builtins: false,
    }
}

/// Run one turn of `agent`, answering each `ToolCallPaused` with the next of
/// `answers` through `step_tx`. A prompt with no answer left is a test failure
/// (the agent asked when it should not have). Gives up after 10 s.
pub async fn run_agent_turn(
    agent: &mut hone_core::Agent,
    step_tx: &mpsc::Sender<StepAction>,
    answers: Vec<StepAction>,
) -> Vec<AgentEvent> {
    let mut answers: std::collections::VecDeque<StepAction> = answers.into();
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let mut events = Vec::new();
    {
        let turn = agent.run_turn("go", tx);
        tokio::pin!(turn);
        let drive = async {
            loop {
                tokio::select! {
                    biased;
                    Some(e) = rx.recv() => {
                        if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                            let a = answers
                                .pop_front()
                                .unwrap_or_else(|| panic!("unexpected extra prompt: {e:?}"));
                            step_tx.send(a).await.unwrap();
                        }
                        events.push(e);
                    }
                    r = &mut turn => break r,
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), drive)
            .await
            .expect("turn hung")
            .expect("turn should succeed");
    }
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}
