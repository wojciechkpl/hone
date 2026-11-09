#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 agent-loop behaviour (`docs/plans/policy-p2.5-spec.md` sections 6-10):
//! tool-level asks, "always" on command rules, the read filter handed to tools, the gate
//! running before `PreToolUse` hooks, and secret redaction of tool output.
//!
//! API assumptions:
//! - `hone_core::PolicyConfig` is `hone_tools::policy_gate::PolicyConfig` with `builtins: bool`.
//! - The redaction note is appended to the model-visible tool result as
//!   `[hone: redacted N secret-shaped value(s) from this output]`; it is asserted on the
//!   conversation transcript (Debug rendering of the messages), not on the `ToolCallEnd` event.
//! - The `ToolCallEnd.result` JSON keeps its shape: every string leaf is redacted, other
//!   values are untouched.
//! - Hooks are shell commands; the marker-file hook below follows the format used by
//!   `policy_gate.rs` (`{"deny":false,"message":null,"mutated_input":null}`).
//! - A hook that rewrites a call's input is still gated on the rewritten input (the existing
//!   `test_agent_policy_when_hook_mutates_input_should_evaluate_mutated_input`); this is not
//!   stated in the spec, see the report.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, PolicyContext, PolicyRule, Subject, builtin_rules};
use hone_core::hooks::{HookRegistry, HookSpec};
use hone_core::{Agent, AgentConfig, AgentMode, PolicyConfig, RetryConfig};
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

// -- fakes ----------------------------------------------------------------------------------------

type SubjFn = Arc<dyn Fn(&ToolInput) -> Vec<Subject> + Send + Sync>;
type OutFn = Arc<dyn Fn(&ToolInput, &ToolContext) -> Value + Send + Sync>;

struct FakeTool {
    name: &'static str,
    read_only: bool,
    ask: Option<&'static str>,
    subj: SubjFn,
    out: OutFn,
    calls: Arc<AtomicUsize>,
}

impl FakeTool {
    fn new(name: &'static str, read_only: bool) -> Self {
        Self {
            name,
            read_only,
            ask: None,
            subj: Arc::new(|_| vec![]),
            out: Arc::new(|_, _| json!({ "ok": true })),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn subjects_from(mut self, f: SubjFn) -> Self {
        self.subj = f;
        self
    }
    fn fixed(self, subjects: Vec<Subject>) -> Self {
        self.subjects_from(Arc::new(move |_| subjects.clone()))
    }
    fn asks(mut self, id: &'static str) -> Self {
        self.ask = Some(id);
        self
    }
    fn output(mut self, f: OutFn) -> Self {
        self.out = f;
        self
    }
    fn counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.calls)
    }
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::new((self.out)(&input, ctx)))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn subjects(&self, input: &ToolInput, _s: &SubjectContext) -> ToolSubjects {
        ToolSubjects {
            subjects: (self.subj)(input),
            ..Default::default()
        }
    }
    fn builtin_ask(&self) -> Option<&'static str> {
        self.ask
    }
}

struct Scripted {
    sequences: Vec<Vec<ProviderEvent>>,
    n: Mutex<usize>,
}

impl Scripted {
    fn calls_then_done(calls: Vec<ToolCallRequest>) -> Arc<Self> {
        let mut first: Vec<ProviderEvent> =
            calls.into_iter().map(ProviderEvent::ToolCall).collect();
        first.push(ProviderEvent::Done);
        let second = vec![
            ProviderEvent::Token {
                text: "done".to_owned(),
            },
            ProviderEvent::Done,
        ];
        Arc::new(Self {
            sequences: vec![first, second],
            n: Mutex::new(0),
        })
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    fn id(&self) -> &str {
        "scripted"
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
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let idx = {
            let mut n = self.n.lock().unwrap();
            let i = *n;
            *n += 1;
            i.min(self.sequences.len() - 1)
        };
        Ok(Box::pin(stream::iter(self.sequences[idx].clone())))
    }
    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        10
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

// -- harness --------------------------------------------------------------------------------------

fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    (tmp, dir)
}

/// A policy whose Builtin layer is `builtin_rules()` when `builtins`, plus `user` rules.
fn policy(
    workdir: &Path,
    builtins: bool,
    user: Vec<PolicyRule>,
    project: Vec<PolicyRule>,
) -> PolicyConfig {
    let context = PolicyContext::for_platform(workdir.to_path_buf(), None);
    let b = if builtins {
        builtin_rules()
    } else {
        Vec::new()
    };
    let compiled = CompiledPolicy::compile(&context, &b, &user, &project).expect("compiles");
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: Vec::new(),
        builtins,
    }
}

fn config(
    provider: Arc<dyn ModelProvider>,
    tool: FakeTool,
    workdir: &Path,
    policy: Option<PolicyConfig>,
) -> AgentConfig {
    AgentConfig {
        provider,
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
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy,
    }
}

type StepRx = Option<Arc<tokio::sync::Mutex<mpsc::Receiver<StepAction>>>>;

fn step_channel(actions: Vec<StepAction>) -> (StepRx, mpsc::Sender<StepAction>) {
    let (tx, rx) = mpsc::channel(16);
    install_feeder(&tx, actions, false);
    (Some(Arc::new(tokio::sync::Mutex::new(rx))), tx)
}

// The agent drops answers buffered before a prompt (stale-answer guard), so
// scripted answers are delivered when the matching `ToolCallPaused` appears.
thread_local! {
    static FEEDER: std::cell::RefCell<Option<Feeder>> = const { std::cell::RefCell::new(None) };
}

struct Feeder {
    queue: std::collections::VecDeque<StepAction>,
    /// `Some` = held strongly (dropped after the last answer to close the channel);
    /// otherwise only a weak handle, so the test's own sender controls closing.
    strong: Option<mpsc::Sender<StepAction>>,
    weak: mpsc::WeakSender<StepAction>,
}

fn install_feeder(tx: &mpsc::Sender<StepAction>, actions: Vec<StepAction>, hold: bool) {
    FEEDER.with(|f| {
        *f.borrow_mut() = Some(Feeder {
            queue: actions.into(),
            strong: hold.then(|| tx.clone()),
            weak: tx.downgrade(),
        });
    });
}

fn feed_on_prompt(e: &AgentEvent) {
    if !matches!(e, AgentEvent::ToolCallPaused { .. }) {
        return;
    }
    FEEDER.with(|f| {
        let mut f = f.borrow_mut();
        let Some(fd) = f.as_mut() else { return };
        if let Some(a) = fd.queue.pop_front() {
            if let Some(t) = fd.weak.upgrade() {
                let _ = t.try_send(a);
            }
        }
        if fd.queue.is_empty() && fd.strong.is_some() {
            fd.strong = None;
        }
    });
}

/// Drive `agent.run_turn`, answering prompts from the feeder; bounded by a timeout.
async fn run_turn_feeding(agent: &mut Agent) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::channel(256);
    let mut events = Vec::new();
    {
        let turn = agent.run_turn("go", tx);
        tokio::pin!(turn);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    biased;
                    Some(e) = rx.recv() => {
                        feed_on_prompt(&e);
                        events.push(e);
                    }
                    r = &mut turn => break r,
                }
            }
        })
        .await
        .expect("turn hung (unexpected approval prompt?)")
        .expect("turn should succeed");
    }
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

struct Outcome {
    events: Vec<AgentEvent>,
    transcript: String,
}

async fn run(cfg: AgentConfig) -> Outcome {
    let mut agent = Agent::new(cfg);
    let events = run_turn_feeding(&mut agent).await;
    let transcript = agent
        .messages()
        .iter()
        .map(|m| format!("{m:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    Outcome { events, transcript }
}

fn paused(o: &Outcome) -> usize {
    o.events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallPaused { .. }))
        .count()
}

fn end_of(o: &Outcome, id: &str) -> (Value, bool) {
    o.events
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
        .unwrap_or_else(|| panic!("no ToolCallEnd for {id}: {:?}", o.events))
}

// -- tool-level asks ------------------------------------------------------------------------------

#[tokio::test]
async fn tool_level_ask_blocks_a_headless_call_when_builtins_are_on() {
    let (_t, dir) = canonical_tmp();
    let tool = FakeTool::new("clicker", false).asks("builtin:computer-control");
    let count = tool.counter();
    let p = policy(&dir, true, vec![], vec![]);
    let prov = Scripted::calls_then_done(vec![call("c1", "clicker", json!({}))]);

    let o = run(config(prov, tool, &dir, Some(p))).await;

    assert_eq!(count.load(Ordering::SeqCst), 0);
    let (result, is_error) = end_of(&o, "c1");
    assert!(is_error);
    assert_eq!(
        result,
        json!({ "error": "requires approval", "rule": "builtin:computer-control" })
    );
}

#[tokio::test]
async fn tool_level_ask_does_not_apply_when_builtins_are_off() {
    let (_t, dir) = canonical_tmp();
    let tool = FakeTool::new("clicker", false).asks("builtin:computer-control");
    let count = tool.counter();
    // a non-empty policy without built-ins
    let p = policy(
        &dir,
        false,
        vec![PolicyRule::command("rm *", Decision::Deny)],
        vec![],
    );
    let prov = Scripted::calls_then_done(vec![call("c1", "clicker", json!({}))]);

    let o = run(config(prov, tool, &dir, Some(p))).await;

    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(!end_of(&o, "c1").1);
}

#[tokio::test]
async fn approve_always_on_a_tool_level_ask_is_remembered_by_id() {
    let (_t, dir) = canonical_tmp();
    let tool = FakeTool::new("clicker", false).asks("builtin:computer-control");
    let count = tool.counter();
    let p = policy(&dir, true, vec![], vec![]);
    let prov = Scripted::calls_then_done(vec![
        call("c1", "clicker", json!({ "x": 1 })),
        call("c2", "clicker", json!({ "x": 2 })),
        call("c3", "clicker", json!({ "x": 3 })),
    ]);
    let (rx, _keep) = step_channel(vec![StepAction::ApproveAlways("clicker".to_owned())]);
    let mut cfg = config(prov, tool, &dir, Some(p));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(paused(&o), 1, "one prompt; later clicks reuse the approval");
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn pre_approved_tool_level_ask_runs_without_a_prompt() {
    let (_t, dir) = canonical_tmp();
    let tool = FakeTool::new("clicker", false).asks("builtin:computer-control");
    let count = tool.counter();
    let mut p = policy(&dir, true, vec![], vec![]);
    p.pre_approved = vec!["builtin:computer-control".to_owned()];
    let prov = Scripted::calls_then_done(vec![call("c1", "clicker", json!({}))]);

    let o = run(config(prov, tool, &dir, Some(p))).await;

    assert_eq!(paused(&o), 0);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

// -- "always" on command rules -----------------------------------------------------------------------

fn deploy_subjects() -> SubjFn {
    Arc::new(|input: &ToolInput| {
        input
            .get_str("target")
            .map(|t| Subject::Command(format!("deploy {t}")))
            .into_iter()
            .collect()
    })
}

#[tokio::test]
async fn approve_always_on_a_literal_command_rule_does_not_cover_a_different_command_line() {
    let (_t, dir) = canonical_tmp();
    let tool = FakeTool::new("deployer", false).subjects_from(deploy_subjects());
    let count = tool.counter();
    let p = policy(
        &dir,
        false,
        vec![PolicyRule::command("deploy", Decision::Ask).with_id("ask-deploy")],
        vec![],
    );
    let prov = Scripted::calls_then_done(vec![
        call("c1", "deployer", json!({ "target": "prod" })),
        call("c2", "deployer", json!({ "target": "--force prod" })),
    ]);
    let (rx, _keep) = step_channel(vec![
        StepAction::ApproveAlways("deployer".to_owned()),
        StepAction::Deny("no".to_owned()),
    ]);
    let mut cfg = config(prov, tool, &dir, Some(p));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(paused(&o), 2, "the second command line must prompt again");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(!end_of(&o, "c1").1);
    assert!(end_of(&o, "c2").1);
}

// -- read filter handed to tools -----------------------------------------------------------------------

/// A tool that reports what the agent put in `ToolContext::read_filter`.
fn probe(workdir: &Path) -> FakeTool {
    let wd = workdir.to_path_buf();
    FakeTool::new("probe", true).output(Arc::new(move |_i, ctx| match &ctx.read_filter {
        None => json!({ "has_filter": false }),
        Some(f) => {
            let name = |p: PathBuf| match f(&p) {
                hone_tools::policy_gate::ReadAccess::Allowed => "allowed",
                hone_tools::policy_gate::ReadAccess::NeedsApproval => "needs-approval",
                hone_tools::policy_gate::ReadAccess::Denied => "denied",
            };
            json!({
                "has_filter": true,
                "secret": name(wd.join("secret/k.txt")),
                "src": name(wd.join("src/a.txt")),
                "env": name(wd.join(".env")),
            })
        }
    }))
}

fn deny_secret() -> PolicyRule {
    PolicyRule::path(
        "secret/**",
        hone_config::policy::OpFilter::Read,
        Decision::Deny,
    )
    .with_id("no-secret")
}

#[tokio::test]
async fn agent_hands_tools_a_read_filter_when_the_policy_has_path_rules() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, false, vec![], vec![deny_secret()]);
    let prov = Scripted::calls_then_done(vec![call("c1", "probe", json!({}))]);

    let o = run(config(prov, probe(&dir), &dir, Some(p))).await;

    let (result, is_error) = end_of(&o, "c1");
    assert!(!is_error, "{result}");
    assert_eq!(result["has_filter"], true);
    assert_eq!(result["secret"], "denied");
    assert_eq!(result["src"], "allowed");
    assert_eq!(result["env"], "allowed", "no built-ins, no .env rule");
}

#[tokio::test]
async fn agent_read_filter_includes_the_builtin_env_rule_when_builtins_are_on() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, true, vec![], vec![deny_secret()]);
    let prov = Scripted::calls_then_done(vec![call("c1", "probe", json!({}))]);

    let o = run(config(prov, probe(&dir), &dir, Some(p))).await;

    let (result, _) = end_of(&o, "c1");
    assert_eq!(result["has_filter"], true);
    assert_eq!(result["env"], "needs-approval");
    assert_eq!(result["secret"], "denied");
}

#[tokio::test]
async fn agent_hands_no_read_filter_without_a_policy() {
    let (_t, dir) = canonical_tmp();
    let prov = Scripted::calls_then_done(vec![call("c1", "probe", json!({}))]);
    let o = run(config(prov, probe(&dir), &dir, None)).await;
    assert_eq!(end_of(&o, "c1").0, json!({ "has_filter": false }));
}

#[tokio::test]
async fn agent_hands_no_read_filter_when_the_policy_has_only_command_rules() {
    let (_t, dir) = canonical_tmp();
    let p = policy(
        &dir,
        false,
        vec![PolicyRule::command("rm *", Decision::Deny)],
        vec![],
    );
    let prov = Scripted::calls_then_done(vec![call("c1", "probe", json!({}))]);
    let o = run(config(prov, probe(&dir), &dir, Some(p))).await;
    assert_eq!(end_of(&o, "c1").0, json!({ "has_filter": false }));
}

// -- the gate runs before PreToolUse hooks ----------------------------------------------------------------

fn marker_hook(marker: &Path, tool: &str) -> Arc<HookRegistry> {
    let mut hooks = HookRegistry::new();
    hooks.register(HookSpec {
        name: "marker".to_owned(),
        event: "PreToolUse".to_owned(),
        command: format!(
            "touch '{}' && cat > /dev/null && echo '{{\"deny\":false,\"message\":null,\"mutated_input\":null}}'",
            marker.display()
        ),
        timeout_ms: 5000,
        tool_name_pattern: Some(tool.to_owned()),
    });
    Arc::new(hooks)
}

async fn hook_run(
    read_only: bool,
    subjects: Vec<Subject>,
    user_rules: Vec<PolicyRule>,
) -> (bool, Arc<AtomicUsize>, Outcome) {
    let (_t, dir) = canonical_tmp();
    let marker = dir.join("hook-ran.marker");
    let tool = FakeTool::new("acts", read_only).fixed(subjects);
    let count = tool.counter();
    let p = policy(&dir, false, user_rules, vec![]);
    let prov = Scripted::calls_then_done(vec![call("c1", "acts", json!({}))]);
    let mut cfg = config(prov, tool, &dir, Some(p));
    cfg.hooks = Some(marker_hook(&marker, "acts"));
    let o = run(cfg).await;
    (marker.exists(), count, o)
}

#[tokio::test]
async fn denied_call_never_reaches_a_pre_tool_use_hook_in_both_dispatch_paths() {
    for read_only in [false, true] {
        let (hook_ran, count, o) = hook_run(
            read_only,
            vec![Subject::Command("git push origin".to_owned())],
            vec![PolicyRule::command("git push*", Decision::Deny).with_id("no-push")],
        )
        .await;
        assert!(
            !hook_ran,
            "hook ran for a denied call (read_only={read_only})"
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let (result, is_error) = end_of(&o, "c1");
        assert!(is_error);
        assert_eq!(
            result,
            json!({ "error": "denied by policy", "rule": "no-push" })
        );
    }
}

#[tokio::test]
async fn unapproved_ask_never_reaches_a_pre_tool_use_hook_in_both_dispatch_paths() {
    for read_only in [false, true] {
        let (hook_ran, count, o) = hook_run(
            read_only,
            vec![Subject::Command("deploy prod".to_owned())],
            vec![PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
        )
        .await;
        assert!(
            !hook_ran,
            "hook ran for an unapproved ask (read_only={read_only})"
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(end_of(&o, "c1").1);
    }
}

#[tokio::test]
async fn allowed_call_still_runs_the_hook_in_both_dispatch_paths() {
    for read_only in [false, true] {
        let (hook_ran, count, o) = hook_run(
            read_only,
            vec![Subject::Command("ls".to_owned())],
            vec![PolicyRule::command("git push*", Decision::Deny).with_id("no-push")],
        )
        .await;
        assert!(
            hook_ran,
            "hook must run for an allowed call (read_only={read_only})"
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!end_of(&o, "c1").1);
    }
}

#[tokio::test]
async fn approved_ask_runs_the_hook_after_the_prompt() {
    for read_only in [false, true] {
        let (_t, dir) = canonical_tmp();
        let marker = dir.join("hook-ran.marker");
        let tool = FakeTool::new("acts", read_only)
            .fixed(vec![Subject::Command("deploy prod".to_owned())]);
        let count = tool.counter();
        let p = policy(
            &dir,
            false,
            vec![PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
            vec![],
        );
        let prov = Scripted::calls_then_done(vec![call("c1", "acts", json!({}))]);
        let (rx, _keep) = step_channel(vec![StepAction::Proceed]);
        let mut cfg = config(prov, tool, &dir, Some(p));
        cfg.hooks = Some(marker_hook(&marker, "acts"));
        cfg.step_rx = rx;

        let o = run(cfg).await;

        assert_eq!(paused(&o), 1);
        assert!(marker.exists(), "read_only={read_only}");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}

// -- redaction -----------------------------------------------------------------------------------------------

const AWS: &str = "AKIAIOSFODNN7EXAMPLE";
const PW_LINE: &str = "password=hunter2hunter2";

fn leaky(read_only: bool) -> FakeTool {
    FakeTool::new("leaky", read_only).output(Arc::new(|_, _| {
        json!({
            "stdout": format!("key {AWS} found"),
            "nested": { "list": [PW_LINE, 5, true, null] },
            "n": 7,
        })
    }))
}

#[tokio::test]
async fn builtins_redact_tool_results_in_the_context_and_in_tool_call_end() {
    for read_only in [false, true] {
        let (_t, dir) = canonical_tmp();
        let p = policy(&dir, true, vec![], vec![]);
        let prov = Scripted::calls_then_done(vec![call("c1", "leaky", json!({}))]);

        let o = run(config(prov, leaky(read_only), &dir, Some(p))).await;

        let (result, is_error) = end_of(&o, "c1");
        assert!(!is_error, "{result}");
        // event: every string leaf redacted, shape and non-strings preserved
        assert_eq!(
            result["stdout"], "key [REDACTED:aws-key] found",
            "read_only={read_only}"
        );
        assert_eq!(
            result["nested"]["list"],
            json!(["password=[REDACTED:secret-assignment]", 5, true, null])
        );
        assert_eq!(result["n"], 7);
        // model context
        assert!(!o.transcript.contains(AWS), "{}", o.transcript);
        assert!(!o.transcript.contains("hunter2hunter2"), "{}", o.transcript);
        assert!(o.transcript.contains("[REDACTED:aws-key]"));
        assert!(
            o.transcript
                .contains("[hone: redacted 2 secret-shaped value(s) from this output]"),
            "{}",
            o.transcript
        );
    }
}

#[tokio::test]
async fn the_note_is_absent_when_nothing_was_redacted() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, true, vec![], vec![]);
    let tool = FakeTool::new("clean", false).output(Arc::new(|_, _| json!({ "stdout": "hello" })));
    let prov = Scripted::calls_then_done(vec![call("c1", "clean", json!({}))]);
    let o = run(config(prov, tool, &dir, Some(p))).await;
    assert_eq!(end_of(&o, "c1").0, json!({ "stdout": "hello" }));
    assert!(
        !o.transcript.contains("[hone: redacted"),
        "{}",
        o.transcript
    );
}

#[tokio::test]
async fn without_builtins_results_are_not_redacted() {
    for read_only in [false, true] {
        let (_t, dir) = canonical_tmp();
        let p = policy(
            &dir,
            false,
            vec![PolicyRule::command("rm *", Decision::Deny)],
            vec![],
        );
        let prov = Scripted::calls_then_done(vec![call("c1", "leaky", json!({}))]);

        let o = run(config(prov, leaky(read_only), &dir, Some(p))).await;

        let (result, _) = end_of(&o, "c1");
        assert_eq!(result["stdout"], format!("key {AWS} found"));
        assert!(o.transcript.contains(AWS));
        assert!(o.transcript.contains("hunter2hunter2"));
        assert!(!o.transcript.contains("[hone: redacted"));
    }
}

#[tokio::test]
async fn without_a_policy_results_are_not_redacted() {
    let (_t, dir) = canonical_tmp();
    let prov = Scripted::calls_then_done(vec![call("c1", "leaky", json!({}))]);
    let o = run(config(prov, leaky(false), &dir, None)).await;
    assert_eq!(end_of(&o, "c1").0["stdout"], format!("key {AWS} found"));
    assert!(o.transcript.contains(AWS));
}

#[tokio::test]
async fn redaction_does_not_touch_tool_arguments() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, true, vec![], vec![]);
    let tool = FakeTool::new("clean", false).output(Arc::new(|_, _| json!({ "ok": true })));
    let prov = Scripted::calls_then_done(vec![call("c1", "clean", json!({ "note": AWS }))]);
    let o = run(config(prov, tool, &dir, Some(p))).await;
    assert!(
        o.transcript.contains(AWS),
        "arguments are the model's own output and stay as sent: {}",
        o.transcript
    );
}

// -- 14.10 one approval covers one rule ----------------------------------------------------------------

/// Policy with a home dir, so `~/.bashrc` is a shell-startup write.
fn policy_with_home(workdir: &Path, home: &Path) -> PolicyConfig {
    let context = PolicyContext::for_platform(workdir.to_path_buf(), Some(home.to_path_buf()));
    let compiled = CompiledPolicy::compile(&context, &builtin_rules(), &[], &[]).expect("compiles");
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: Vec::new(),
        builtins: true,
    }
}

/// A tool whose single call would both push and append to the shell rc file.
fn push_and_rc(home: &Path) -> FakeTool {
    FakeTool::new("combo", false).fixed(vec![
        Subject::Command("git push origin main".to_owned()),
        Subject::Path {
            path: home.join(".bashrc"),
            op: hone_config::policy::PathOp::Write,
        },
    ])
}

fn paused_rules(o: &Outcome) -> Vec<String> {
    o.events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalRequired { description, .. } => Some(description.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn approve_once_covers_only_the_asked_rule_and_prompts_for_the_next() {
    let (_t, dir) = canonical_tmp();
    let (_h, home) = canonical_tmp();
    let tool = push_and_rc(&home);
    let count = tool.counter();
    let prov = Scripted::calls_then_done(vec![call("c1", "combo", json!({}))]);
    let (rx, _keep) = step_channel(vec![StepAction::Proceed, StepAction::Deny("no".to_owned())]);
    let mut cfg = config(prov, tool, &dir, Some(policy_with_home(&dir, &home)));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(paused(&o), 2, "git-push first, then shell-startup");
    let rules = paused_rules(&o);
    assert!(rules[0].contains("builtin:git-push"), "{rules:?}");
    assert!(rules[1].contains("builtin:shell-startup"), "{rules:?}");
    assert_eq!(count.load(Ordering::SeqCst), 0, "the second ask was denied");
    assert!(end_of(&o, "c1").1);
}

#[tokio::test]
async fn approving_every_ask_once_runs_the_call() {
    let (_t, dir) = canonical_tmp();
    let (_h, home) = canonical_tmp();
    let tool = push_and_rc(&home);
    let count = tool.counter();
    let prov = Scripted::calls_then_done(vec![call("c1", "combo", json!({}))]);
    let (rx, _keep) = step_channel(vec![StepAction::Proceed, StepAction::Proceed]);
    let mut cfg = config(prov, tool, &dir, Some(policy_with_home(&dir, &home)));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(paused(&o), 2);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_once_approval_is_not_remembered_for_the_next_call() {
    let (_t, dir) = canonical_tmp();
    let tool = FakeTool::new("pusher", false)
        .fixed(vec![Subject::Command("git push origin main".to_owned())]);
    let count = tool.counter();
    let prov = Scripted::calls_then_done(vec![
        call("c1", "pusher", json!({})),
        call("c2", "pusher", json!({})),
    ]);
    let (rx, _keep) = step_channel(vec![StepAction::Proceed, StepAction::Deny("no".to_owned())]);
    let mut cfg = config(prov, tool, &dir, Some(policy(&dir, true, vec![], vec![])));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(paused(&o), 2, "each call asks again");
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn headless_run_fails_closed_on_the_first_ask() {
    let (_t, dir) = canonical_tmp();
    let (_h, home) = canonical_tmp();
    let tool = push_and_rc(&home);
    let count = tool.counter();
    let prov = Scripted::calls_then_done(vec![call("c1", "combo", json!({}))]);
    let o = run(config(
        prov,
        tool,
        &dir,
        Some(policy_with_home(&dir, &home)),
    ))
    .await;
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(
        end_of(&o, "c1").0,
        json!({ "error": "requires approval", "rule": "builtin:git-push" })
    );
}

// -- 14.12 the other ways out -----------------------------------------------------------------------------

fn read_audit(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn egress(dir: &Path) -> (Arc<tokio::sync::Mutex<hone_core::EgressLog>>, PathBuf) {
    let path = dir.join("audit.jsonl");
    (
        Arc::new(tokio::sync::Mutex::new(hone_core::EgressLog::new(
            path.clone(),
        ))),
        path,
    )
}

async fn run_with_egress(builtins: bool) -> (String, String) {
    let (_t, dir) = canonical_tmp();
    let (log, path) = egress(&dir);
    let p = policy(
        &dir,
        builtins,
        vec![PolicyRule::command("rm *", Decision::Deny)],
        vec![],
    );
    let prov = Scripted::calls_then_done(vec![call("c1", "leaky", json!({}))]);
    let mut cfg = config(prov, leaky(false), &dir, Some(p));
    cfg.egress_log = Some(Arc::clone(&log));
    let o = run(cfg).await;
    let _ = log.lock().await.flush();
    (read_audit(&path), o.transcript)
}

#[tokio::test]
async fn egress_log_previews_are_redacted_when_builtins_are_on() {
    let (audit, _) = run_with_egress(true).await;
    assert!(
        audit.contains("leaky"),
        "the tool call must be logged: {audit}"
    );
    assert!(!audit.contains(AWS), "{audit}");
    assert!(!audit.contains("hunter2hunter2"), "{audit}");
}

#[tokio::test]
async fn egress_log_previews_are_unchanged_when_builtins_are_off() {
    let (audit, _) = run_with_egress(false).await;
    assert!(audit.contains(AWS), "{audit}");
}

async fn run_with_post_hook(builtins: bool) -> String {
    let (_t, dir) = canonical_tmp();
    let capture = dir.join("post-payload.json");
    let mut hooks = HookRegistry::new();
    hooks.register(HookSpec {
        name: "capture".to_owned(),
        event: "PostToolUse".to_owned(),
        command: format!(
            "cat > '{}' && echo '{{\"deny\":false,\"message\":null,\"mutated_input\":null}}'",
            capture.display()
        ),
        timeout_ms: 5000,
        tool_name_pattern: Some("leaky".to_owned()),
    });
    let p = policy(
        &dir,
        builtins,
        vec![PolicyRule::command("rm *", Decision::Deny)],
        vec![],
    );
    let prov = Scripted::calls_then_done(vec![call("c1", "leaky", json!({}))]);
    let mut cfg = config(prov, leaky(false), &dir, Some(p));
    cfg.hooks = Some(Arc::new(hooks));
    let _ = run(cfg).await;
    std::fs::read_to_string(&capture).unwrap_or_default()
}

#[tokio::test]
async fn post_tool_use_hook_payload_is_redacted_when_builtins_are_on() {
    let payload = run_with_post_hook(true).await;
    assert!(
        payload.contains("PostToolUse"),
        "hook must have run: {payload}"
    );
    assert!(!payload.contains(AWS), "{payload}");
    assert!(!payload.contains("hunter2hunter2"), "{payload}");
}

#[tokio::test]
async fn post_tool_use_hook_payload_is_unchanged_when_builtins_are_off() {
    let payload = run_with_post_hook(false).await;
    assert!(payload.contains(AWS), "{payload}");
}

async fn run_with_injection(builtins: bool) -> String {
    let (_t, dir) = canonical_tmp();
    // a source file whose NAME is secret-shaped: it appears in the injected repo skeleton
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join(format!("src/{AWS}.rs")), "pub fn leaked() {}\n").unwrap();
    let p = policy(
        &dir,
        builtins,
        vec![PolicyRule::command("rm *", Decision::Deny)],
        vec![],
    );
    let prov = Scripted::calls_then_done(vec![]);
    let mut cfg = config(prov, FakeTool::new("noop", false), &dir, Some(p));
    cfg.inject_context = true;
    run(cfg).await.transcript
}

#[tokio::test]
async fn context_injection_text_is_redacted_when_builtins_are_on() {
    let transcript = run_with_injection(true).await;
    assert!(transcript.contains("Repository Structure"), "{transcript}");
    assert!(!transcript.contains(AWS), "{transcript}");
}

#[tokio::test]
async fn context_injection_text_is_unchanged_when_builtins_are_off() {
    let transcript = run_with_injection(false).await;
    assert!(transcript.contains("Repository Structure"), "{transcript}");
    assert!(
        transcript.contains(AWS),
        "injection must have included the file name: {transcript}"
    );
}
