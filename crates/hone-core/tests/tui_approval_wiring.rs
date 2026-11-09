#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Red-phase tests for `docs/plans/tui-approval-fix-spec.md` rows 1-5.
//!
//! Contract:
//! - `AgentHandle::set_step_channel(Option<Arc<tokio::sync::Mutex<mpsc::Receiver<StepAction>>>>)`
//!   hands a step channel to an already-running agent (FIFO with other commands; `None` clears it).
//! - `AgentMode::AutoApprove` skips the `ToolApprovalPolicy` pause but never the permission policy.
//!
//! Tests 4 and 5 deliberately give the agent its channel through `AgentConfig::step_rx` so they
//! only depend on existing APIs.
//!
//! Every test is bounded by a timeout so a missing pause cannot hang the suite.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, PolicyContext, PolicyRule, Subject};
use hone_core::{AgentConfig, AgentHandle, AgentMode, PolicyConfig, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StepAction, ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{
    SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry, ToolSubjects,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

type StepRx = Arc<tokio::sync::Mutex<mpsc::Receiver<StepAction>>>;

// -- fakes ----------------------------------------------------------------------------------------

/// Writes `target` when called, so a test can assert on the filesystem.
struct FsTool {
    name: &'static str,
    read_only: bool,
    target: PathBuf,
    subjects: Vec<Subject>,
}

#[async_trait]
impl Tool for FsTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "writes a marker file"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        std::fs::write(&self.target, b"written").unwrap();
        Ok(ToolOutput::new(json!({ "ok": true })))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn subjects(&self, _input: &ToolInput, _s: &SubjectContext) -> ToolSubjects {
        ToolSubjects {
            subjects: self.subjects.clone(),
            ..Default::default()
        }
    }
}

/// Call 1: one tool call. Call 2+: text "done".
struct OneCallThenDone {
    first: Vec<ProviderEvent>,
    n: Mutex<usize>,
}

impl OneCallThenDone {
    fn new(tool: &str) -> Arc<Self> {
        Arc::new(Self {
            first: vec![
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "c1".to_owned(),
                    name: tool.to_owned(),
                    arguments: json!({}),
                }),
                ProviderEvent::Done,
            ],
            n: Mutex::new(0),
        })
    }
}

#[async_trait]
impl ModelProvider for OneCallThenDone {
    fn id(&self) -> &str {
        "one-call"
    }
    fn display_name(&self) -> &str {
        "One call"
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
        let i = {
            let mut n = self.n.lock().unwrap();
            let i = *n;
            *n += 1;
            i
        };
        let events = if i == 0 {
            self.first.clone()
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

// -- harness --------------------------------------------------------------------------------------

fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    (tmp, dir)
}

fn policy_with(workdir: &Path, user: Vec<PolicyRule>) -> PolicyConfig {
    let context = PolicyContext::for_platform(workdir.to_path_buf(), None);
    let compiled = CompiledPolicy::compile(&context, &[], &user, &[]).expect("compiles");
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: Vec::new(),
        builtins: false,
    }
}

fn deploy_subject() -> Vec<Subject> {
    vec![Subject::Command("deploy prod".to_owned())]
}

fn config(
    tool: FsTool,
    workdir: &Path,
    approval_policy: ToolApprovalPolicy,
    step_rx: Option<StepRx>,
    policy: Option<PolicyConfig>,
) -> AgentConfig {
    let tool_name = tool.name;
    AgentConfig {
        provider: OneCallThenDone::new(tool_name),
        tools: ToolRegistry::builder().register(Box::new(tool)).build(),
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
        working_dir: workdir.to_path_buf(),
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
        policy,
    }
}

fn channel() -> (mpsc::Sender<StepAction>, StepRx) {
    let (tx, rx) = mpsc::channel(8);
    (tx, Arc::new(tokio::sync::Mutex::new(rx)))
}

fn write_tool(dir: &Path, subjects: Vec<Subject>) -> (FsTool, PathBuf) {
    let target = dir.join("out.txt");
    (
        FsTool {
            name: "mock_write",
            read_only: false,
            target: target.clone(),
            subjects,
        },
        target,
    )
}

/// Run one turn on `handle`. Each `ToolCallPaused` is answered with the next entry of `replies`
/// via `step_tx` (a panic if the agent pauses more often than scripted). Returns every event.
async fn drive(
    handle: &AgentHandle,
    step_tx: &mpsc::Sender<StepAction>,
    mut replies: Vec<StepAction>,
) -> Vec<AgentEvent> {
    replies.reverse();
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let h = handle.clone();
    let turn = tokio::spawn(async move { h.run_turn("go", tx).await });
    let mut events = Vec::new();
    let collect = async {
        while let Some(e) = rx.recv().await {
            if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                let reply = replies
                    .pop()
                    .unwrap_or_else(|| panic!("unexpected extra pause; events: {events:?}"));
                step_tx.send(reply).await.unwrap();
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
        .expect("turn hung: the agent never paused/answered as expected");
    let _ = tokio::time::timeout(Duration::from_secs(5), turn).await;
    events
}

fn count(events: &[AgentEvent], f: impl Fn(&AgentEvent) -> bool) -> usize {
    events.iter().filter(|e| f(e)).count()
}

fn paused(events: &[AgentEvent]) -> usize {
    count(events, |e| matches!(e, AgentEvent::ToolCallPaused { .. }))
}

fn approval_required(events: &[AgentEvent]) -> usize {
    count(events, |e| matches!(e, AgentEvent::ApprovalRequired { .. }))
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

// -- 1 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_set_step_channel_when_agent_spawned_auto_and_askrisky_set_should_pause_and_deny_stops_tool()
 {
    let (_t, dir) = canonical_tmp();
    let (tool, target) = write_tool(&dir, vec![]);
    let handle = AgentHandle::spawn(config(tool, &dir, ToolApprovalPolicy::Auto, None, None));

    let (step_tx, rx) = channel();
    handle
        .set_approval_policy(ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        })
        .await;
    handle.set_step_channel(Some(rx)).await;

    let events = drive(&handle, &step_tx, vec![StepAction::Deny("nope".to_owned())]).await;

    assert_eq!(paused(&events), 1, "write tool must pause: {events:?}");
    assert!(!target.exists(), "denied write must not touch the filesystem");
    let (result, is_error) = end_of(&events, "c1");
    assert!(is_error);
    assert_eq!(result["error"], "denied by user", "{result}");
}

// -- 2 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_set_step_channel_when_policy_ask_rule_and_channel_set_later_should_prompt_and_proceed_runs()
 {
    let (_t, dir) = canonical_tmp();
    let (tool, target) = write_tool(&dir, deploy_subject());
    let p = policy_with(
        &dir,
        vec![PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
    );
    let handle = AgentHandle::spawn(config(tool, &dir, ToolApprovalPolicy::Auto, None, Some(p)));

    let (step_tx, rx) = channel();
    handle.set_step_channel(Some(rx)).await;

    let events = drive(&handle, &step_tx, vec![StepAction::Proceed]).await;

    assert_eq!(approval_required(&events), 1, "{events:?}");
    assert!(target.exists(), "approved call must run the tool");
    let (result, is_error) = end_of(&events, "c1");
    assert!(!is_error, "{result}");
}

// -- 3 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_set_step_channel_when_cleared_with_none_should_fail_closed_on_policy_ask() {
    let (_t, dir) = canonical_tmp();
    let (tool, target) = write_tool(&dir, deploy_subject());
    let p = policy_with(
        &dir,
        vec![PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
    );
    let handle = AgentHandle::spawn(config(tool, &dir, ToolApprovalPolicy::Auto, None, Some(p)));

    let (step_tx, rx) = channel();
    handle.set_step_channel(Some(rx)).await;
    handle.set_step_channel(None).await;

    let events = drive(&handle, &step_tx, vec![]).await;

    assert_eq!(approval_required(&events), 0, "{events:?}");
    assert!(!target.exists());
    let (result, is_error) = end_of(&events, "c1");
    assert!(is_error);
    assert_eq!(result["error"], "requires approval", "{result}");
    assert_eq!(result["rule"], "ask-deploy");
}

// -- 4 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_auto_approve_mode_when_askrisky_and_channel_should_run_write_tool_without_pause() {
    let (_t, dir) = canonical_tmp();
    let (tool, target) = write_tool(&dir, vec![]);
    let (step_tx, rx) = channel();
    // Pre-load a Deny: if the agent wrongly pauses, it consumes this and the write is blocked,
    // so the failure shows up as an assertion instead of a hang.
    step_tx
        .send(StepAction::Deny("auto-approve must not ask".to_owned()))
        .await
        .unwrap();
    let handle = AgentHandle::spawn(config(
        tool,
        &dir,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
        Some(rx),
        None,
    ));
    handle.set_mode(AgentMode::AutoApprove).await;

    let events = drive(&handle, &step_tx, vec![]).await;

    assert_eq!(paused(&events), 0, "AutoApprove must not pause: {events:?}");
    assert!(target.exists(), "write tool must have run");
    let (_, is_error) = end_of(&events, "c1");
    assert!(!is_error);
}

// -- 5 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_auto_approve_mode_when_policy_ask_rule_should_still_prompt() {
    let (_t, dir) = canonical_tmp();
    let (tool, target) = write_tool(&dir, deploy_subject());
    let p = policy_with(
        &dir,
        vec![PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
    );
    let (step_tx, rx) = channel();
    let handle = AgentHandle::spawn(config(
        tool,
        &dir,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
        Some(rx),
        Some(p),
    ));
    handle.set_mode(AgentMode::AutoApprove).await;

    // Exactly one prompt (the policy's); the approval-policy pause must not add a second.
    let events = drive(&handle, &step_tx, vec![StepAction::Deny("no".to_owned())]).await;

    assert_eq!(approval_required(&events), 1, "{events:?}");
    assert_eq!(paused(&events), 1, "{events:?}");
    assert!(!target.exists(), "denied at the policy prompt");
}

#[tokio::test]
async fn test_auto_approve_mode_when_policy_deny_rule_should_still_refuse() {
    let (_t, dir) = canonical_tmp();
    let (tool, target) = write_tool(&dir, deploy_subject());
    let p = policy_with(
        &dir,
        vec![PolicyRule::command("deploy*", Decision::Deny).with_id("deny-deploy")],
    );
    let (step_tx, rx) = channel();
    let handle = AgentHandle::spawn(config(
        tool,
        &dir,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
        Some(rx),
        Some(p),
    ));
    handle.set_mode(AgentMode::AutoApprove).await;

    let events = drive(&handle, &step_tx, vec![]).await;

    assert_eq!(paused(&events), 0, "{events:?}");
    assert!(!target.exists());
    let (result, is_error) = end_of(&events, "c1");
    assert!(is_error);
    assert_eq!(result["error"], "denied by policy", "{result}");
}
