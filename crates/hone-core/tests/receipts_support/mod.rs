//! Shared harness for the context-receipt tests: a scripted provider that records
//! every `CompletionRequest` it receives (and how many `ContextAssembled` events
//! had been sent when each request arrived), fake tools with long tagged output,
//! and an agent builder. Compiles against today's API only (no receipt types).
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig, SkeletonStrategy, SkillRegistry};
use hone_index::SqliteIndex;
use hone_memory::{Embedder, MemoryStore};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub const SYSTEM_PROMPT: &str = "You are a test assistant.";
pub const MODEL: &str = "mock:test";

/// A long plain-text tool result, unique per `tag`; `reps` scales its size.
pub fn long_output(tag: &str, reps: usize) -> String {
    format!("{tag} {}", "lorem ipsum dolor sit amet ".repeat(reps))
}

pub struct FakeTool {
    pub name: &'static str,
    pub read_only: bool,
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
        let tag = input.0["tag"].as_str().unwrap_or("none").to_owned();
        let reps = input.0["reps"].as_u64().unwrap_or(20) as usize;
        Ok(ToolOutput::new(json!(long_output(&tag, reps))))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
}

pub fn call(id: &str, name: &str, args: Value) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: args,
    }
}

/// A `read_file` call whose result carries `tag`.
pub fn read(tag: &str) -> ToolCallRequest {
    call(tag, "read_file", json!({ "tag": tag }))
}

/// Plays the n-th round (tool calls) for the n-th `complete()` of the whole
/// session; after the rounds run out every call answers with text.
pub struct Script {
    rounds: Mutex<Vec<Vec<ToolCallRequest>>>,
    n: AtomicUsize,
    pub requests: Mutex<Vec<CompletionRequest>>,
    rx: Mutex<Option<mpsc::Receiver<AgentEvent>>>,
    log: Mutex<Vec<AgentEvent>>,
    /// For each request of the current turn: the number of `ContextAssembled`
    /// events the agent had sent by the time the request arrived.
    pub assembled_before_request: Mutex<Vec<usize>>,
}

impl Script {
    pub fn new(rounds: Vec<Vec<ToolCallRequest>>) -> Arc<Self> {
        Arc::new(Self {
            rounds: Mutex::new(rounds),
            n: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            rx: Mutex::new(None),
            log: Mutex::new(Vec::new()),
            assembled_before_request: Mutex::new(Vec::new()),
        })
    }

    fn drain(&self) {
        let mut rx = self.rx.lock().unwrap();
        if let Some(rx) = rx.as_mut() {
            while let Ok(e) = rx.try_recv() {
                self.log.lock().unwrap().push(e);
            }
        }
    }

    pub fn request(&self, i: usize) -> CompletionRequest {
        self.requests.lock().unwrap()[i].clone()
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
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
            id: ModelId::new(MODEL),
            display_name: "Test".to_owned(),
            context_length: 128_000,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        self.drain();
        let seen = self
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| format!("{e:?}").starts_with("ContextAssembled"))
            .count();
        self.assembled_before_request.lock().unwrap().push(seen);
        self.requests.lock().unwrap().push(r);
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let round = self.rounds.lock().unwrap().get(i).cloned();
        let mut events: Vec<ProviderEvent> = match round {
            Some(calls) => calls.into_iter().map(ProviderEvent::ToolCall).collect(),
            None => vec![ProviderEvent::Token {
                text: format!("answer {i}"),
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

#[derive(Default)]
pub struct Opts {
    pub memory: Option<Arc<dyn MemoryStore>>,
    pub embedder: Option<Arc<dyn Embedder>>,
    pub skills: Option<Arc<SkillRegistry>>,
    pub index: Option<Arc<SqliteIndex>>,
    pub inject: bool,
    pub strategy: SkeletonStrategy,
    pub working_dir: Option<std::path::PathBuf>,
    pub hooks: Option<Arc<hone_core::hooks::HookRegistry>>,
    pub policy: Option<hone_core::PolicyConfig>,
    pub egress_log: Option<Arc<tokio::sync::Mutex<hone_core::EgressLog>>>,
}

pub struct Rig {
    pub provider: Arc<Script>,
    pub agent: Agent,
}

pub fn config(provider: Arc<Script>, o: Opts) -> AgentConfig {
    let mut b = ToolRegistry::builder();
    for (name, read_only) in [
        ("read_file", true),
        ("edit_file", false),
        ("apply_patch", false),
        ("shell", false),
    ] {
        b = b.register(Box::new(FakeTool { name, read_only }));
    }
    AgentConfig {
        provider,
        tools: b.build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: SYSTEM_PROMPT.to_owned(),
        model: ModelId::new(MODEL),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new(MODEL),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: o.working_dir.unwrap_or_else(std::env::temp_dir),
        memory_store: o.memory,
        retry_config: RetryConfig::default(),
        index: o.index,
        inject_context: o.inject,
        skeleton_strategy: o.strategy,
        egress_log: o.egress_log,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: o.hooks,
        skills: o.skills,
        embedder: o.embedder,
        mode: AgentMode::Build,
        policy: o.policy,
    }
}

pub fn rig(rounds: Vec<Vec<ToolCallRequest>>, o: Opts) -> Rig {
    let provider = Script::new(rounds);
    let agent = Agent::new(config(provider.clone(), o));
    Rig { provider, agent }
}

/// Like `rig`, but a spawned `AgentHandle` (what the CLI and server hold).
pub fn handle(rounds: Vec<Vec<ToolCallRequest>>, o: Opts) -> (Arc<Script>, hone_core::AgentHandle) {
    let provider = Script::new(rounds);
    let h = hone_core::AgentHandle::spawn(config(provider.clone(), o));
    (provider, h)
}

impl Rig {
    /// Run one turn; returns every event of that turn, in order.
    pub async fn turn(&mut self, input: &str) -> Vec<AgentEvent> {
        let (tx, rx) = mpsc::channel::<AgentEvent>(1024);
        self.provider.log.lock().unwrap().clear();
        self.provider
            .assembled_before_request
            .lock()
            .unwrap()
            .clear();
        *self.provider.rx.lock().unwrap() = Some(rx);
        tokio::time::timeout(Duration::from_secs(30), self.agent.run_turn(input, tx))
            .await
            .expect("turn hung")
            .expect("turn failed");
        self.provider.drain();
        std::mem::take(&mut *self.provider.log.lock().unwrap())
    }
}
