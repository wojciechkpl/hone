//! Shared harness for the review-round-1 eval tests (`docs/plans/eval-profiles-spec.md`,
//! F12-F23): a scripted provider, real file tools, an optional slow tool, a builder seam
//! and small fixtures. No model is called.
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig};
use hone_eval::tasks::TasksFile;
use hone_eval::{AgentBuild, AgentBuilder, EvalOptions, EvalRunner};
use hone_proto::{
    Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, TokenUsage,
    ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{ReadFileTool, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry, WriteFileTool};
use serde_json::{Value, json};

pub struct Script {
    rounds: Mutex<Vec<Vec<ToolCallRequest>>>,
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
            display_name: "t".to_owned(),
            context_length: 128_000,
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
        if round.as_ref().is_some_and(|calls| calls.iter().any(|c| c.name == "__fail__")) {
            return Err(ProviderError::Http {
                provider: "script".to_owned(),
                status: 400,
                body: "scripted provider failure".to_owned(),
            });
        }
        let mut events: Vec<ProviderEvent> = match round {
            Some(calls) => calls.into_iter().map(ProviderEvent::ToolCall).collect(),
            None => vec![ProviderEvent::Token {
                text: "done".to_owned(),
            }],
        };
        events.push(ProviderEvent::Usage(TokenUsage {
            input: 100,
            output: 20,
            cache_read: 0,
            cache_creation: 0,
        }));
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

/// A tool that sleeps (so a timeout lands while it runs).
pub struct SlowTool;

#[async_trait]
impl Tool for SlowTool {
    fn name(&self) -> &str {
        "slow"
    }
    fn description(&self) -> &str {
        "sleeps"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(ToolOutput::new(json!("late")))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

/// A tool that writes `text` to the ABSOLUTE `path` (so a test agent can reach outside its copy).
pub struct TamperTool;

#[async_trait]
impl Tool for TamperTool {
    fn name(&self) -> &str {
        "tamper"
    }
    fn description(&self) -> &str {
        "writes a file anywhere"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        let path = i.0["path"].as_str().unwrap_or_default().to_owned();
        let text = i.0["text"].as_str().unwrap_or_default().to_owned();
        std::fs::write(&path, text).map_err(|e| ToolError::Other(e.to_string()))?;
        Ok(ToolOutput::new(json!("written")))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

/// Polls (up to 30 s) until `path` exists, or, with `unless_equals`, until its content differs
/// from that text; then sleeps `sleep_after_ms`. Lets scripted agents of parallel runs be
/// ordered without sleeps.
pub struct WaitTool;

#[async_trait]
impl Tool for WaitTool {
    fn name(&self) -> &str {
        "waitfor"
    }
    fn description(&self) -> &str {
        "waits for a file"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        let path = i.0["path"].as_str().unwrap_or_default().to_owned();
        let unless = i.0["unless_equals"].as_str().map(str::to_owned);
        let after = i.0["sleep_after_ms"].as_u64().unwrap_or(0);
        for _ in 0..600 {
            let ready = match (&unless, std::fs::read_to_string(&path)) {
                (Some(orig), Ok(now)) => &now != orig,
                (None, Ok(_)) => true,
                _ => false,
            };
            if ready {
                tokio::time::sleep(Duration::from_millis(after)).await;
                return Ok(ToolOutput::new(json!("ready")));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(ToolError::Other("waited 30 s".to_owned()))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

pub fn call(id: &str, name: &str, args: Value) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: args,
    }
}

pub fn agent_for(b: &AgentBuild, rounds: Vec<Vec<ToolCallRequest>>) -> Agent {
    let tools = ToolRegistry::builder()
        .register(Box::new(ReadFileTool))
        .register(Box::new(WriteFileTool))
        .register(Box::new(SlowTool))
        .register(Box::new(TamperTool))
        .register(Box::new(WaitTool))
        .build();
    Agent::new(AgentConfig {
        provider: Arc::new(Script {
            rounds: Mutex::new(rounds),
            n: AtomicUsize::new(0),
        }),
        tools,
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "eval".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 1.0,
            output_per_million: 2.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: b.workdir.clone(),
        memory_store: Some(b.memory.clone()),
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
    })
}

/// A builder whose agents just answer (no tool calls), counting the runs it built.
pub fn quiet_builder(built: Arc<Mutex<Vec<String>>>) -> AgentBuilder {
    Arc::new(move |b: AgentBuild| {
        built.lock().unwrap().push(b.task.id.clone());
        Ok(agent_for(&b, vec![]))
    })
}

pub struct Fx {
    pub _tmp: tempfile::TempDir,
    /// The tasks file's directory.
    pub base: PathBuf,
    pub out: PathBuf,
}

pub fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (base, out) = (root.join("evals"), root.join("out"));
    std::fs::create_dir_all(&base).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    Fx {
        _tmp: tmp,
        base,
        out,
    }
}

pub fn task_toml(id: &str, dir: &str, check: &str) -> String {
    format!(
        "[[tasks]]\nid = \"{id}\"\nprompt = \"p\"\nsuccess_check = {check:?}\nworking_dir = \"{dir}\"\ntimeout_secs = 30\n\n"
    )
}

pub fn runner(f: &Fx, raw: &str, b: AgentBuilder) -> EvalRunner {
    let tasks = TasksFile::from_toml_str("tasks.toml", raw).unwrap_or_else(|e| panic!("{e}"));
    EvalRunner::with_builder(tasks, f.base.clone(), "mock:default", b)
}

pub fn opts(f: &Fx) -> EvalOptions {
    EvalOptions {
        out_dir: f.out.clone(),
        ..Default::default()
    }
}

pub fn read_json(p: &Path) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

pub fn run_dir_of(run: &Path, profile: &str, task: &str, k: u32) -> PathBuf {
    run.join(profile).join(task).join(format!("r{k}"))
}

pub fn outcome(run: &Path, task: &str) -> Value {
    read_json(&run_dir_of(run, "default", task, 1).join("outcome.json"))
}

/// Run `git` in `dir` with a fixed identity; panics on failure; returns stdout (trimmed).
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}
