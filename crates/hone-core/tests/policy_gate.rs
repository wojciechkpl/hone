//! P2.2 contract tests: the policy decision point (`docs/plans/policy-p2.2-spec.md`
//! section 6 and the agent-loop items of section 8).
//!
//! Tools are fakes that override `Tool::subjects` with fixed subjects, so these
//! tests do not depend on real subject extraction (sections 3-5).
//!
//! Interface assumptions (spec leaves them open):
//! - `hone_tools` exports `ToolSubjects { subjects, unparsed }` and `SubjectContext`
//!   at its crate root next to `Tool`.
//! - `PolicyContext::for_platform(workdir: PathBuf, home: Option<PathBuf>)`.
//! - `GateDecision` is `Debug`; tests destructure it rather than require `PartialEq`.
//! - `decide` does not consult `session_approved` for the synthetic
//!   `builtin:unparsed-command` rule (spec step 2 returns `Ask` directly); that
//!   case is deliberately not asserted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, OpFilter, PathOp, PolicyContext, PolicyRule, Subject};
use hone_core::egress_log::EgressLog;
use hone_core::hooks::{HookRegistry, HookSpec};
use hone_core::{Agent, AgentConfig, AgentMode, GateDecision, PolicyConfig, RetryConfig, decide};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StepAction, ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::policy_gate::approval_key;
use hone_tools::{
    SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry, ToolSubjects,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

type SubjFn = Arc<dyn Fn(&ToolInput) -> ToolSubjects + Send + Sync>;

struct FakeTool {
    name: &'static str,
    read_only: bool,
    subj: SubjFn,
    calls: Arc<AtomicUsize>,
}

impl FakeTool {
    fn new(name: &'static str, read_only: bool, subj: SubjFn) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                name,
                read_only,
                subj,
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }

    /// A tool whose every call touches the given fixed subjects.
    fn fixed(
        name: &'static str,
        read_only: bool,
        subjects: Vec<Subject>,
    ) -> (Self, Arc<AtomicUsize>) {
        Self::new(
            name,
            read_only,
            Arc::new(move |_| ToolSubjects {
                subjects: subjects.clone(),
                unparsed: false,
                ..Default::default()
            }),
        )
    }

    /// A tool whose calls report an unparseable shell command.
    fn unparsed(name: &'static str) -> (Self, Arc<AtomicUsize>) {
        Self::new(
            name,
            false,
            Arc::new(|_| ToolSubjects {
                subjects: vec![],
                unparsed: true,
                ..Default::default()
            }),
        )
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
    async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::new(json!({ "ok": true })))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn subjects(&self, input: &ToolInput, _sctx: &SubjectContext) -> ToolSubjects {
        (self.subj)(input)
    }
}

/// Provider that replays scripted event sequences; the last one repeats.
struct Scripted {
    sequences: Vec<Vec<ProviderEvent>>,
    n: Mutex<usize>,
}

impl Scripted {
    /// First model turn issues `calls`; the second answers "done".
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

// ---------------------------------------------------------------------------
// Policy helpers
// ---------------------------------------------------------------------------

fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    (tmp, dir)
}

fn policy(workdir: &Path, rules: Vec<PolicyRule>, pre_approved: &[&str]) -> PolicyConfig {
    let context = PolicyContext::for_platform(workdir.to_path_buf(), None);
    let compiled = CompiledPolicy::compile(&context, &[], &rules, &[]).expect("policy compiles");
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: pre_approved.iter().map(|s| (*s).to_owned()).collect(),
        builtins: false,
    }
}

fn cmd(s: &str) -> Subject {
    Subject::Command(s.to_owned())
}

fn deny_rule(pattern: &str, id: &str) -> PolicyRule {
    PolicyRule::command(pattern, Decision::Deny).with_id(id)
}

fn ask_rule(pattern: &str, id: &str) -> PolicyRule {
    PolicyRule::command(pattern, Decision::Ask).with_id(id)
}

/// Flatten a decision for comparison.
fn flat(d: GateDecision) -> (&'static str, Option<String>) {
    match d {
        GateDecision::Allow => ("allow", None),
        GateDecision::Deny { rule_id } => ("deny", Some(rule_id)),
        GateDecision::Ask { rule_id } => ("ask", Some(rule_id)),
    }
}

fn decide_for(
    p: &PolicyConfig,
    approved: &[&str],
    tool: Option<&FakeTool>,
    c: &ToolCallRequest,
) -> (&'static str, Option<String>) {
    let set: HashSet<String> = approved.iter().map(|s| (*s).to_owned()).collect();
    flat(decide(p, &set, tool.map(|t| t as &dyn Tool), c))
}

fn some(s: &str) -> Option<String> {
    Some(s.to_owned())
}

// ---------------------------------------------------------------------------
// decide(): pure function
// ---------------------------------------------------------------------------

#[test]
fn test_decide_when_no_rule_matches_should_allow() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("ls -la")]);
    let c = call("1", "t", json!({}));
    assert_eq!(decide_for(&p, &[], Some(&tool), &c), ("allow", None));
}

#[test]
fn test_decide_when_deny_rule_matches_should_deny_with_rule_id() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("git push origin main")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &[], Some(&tool), &c),
        ("deny", some("no-push"))
    );
}

#[test]
fn test_decide_when_ask_rule_matches_should_ask_with_rule_id() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("deploy prod")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &[], Some(&tool), &c),
        ("ask", some("ask-deploy"))
    );
}

#[test]
fn test_decide_when_path_deny_rule_matches_should_deny() {
    let (_t, dir) = canonical_tmp();
    let rule = PolicyRule::path("secret.txt", OpFilter::Read, Decision::Deny).with_id("no-secret");
    let p = policy(&dir, vec![rule], &[]);
    let (tool, _) = FakeTool::fixed(
        "t",
        true,
        vec![Subject::Path {
            path: dir.join("secret.txt"),
            op: PathOp::Read,
        }],
    );
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &[], Some(&tool), &c),
        ("deny", some("no-secret"))
    );
}

#[test]
fn test_decide_when_tool_unknown_should_allow_even_with_deny_everything() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![deny_rule("*", "deny-all")], &[]);
    let c = call("1", "no_such_tool", json!({}));
    assert_eq!(decide_for(&p, &[], None, &c), ("allow", None));
}

#[test]
fn test_decide_when_tool_has_no_subjects_should_allow() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![deny_rule("*", "deny-all")], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![]);
    let c = call("1", "t", json!({}));
    assert_eq!(decide_for(&p, &[], Some(&tool), &c), ("allow", None));
}

#[test]
fn test_decide_when_strictest_of_many_subjects_should_win() {
    let (_t, dir) = canonical_tmp();
    let p = policy(
        &dir,
        vec![
            ask_rule("deploy*", "ask-deploy"),
            deny_rule("rm *", "no-rm"),
        ],
        &[],
    );
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("deploy prod"), cmd("rm -rf x")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &[], Some(&tool), &c),
        ("deny", some("no-rm"))
    );
}

#[test]
fn test_decide_when_unparsed_and_policy_has_command_rules_should_ask_unparsed_command() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &[]);
    let (tool, _) = FakeTool::unparsed("t");
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &[], Some(&tool), &c),
        ("ask", some("builtin:unparsed-command"))
    );
}

#[test]
fn test_decide_when_unparsed_and_policy_has_only_path_rules_should_ask_unparsed_command() {
    let (_t, dir) = canonical_tmp();
    // A path-only policy still catches commands hidden behind a parse error.
    let rule = PolicyRule::path("secret.txt", OpFilter::Read, Decision::Deny).with_id("no-secret");
    let p = policy(&dir, vec![rule], &[]);
    let (tool, _) = FakeTool::unparsed("t");
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &[], Some(&tool), &c),
        ("ask", some("builtin:unparsed-command"))
    );
}

#[test]
fn test_decide_when_empty_policy_and_unparsed_should_allow() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![], &[]);
    let (tool, _) = FakeTool::unparsed("t");
    let c = call("1", "t", json!({}));
    assert_eq!(decide_for(&p, &[], Some(&tool), &c), ("allow", None));
}

#[test]
fn test_decide_when_literal_host_ask_rule_session_approved_by_id_should_allow() {
    // P2.5 section 7: only host and path rules are approved by bare id.
    let (_t, dir) = canonical_tmp();
    let rule = PolicyRule::host("example.com", Decision::Ask).with_id("ask-host");
    let p = policy(&dir, vec![rule], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![Subject::Host("example.com".to_owned())]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &["ask-host"], Some(&tool), &c),
        ("allow", None)
    );
}

#[test]
fn test_decide_when_literal_command_ask_rule_session_approved_by_id_should_still_ask() {
    // P2.5 section 7: a command rule is a prefix match, so a bare id never approves it.
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![ask_rule("deploy prod", "ask-deploy")], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("deploy prod")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &["ask-deploy"], Some(&tool), &c),
        ("ask", some("ask-deploy"))
    );
    let key = approval_key("ask-deploy", &cmd("deploy prod"));
    assert_eq!(
        decide_for(&p, &[key.as_str()], Some(&tool), &c),
        ("allow", None)
    );
}

#[test]
fn test_decide_when_wildcard_ask_rule_session_approved_by_subject_key_should_allow() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("deploy prod")]);
    let c = call("1", "t", json!({}));
    let key = approval_key("ask-deploy", &cmd("deploy prod"));
    assert_eq!(
        decide_for(&p, &[key.as_str()], Some(&tool), &c),
        ("allow", None)
    );
}

#[test]
fn test_decide_when_wildcard_ask_rule_session_approved_by_bare_id_should_still_ask() {
    // Spec 7: a wildcard rule is never approved by its bare id.
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("deploy prod")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &["ask-deploy"], Some(&tool), &c),
        ("ask", some("ask-deploy"))
    );
}

#[test]
fn test_decide_when_wildcard_ask_rule_approved_for_other_subject_should_still_ask() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("deploy staging")]);
    let c = call("1", "t", json!({}));
    let other = approval_key("ask-deploy", &cmd("deploy prod"));
    assert_eq!(
        decide_for(&p, &[other.as_str()], Some(&tool), &c),
        ("ask", some("ask-deploy"))
    );
}

#[test]
fn test_decide_when_ask_rule_pre_approved_should_allow() {
    let (_t, dir) = canonical_tmp();
    let p = policy(
        &dir,
        vec![ask_rule("deploy*", "ask-deploy")],
        &["ask-deploy"],
    );
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("deploy prod")]);
    let c = call("1", "t", json!({}));
    assert_eq!(decide_for(&p, &[], Some(&tool), &c), ("allow", None));
}

#[test]
fn test_decide_when_approved_id_is_for_a_different_rule_should_still_ask() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &["other"]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("deploy prod")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &["unrelated"], Some(&tool), &c),
        ("ask", some("ask-deploy"))
    );
}

#[test]
fn test_decide_when_deny_rule_id_is_approved_should_still_deny() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &["no-push"]);
    let (tool, _) = FakeTool::fixed("t", false, vec![cmd("git push")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &["no-push"], Some(&tool), &c),
        ("deny", some("no-push"))
    );
}

// ---------------------------------------------------------------------------
// Agent loop helpers
// ---------------------------------------------------------------------------

fn config(
    provider: Arc<dyn ModelProvider>,
    tools: ToolRegistry,
    workdir: &Path,
    policy: Option<PolicyConfig>,
) -> AgentConfig {
    AgentConfig {
        provider,
        tools,
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

/// Returns the receiver for `AgentConfig::step_rx` and, when `keep_open`, the
/// sender (drop it to simulate a closed channel).
fn step_channel(
    actions: Vec<StepAction>,
    keep_open: bool,
) -> (StepRx, Option<mpsc::Sender<StepAction>>) {
    let (tx, rx) = mpsc::channel(16);
    if keep_open || !actions.is_empty() {
        install_feeder(&tx, actions, !keep_open);
    }
    (
        Some(Arc::new(tokio::sync::Mutex::new(rx))),
        if keep_open { Some(tx) } else { None },
    )
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
    /// Debug rendering of the model-visible conversation.
    transcript: String,
}

/// Runs one turn with a hang guard (a wrongly-prompting agent would block on
/// the open step channel forever).
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

fn starts(o: &Outcome, id: &str) -> usize {
    o.events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallStart { id: i, .. } if i == id))
        .count()
}

fn paused(o: &Outcome) -> usize {
    o.events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallPaused { .. }))
        .count()
}

fn approval_required(o: &Outcome) -> Vec<(String, String)> {
    o.events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalRequired {
                action,
                description,
                ..
            } => Some((action.clone(), description.clone())),
            _ => None,
        })
        .collect()
}

/// `(result, is_error)` of the ToolCallEnd for `id`.
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

fn registry(tool: FakeTool) -> ToolRegistry {
    ToolRegistry::builder().register(Box::new(tool)).build()
}

// ---------------------------------------------------------------------------
// Deny: sequential and parallel paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_deny_sequential_should_not_dispatch_and_report_denied_by_policy() {
    let (_t, dir) = canonical_tmp();
    let (tool, calls) = FakeTool::fixed("pusher", false, vec![cmd("git push origin")]);
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "pusher", json!({}))]);

    let o = run(config(prov, registry(tool), &dir, Some(p))).await;

    assert_eq!(calls.load(Ordering::SeqCst), 0, "denied tool must not run");
    assert_eq!(starts(&o, "c1"), 0, "no ToolCallStart for a denied call");
    let (result, is_error) = end_of(&o, "c1");
    assert!(is_error);
    assert_eq!(
        result,
        json!({ "error": "denied by policy", "rule": "no-push" })
    );
    assert!(
        o.transcript.contains("denied by policy: rule no-push"),
        "model must see the denial: {}",
        o.transcript
    );
}

#[tokio::test]
async fn test_agent_deny_parallel_read_only_should_not_dispatch_and_report_denied_by_policy() {
    let (_t, dir) = canonical_tmp();
    let (tool, calls) = FakeTool::fixed("peek", true, vec![cmd("peek secret")]);
    let p = policy(&dir, vec![deny_rule("peek*", "no-peek")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "peek", json!({}))]);

    let o = run(config(prov, registry(tool), &dir, Some(p))).await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(starts(&o, "c1"), 0);
    let (result, is_error) = end_of(&o, "c1");
    assert!(is_error);
    assert_eq!(
        result,
        json!({ "error": "denied by policy", "rule": "no-peek" })
    );
    assert!(o.transcript.contains("denied by policy: rule no-peek"));
}

#[tokio::test]
async fn test_agent_deny_parallel_batch_should_only_block_the_denied_call() {
    let (_t, dir) = canonical_tmp();
    let (ok, ok_calls) = FakeTool::fixed("peek_ok", true, vec![cmd("peek fine")]);
    let (bad, bad_calls) = FakeTool::fixed("peek_bad", true, vec![cmd("peek secret")]);
    let tools = ToolRegistry::builder()
        .register(Box::new(ok))
        .register(Box::new(bad))
        .build();
    let p = policy(&dir, vec![deny_rule("peek secret", "no-secret-peek")], &[]);
    let prov = Scripted::calls_then_done(vec![
        call("a", "peek_ok", json!({})),
        call("b", "peek_bad", json!({})),
    ]);

    let o = run(config(prov, tools, &dir, Some(p))).await;

    assert_eq!(ok_calls.load(Ordering::SeqCst), 1);
    assert_eq!(bad_calls.load(Ordering::SeqCst), 0);
    assert!(!end_of(&o, "a").1);
    assert!(end_of(&o, "b").1);
}

#[tokio::test]
async fn test_agent_deny_should_not_prompt_even_when_interactive() {
    let (_t, dir) = canonical_tmp();
    let (tool, calls) = FakeTool::fixed("pusher", false, vec![cmd("git push")]);
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "pusher", json!({}))]);
    let (rx, _keep) = step_channel(vec![], true);
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(paused(&o), 0);
    assert!(approval_required(&o).is_empty());
    assert_eq!(end_of(&o, "c1").0["error"], "denied by policy");
}

// ---------------------------------------------------------------------------
// Ask: headless
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_ask_headless_sequential_should_fail_closed_with_rule_id() {
    let (_t, dir) = canonical_tmp();
    let (tool, calls) = FakeTool::fixed("deployer", false, vec![cmd("deploy prod")]);
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "deployer", json!({}))]);

    let o = run(config(prov, registry(tool), &dir, Some(p))).await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(starts(&o, "c1"), 0);
    assert_eq!(paused(&o), 0, "headless runs never pause");
    let (result, is_error) = end_of(&o, "c1");
    assert!(is_error);
    assert_eq!(
        result,
        json!({ "error": "requires approval", "rule": "ask-deploy" })
    );
    assert!(
        o.transcript.contains("requires approval: rule ask-deploy"),
        "{}",
        o.transcript
    );
}

#[tokio::test]
async fn test_agent_ask_headless_parallel_should_fail_closed_with_rule_id() {
    let (_t, dir) = canonical_tmp();
    let (tool, calls) = FakeTool::fixed("peek", true, vec![cmd("peek x")]);
    let p = policy(&dir, vec![ask_rule("peek*", "ask-peek")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "peek", json!({}))]);

    let o = run(config(prov, registry(tool), &dir, Some(p))).await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let (result, is_error) = end_of(&o, "c1");
    assert!(is_error);
    assert_eq!(
        result,
        json!({ "error": "requires approval", "rule": "ask-peek" })
    );
}

#[tokio::test]
async fn test_agent_ask_when_rule_pre_approved_should_dispatch_without_prompt() {
    let (_t, dir) = canonical_tmp();
    let (tool, calls) = FakeTool::fixed("deployer", false, vec![cmd("deploy prod")]);
    let p = policy(
        &dir,
        vec![ask_rule("deploy*", "ask-deploy")],
        &["ask-deploy"],
    );
    let prov = Scripted::calls_then_done(vec![call("c1", "deployer", json!({}))]);

    let o = run(config(prov, registry(tool), &dir, Some(p))).await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(paused(&o), 0);
    assert!(!end_of(&o, "c1").1);
}

// ---------------------------------------------------------------------------
// Ask: interactive
// ---------------------------------------------------------------------------

struct Interactive {
    cfg: AgentConfig,
    count: Arc<AtomicUsize>,
    _keep: Option<mpsc::Sender<StepAction>>,
    _tmp: tempfile::TempDir,
}

fn interactive(
    actions: Vec<StepAction>,
    keep_open: bool,
    calls_to_make: Vec<ToolCallRequest>,
) -> Interactive {
    let (tmp, dir) = canonical_tmp();
    let (tool, count) = FakeTool::fixed("deployer", false, vec![cmd("deploy prod")]);
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let prov = Scripted::calls_then_done(calls_to_make);
    let (rx, keep) = step_channel(actions, keep_open);
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.step_rx = rx;
    Interactive {
        cfg,
        count,
        _keep: keep,
        _tmp: tmp,
    }
}

#[tokio::test]
async fn test_agent_ask_interactive_when_proceed_should_prompt_and_dispatch_once() {
    let i = interactive(
        vec![StepAction::Proceed],
        true,
        vec![call("c1", "deployer", json!({}))],
    );
    let o = run(i.cfg.clone()).await;

    assert_eq!(i.count.load(Ordering::SeqCst), 1);
    assert_eq!(paused(&o), 1);
    let ar = approval_required(&o);
    assert_eq!(ar.len(), 1);
    assert_eq!(ar[0].0, "deployer");
    assert_eq!(ar[0].1, "policy rule ask-deploy requires approval");
    assert!(!end_of(&o, "c1").1);
}

#[tokio::test]
async fn test_agent_ask_interactive_when_approve_always_should_not_prompt_second_call() {
    let i = interactive(
        // Only ONE action is queued and the channel stays open: a second
        // prompt would hang and trip the timeout in `run`.
        vec![StepAction::ApproveAlways("deployer".to_owned())],
        true,
        vec![
            call("c1", "deployer", json!({})),
            call("c2", "deployer", json!({})),
        ],
    );
    let o = run(i.cfg.clone()).await;

    assert_eq!(i.count.load(Ordering::SeqCst), 2);
    assert_eq!(paused(&o), 1, "second call hits the remembered rule id");
    assert!(!end_of(&o, "c1").1);
    assert!(!end_of(&o, "c2").1);
}

#[tokio::test]
async fn test_agent_ask_interactive_when_proceed_should_prompt_again_for_next_call() {
    let i = interactive(
        vec![StepAction::Proceed, StepAction::Proceed],
        true,
        vec![
            call("c1", "deployer", json!({})),
            call("c2", "deployer", json!({})),
        ],
    );
    let o = run(i.cfg.clone()).await;

    assert_eq!(i.count.load(Ordering::SeqCst), 2);
    assert_eq!(paused(&o), 2, "Proceed runs once and is not remembered");
}

#[tokio::test]
async fn test_agent_ask_interactive_when_deny_should_report_denied_by_user() {
    let i = interactive(
        vec![StepAction::Deny("nope".to_owned())],
        true,
        vec![call("c1", "deployer", json!({}))],
    );
    let o = run(i.cfg.clone()).await;

    assert_eq!(i.count.load(Ordering::SeqCst), 0);
    let (result, is_error) = end_of(&o, "c1");
    assert!(is_error);
    assert_eq!(result["error"], "denied by user");
    assert!(o.transcript.contains("(denied by user): nope"));
}

#[tokio::test]
async fn test_agent_ask_interactive_when_skip_should_not_dispatch() {
    let i = interactive(
        vec![StepAction::Skip],
        true,
        vec![call("c1", "deployer", json!({}))],
    );
    let o = run(i.cfg.clone()).await;

    assert_eq!(i.count.load(Ordering::SeqCst), 0);
    assert_eq!(end_of(&o, "c1").0, json!({ "skipped": true }));
}

#[tokio::test]
async fn test_agent_ask_interactive_when_channel_closed_should_fail_closed() {
    let i = interactive(vec![], false, vec![call("c1", "deployer", json!({}))]);
    let o = run(i.cfg.clone()).await;

    assert_eq!(i.count.load(Ordering::SeqCst), 0);
    assert_eq!(paused(&o), 1);
    let (result, is_error) = end_of(&o, "c1");
    assert!(is_error);
    assert_eq!(
        result,
        json!({ "error": "requires approval", "rule": "ask-deploy" })
    );
}

#[tokio::test]
async fn test_agent_ask_interactive_parallel_read_only_when_proceed_should_dispatch() {
    let (_t, dir) = canonical_tmp();
    let (tool, count) = FakeTool::fixed("peek", true, vec![cmd("peek x")]);
    let p = policy(&dir, vec![ask_rule("peek*", "ask-peek")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "peek", json!({}))]);
    let (rx, _keep) = step_channel(vec![StepAction::Proceed], true);
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(paused(&o), 1);
    assert_eq!(approval_required(&o).len(), 1);
}

#[tokio::test]
async fn test_agent_policy_approved_call_should_not_be_paused_again_by_ask_risky() {
    let mut i = interactive(
        // One action only: a second (ToolApprovalPolicy) pause would hang.
        vec![StepAction::Proceed],
        true,
        vec![call("c1", "deployer", json!({}))],
    );
    i.cfg.approval_policy = ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    };
    let o = run(i.cfg.clone()).await;

    assert_eq!(i.count.load(Ordering::SeqCst), 1);
    assert_eq!(paused(&o), 1, "policy prompt replaces the AskRisky prompt");
}

// ---------------------------------------------------------------------------
// Guard 1: synthetic read_file before an edit
// ---------------------------------------------------------------------------

struct GuardSetup {
    cfg: AgentConfig,
    edit_calls: Arc<AtomicUsize>,
    read_calls: Arc<AtomicUsize>,
    _tmp: tempfile::TempDir,
}

fn guard_setup(deny_read: bool) -> GuardSetup {
    let (tmp, dir) = canonical_tmp();
    let secret = dir.join("secret.txt");
    let read_subj = Subject::Path {
        path: secret.clone(),
        op: PathOp::Read,
    };
    let write_subj = Subject::Path {
        path: secret.clone(),
        op: PathOp::Write,
    };
    // The edit itself only writes, so a read-only deny rule hits the guard's
    // synthetic read and nothing else.
    let (edit, edit_calls) = FakeTool::fixed("edit_file", false, vec![write_subj]);
    let (read, read_calls) = FakeTool::fixed("read_file", true, vec![read_subj]);
    let tools = ToolRegistry::builder()
        .register(Box::new(edit))
        .register(Box::new(read))
        .build();
    let rules = if deny_read {
        vec![
            PolicyRule::path("secret.txt", OpFilter::Read, Decision::Deny)
                .with_id("no-read-secret"),
        ]
    } else {
        vec![]
    };
    let p = policy(&dir, rules, &[]);
    let prov = Scripted::calls_then_done(vec![call(
        "e1",
        "edit_file",
        json!({ "path": secret.to_string_lossy() }),
    )]);
    GuardSetup {
        cfg: config(prov, tools, &dir, Some(p)),
        edit_calls,
        read_calls,
        _tmp: tmp,
    }
}

#[tokio::test]
async fn test_guard_read_when_policy_denies_path_read_should_skip_synthetic_read() {
    let g = guard_setup(true);
    let _ = run(g.cfg.clone()).await;

    assert_eq!(
        g.read_calls.load(Ordering::SeqCst),
        0,
        "denied read must not be injected"
    );
    assert_eq!(
        g.edit_calls.load(Ordering::SeqCst),
        1,
        "the edit itself is still allowed"
    );
}

#[tokio::test]
async fn test_guard_read_when_policy_allows_path_read_should_still_inject_read() {
    let g = guard_setup(false);
    let _ = run(g.cfg.clone()).await;

    assert_eq!(g.read_calls.load(Ordering::SeqCst), 1);
    assert_eq!(g.edit_calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// PreToolUse hook mutation feeds the policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_policy_when_hook_mutates_input_should_evaluate_mutated_input() {
    let (_t, dir) = canonical_tmp();
    // Subjects depend on the input: `target` becomes a Write path.
    let d2 = dir.clone();
    let (tool, calls) = FakeTool::new(
        "writer",
        false,
        Arc::new(move |input: &ToolInput| ToolSubjects {
            subjects: input
                .get_str("target")
                .map(|t| Subject::Path {
                    path: d2.join(t),
                    op: PathOp::Write,
                })
                .into_iter()
                .collect(),
            unparsed: false,
            ..Default::default()
        }),
    );
    let rule =
        PolicyRule::path("blocked/**", OpFilter::Write, Decision::Deny).with_id("no-blocked");
    let p = policy(&dir, vec![rule], &[]);

    let mut hooks = HookRegistry::new();
    hooks.register(HookSpec {
        name: "rewrite".to_owned(),
        event: "PreToolUse".to_owned(),
        command: r#"cat > /dev/null && echo '{"deny":false,"message":null,"mutated_input":{"target":"blocked/x.txt"}}'"#
            .to_owned(),
        timeout_ms: 5000,
        tool_name_pattern: Some("writer".to_owned()),
    });

    // The model asks for a harmless path; only the hook's rewrite is blocked.
    let prov =
        Scripted::calls_then_done(vec![call("c1", "writer", json!({ "target": "safe.txt" }))]);
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.hooks = Some(Arc::new(hooks));

    let o = run(cfg).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "mutated input hits the deny rule"
    );
    assert_eq!(
        end_of(&o, "c1").0,
        json!({ "error": "denied by policy", "rule": "no-blocked" })
    );
}

// ---------------------------------------------------------------------------
// No policy / empty policy: unchanged behaviour
// ---------------------------------------------------------------------------

/// Event stream reduced to timing-independent fields.
fn shape(o: &Outcome) -> Vec<String> {
    o.events
        .iter()
        .map(|e| match e {
            AgentEvent::ToolCallStart { id, name, .. } => format!("start {id} {name}"),
            AgentEvent::ToolCallEnd {
                id,
                result,
                is_error,
                ..
            } => format!("end {id} {is_error} {result}"),
            AgentEvent::Token { text } => format!("token {text}"),
            other => format!("{other:?}")
                .split([' ', '{', '('])
                .next()
                .unwrap_or_default()
                .to_owned(),
        })
        .collect()
}

async fn simple_run(policy_for: impl FnOnce(&Path) -> Option<PolicyConfig>) -> Outcome {
    let (_t, dir) = canonical_tmp();
    let (a, _) = FakeTool::fixed("writer", false, vec![cmd("git push origin")]);
    let (b, _) = FakeTool::fixed("peek", true, vec![cmd("cat x")]);
    let tools = ToolRegistry::builder()
        .register(Box::new(a))
        .register(Box::new(b))
        .build();
    let prov = Scripted::calls_then_done(vec![
        call("c1", "peek", json!({})),
        call("c2", "writer", json!({})),
    ]);
    run(config(prov, tools, &dir, policy_for(&dir))).await
}

#[tokio::test]
async fn test_agent_policy_none_should_dispatch_everything() {
    let o = simple_run(|_| None).await;
    assert!(!end_of(&o, "c1").1);
    assert!(!end_of(&o, "c2").1);
    assert_eq!(starts(&o, "c2"), 1);
}

#[tokio::test]
async fn test_agent_policy_empty_should_match_policy_none_event_stream() {
    let none = simple_run(|_| None).await;
    let empty = simple_run(|dir| Some(policy(dir, vec![], &[]))).await;
    assert_eq!(shape(&none), shape(&empty));
}

// ---------------------------------------------------------------------------
// Egress log
// ---------------------------------------------------------------------------

async fn blocked_entries(
    rules: Vec<PolicyRule>,
    tool_name: &'static str,
    read_only: bool,
    subject: &str,
) -> Vec<Value> {
    let (_t, dir) = canonical_tmp();
    let (tool, _) = FakeTool::fixed(tool_name, read_only, vec![cmd(subject)]);
    let p = policy(&dir, rules, &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", tool_name, json!({ "k": "v" }))]);
    let log_path = dir.join("audit.jsonl");
    let log = Arc::new(tokio::sync::Mutex::new(EgressLog::new(log_path.clone())));
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.egress_log = Some(Arc::clone(&log));

    let _ = run(cfg).await;
    log.lock().await.flush().unwrap();

    std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["blocked"] == json!(true))
        .collect()
}

#[tokio::test]
async fn test_egress_log_when_policy_denies_should_record_blocked_with_reason() {
    let entries = blocked_entries(
        vec![deny_rule("git push*", "no-push")],
        "pusher",
        false,
        "git push",
    )
    .await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["tool"], "pusher");
    assert_eq!(entries[0]["block_reason"], "policy: deny (rule no-push)");
}

#[tokio::test]
async fn test_egress_log_when_parallel_call_denied_should_record_blocked_with_reason() {
    let entries =
        blocked_entries(vec![deny_rule("peek*", "no-peek")], "peek", true, "peek x").await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["block_reason"], "policy: deny (rule no-peek)");
}

#[tokio::test]
async fn test_egress_log_when_headless_ask_should_record_requires_approval() {
    let entries = blocked_entries(
        vec![ask_rule("deploy*", "ask-deploy")],
        "deployer",
        false,
        "deploy prod",
    )
    .await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        entries[0]["block_reason"],
        "policy: requires approval (rule ask-deploy)"
    );
}

// ---------------------------------------------------------------------------
// Synthetic rule ids (`builtin:unparsed-command`, `builtin:invalid-host`)
// ---------------------------------------------------------------------------

#[test]
fn test_decide_when_unparsed_command_pre_approved_should_allow() {
    let (_t, dir) = canonical_tmp();
    let p = policy(
        &dir,
        vec![deny_rule("git push*", "no-push")],
        &["builtin:unparsed-command"],
    );
    let (tool, _) = FakeTool::unparsed("t");
    let c = call("1", "t", json!({}));
    assert_eq!(decide_for(&p, &[], Some(&tool), &c), ("allow", None));
}

#[tokio::test]
async fn test_agent_unparsed_command_approve_always_should_not_be_remembered() {
    let (_t, dir) = canonical_tmp();
    let (tool, count) = FakeTool::unparsed("runner");
    // A command rule must exist for unparsed input to escalate to Ask.
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &[]);
    let prov = Scripted::calls_then_done(vec![
        call("c1", "runner", json!({})),
        call("c2", "runner", json!({})),
    ]);
    // Two actions queued: ApproveAlways acts as Proceed for synthetic ids, so
    // the second call must prompt again.
    let (rx, _keep) = step_channel(
        vec![
            StepAction::ApproveAlways("runner".to_owned()),
            StepAction::ApproveAlways("runner".to_owned()),
        ],
        true,
    );
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(paused(&o), 2, "synthetic ids are never remembered");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let ar = approval_required(&o);
    assert_eq!(ar.len(), 2);
    assert_eq!(
        ar[0].1,
        "policy rule builtin:unparsed-command requires approval"
    );
}

// ---------------------------------------------------------------------------
// Review round 2: per-subject approval (B2), unparsed vs Deny (B3)
// ---------------------------------------------------------------------------

fn policy_with_home(
    workdir: &Path,
    home: &Path,
    rules: Vec<PolicyRule>,
    pre_approved: &[&str],
) -> PolicyConfig {
    let context = PolicyContext::for_platform(workdir.to_path_buf(), Some(home.to_path_buf()));
    let compiled = CompiledPolicy::compile(&context, &[], &rules, &[]).expect("policy compiles");
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: pre_approved.iter().map(|s| (*s).to_owned()).collect(),
        builtins: false,
    }
}

/// Two ask rules in the user layer; automatic ids are `user:1` and `user:2`.
fn two_ask_rules() -> Vec<PolicyRule> {
    vec![
        PolicyRule::command("git push origin", Decision::Ask),
        PolicyRule::path("~/.vault/k", OpFilter::Read, Decision::Ask),
    ]
}

/// Wildcard twins of `two_ask_rules` (same automatic ids).
fn two_wildcard_ask_rules() -> Vec<PolicyRule> {
    vec![
        PolicyRule::command("git push*", Decision::Ask),
        PolicyRule::path("~/.vault/**", OpFilter::Read, Decision::Ask),
    ]
}

fn push_subject() -> Subject {
    cmd("git push origin")
}

fn vault_subject(home: &Path) -> Subject {
    Subject::Path {
        path: home.join(".vault").join("k"),
        op: PathOp::Read,
    }
}

fn push_and_vault_tool(home: &Path) -> FakeTool {
    FakeTool::fixed(
        "t",
        false,
        vec![
            cmd("git push origin"),
            Subject::Path {
                path: home.join(".vault").join("k"),
                op: PathOp::Read,
            },
        ],
    )
    .0
}

#[test]
fn test_decide_when_one_of_two_ask_rules_session_approved_should_still_ask_the_other() {
    let (_t, dir) = canonical_tmp();
    let (_h, home) = canonical_tmp();
    let p = policy_with_home(&dir, &home, two_ask_rules(), &[]);
    let tool = push_and_vault_tool(&home);
    let c = call("1", "t", json!({}));
    // user:1 is a command rule: approved per (rule, subject), not by bare id (P2.5 section 7)
    let k1 = approval_key("user:1", &push_subject());
    assert_eq!(
        decide_for(&p, &[k1.as_str()], Some(&tool), &c),
        ("ask", some("user:2"))
    );
    assert_eq!(
        decide_for(&p, &["user:2"], Some(&tool), &c),
        ("ask", some("user:1"))
    );
}

#[test]
fn test_decide_when_one_of_two_wildcard_ask_rules_approved_by_key_should_still_ask_the_other() {
    let (_t, dir) = canonical_tmp();
    let (_h, home) = canonical_tmp();
    let p = policy_with_home(&dir, &home, two_wildcard_ask_rules(), &[]);
    let tool = push_and_vault_tool(&home);
    let c = call("1", "t", json!({}));
    let k1 = approval_key("user:1", &push_subject());
    let k2 = approval_key("user:2", &vault_subject(&home));
    assert_eq!(
        decide_for(&p, &[k1.as_str()], Some(&tool), &c),
        ("ask", some("user:2"))
    );
    assert_eq!(
        decide_for(&p, &[k2.as_str()], Some(&tool), &c),
        ("ask", some("user:1"))
    );
}

#[test]
fn test_decide_when_one_of_two_ask_rules_pre_approved_should_still_ask_the_other() {
    let (_t, dir) = canonical_tmp();
    let (_h, home) = canonical_tmp();
    let p = policy_with_home(&dir, &home, two_ask_rules(), &["user:1"]);
    let tool = push_and_vault_tool(&home);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &[], Some(&tool), &c),
        ("ask", some("user:2"))
    );
}

#[test]
fn test_decide_when_both_literal_ask_rules_approved_should_allow() {
    let (_t, dir) = canonical_tmp();
    let (_h, home) = canonical_tmp();
    let p = policy_with_home(&dir, &home, two_ask_rules(), &["user:1"]);
    let tool = push_and_vault_tool(&home);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &["user:2"], Some(&tool), &c),
        ("allow", None)
    );
}

#[test]
fn test_decide_when_both_wildcard_ask_rules_approved_by_subject_keys_should_allow() {
    let (_t, dir) = canonical_tmp();
    let (_h, home) = canonical_tmp();
    let p = policy_with_home(&dir, &home, two_wildcard_ask_rules(), &["user:1"]);
    let tool = push_and_vault_tool(&home);
    let c = call("1", "t", json!({}));
    let k2 = approval_key("user:2", &vault_subject(&home));
    assert_eq!(
        decide_for(&p, &[k2.as_str()], Some(&tool), &c),
        ("allow", None)
    );
}

fn unparsed_with(subjects: Vec<Subject>) -> FakeTool {
    FakeTool::new(
        "t",
        false,
        Arc::new(move |_| ToolSubjects {
            subjects: subjects.clone(),
            unparsed: true,
            ..Default::default()
        }),
    )
    .0
}

#[test]
fn test_decide_when_unparsed_and_parsed_deny_subject_should_deny_not_ask() {
    let (_t, dir) = canonical_tmp();
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &[]);
    let tool = unparsed_with(vec![cmd("git push origin")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &[], Some(&tool), &c),
        ("deny", some("no-push"))
    );
}

#[test]
fn test_decide_when_unparsed_pre_approved_and_parsed_deny_subject_should_still_deny() {
    let (_t, dir) = canonical_tmp();
    let p = policy(
        &dir,
        vec![deny_rule("git push*", "no-push")],
        &["builtin:unparsed-command"],
    );
    let tool = unparsed_with(vec![cmd("git push origin")]);
    let c = call("1", "t", json!({}));
    assert_eq!(
        decide_for(&p, &["builtin:unparsed-command"], Some(&tool), &c),
        ("deny", some("no-push"))
    );
}

// ---------------------------------------------------------------------------
// Review round 2: egress for user deny/skip, bookkeeping, Guard 1 post-hook
// ---------------------------------------------------------------------------

async fn user_blocked_reasons(action: StepAction) -> Vec<Value> {
    let mut i = interactive(vec![action], true, vec![call("c1", "deployer", json!({}))]);
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("audit.jsonl");
    let log = Arc::new(tokio::sync::Mutex::new(EgressLog::new(log_path.clone())));
    i.cfg.egress_log = Some(Arc::clone(&log));

    let _ = run(i.cfg.clone()).await;
    log.lock().await.flush().unwrap();

    std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["blocked"] == json!(true))
        .collect()
}

#[tokio::test]
async fn test_egress_log_when_user_denies_policy_prompt_should_record_denied_by_user() {
    let entries = user_blocked_reasons(StepAction::Deny("no".to_owned())).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        entries[0]["block_reason"],
        "policy: denied by user (rule ask-deploy)"
    );
}

#[tokio::test]
async fn test_egress_log_when_user_skips_policy_prompt_should_record_skipped_by_user() {
    let entries = user_blocked_reasons(StepAction::Skip).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        entries[0]["block_reason"],
        "policy: skipped by user (rule ask-deploy)"
    );
}

/// Like `run`, but returns `TurnResult::tool_calls_count`.
async fn run_counting(cfg: AgentConfig) -> u32 {
    let mut agent = Agent::new(cfg);
    let (tx, _rx) = mpsc::channel(256);
    tokio::time::timeout(Duration::from_secs(10), agent.run_turn("go", tx))
        .await
        .expect("turn hung")
        .expect("turn should succeed")
        .tool_calls_count
}

#[tokio::test]
async fn test_agent_deny_sequential_should_not_count_towards_tool_calls() {
    let (_t, dir) = canonical_tmp();
    let (tool, _) = FakeTool::fixed("pusher", false, vec![cmd("git push")]);
    let p = policy(&dir, vec![deny_rule("git push*", "no-push")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "pusher", json!({}))]);
    assert_eq!(
        run_counting(config(prov, registry(tool), &dir, Some(p))).await,
        0
    );
}

#[tokio::test]
async fn test_agent_deny_parallel_should_not_count_towards_tool_calls() {
    let (_t, dir) = canonical_tmp();
    let (tool, _) = FakeTool::fixed("peek", true, vec![cmd("peek x")]);
    let p = policy(&dir, vec![deny_rule("peek*", "no-peek")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "peek", json!({}))]);
    assert_eq!(
        run_counting(config(prov, registry(tool), &dir, Some(p))).await,
        0
    );
}

#[tokio::test]
async fn test_agent_headless_ask_should_not_count_towards_tool_calls() {
    let (_t, dir) = canonical_tmp();
    let (tool, _) = FakeTool::fixed("deployer", false, vec![cmd("deploy prod")]);
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let prov = Scripted::calls_then_done(vec![call("c1", "deployer", json!({}))]);
    assert_eq!(
        run_counting(config(prov, registry(tool), &dir, Some(p))).await,
        0
    );
}

/// Subjects for a tool whose `path` argument is a Path with the given op.
fn path_arg_subjects(op: PathOp) -> SubjFn {
    Arc::new(move |input: &ToolInput| ToolSubjects {
        subjects: input
            .get_str("path")
            .map(|p| Subject::Path {
                path: PathBuf::from(p),
                op,
            })
            .into_iter()
            .collect(),
        unparsed: false,
        ..Default::default()
    })
}

#[tokio::test]
async fn test_guard_read_should_use_edit_path_after_hook_mutation() {
    let (_t, dir) = canonical_tmp();
    let ok = dir.join("ok.txt");
    let secret = dir.join("secret.txt");
    let (edit, edit_calls) = FakeTool::new("edit_file", false, path_arg_subjects(PathOp::Write));
    let (read, read_calls) = FakeTool::new("read_file", true, path_arg_subjects(PathOp::Read));
    let tools = ToolRegistry::builder()
        .register(Box::new(edit))
        .register(Box::new(read))
        .build();
    let p = policy(
        &dir,
        vec![PolicyRule::path("secret.txt", OpFilter::Read, Decision::Deny).with_id("no-read")],
        &[],
    );

    let mut hooks = HookRegistry::new();
    hooks.register(HookSpec {
        name: "rewrite".to_owned(),
        event: "PreToolUse".to_owned(),
        command: format!(
            r#"cat > /dev/null && echo '{{"deny":false,"message":null,"mutated_input":{{"path":"{}"}}}}'"#,
            secret.display()
        ),
        timeout_ms: 5000,
        tool_name_pattern: Some("edit_file".to_owned()),
    });

    let prov = Scripted::calls_then_done(vec![call(
        "e1",
        "edit_file",
        json!({ "path": ok.to_string_lossy() }),
    )]);
    let mut cfg = config(prov, tools, &dir, Some(p));
    cfg.hooks = Some(Arc::new(hooks));

    let _ = run(cfg).await;

    // The post-hook path is secret.txt, whose read is denied: nothing is read
    // (a guard that ran on the pre-hook path would have read ok.txt).
    assert_eq!(read_calls.load(Ordering::SeqCst), 0);
    assert_eq!(edit_calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// ApproveAlways on a wildcard rule is keyed by subject (spec section 7)
// ---------------------------------------------------------------------------

fn deploy_target_subjects() -> SubjFn {
    Arc::new(|input: &ToolInput| ToolSubjects {
        subjects: input
            .get_str("target")
            .map(|t| Subject::Command(format!("deploy {t}")))
            .into_iter()
            .collect(),
        unparsed: false,
        ..Default::default()
    })
}

#[tokio::test]
async fn test_agent_approve_always_on_wildcard_rule_should_not_cover_a_different_subject() {
    let (_t, dir) = canonical_tmp();
    let (tool, count) = FakeTool::new("deployer", false, deploy_target_subjects());
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let prov = Scripted::calls_then_done(vec![
        call("c1", "deployer", json!({ "target": "prod" })),
        call("c2", "deployer", json!({ "target": "staging" })),
    ]);
    // Two actions queued: the second subject must prompt again.
    let (rx, _keep) = step_channel(
        vec![
            StepAction::ApproveAlways("deployer".to_owned()),
            StepAction::Deny("no".to_owned()),
        ],
        true,
    );
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(
        paused(&o),
        2,
        "approving 'deploy prod' must not approve 'deploy staging'"
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(!end_of(&o, "c1").1);
    assert!(end_of(&o, "c2").1);
}

#[tokio::test]
async fn test_agent_approve_always_on_wildcard_rule_should_cover_the_same_subject_again() {
    let (_t, dir) = canonical_tmp();
    let (tool, count) = FakeTool::new("deployer", false, deploy_target_subjects());
    let p = policy(&dir, vec![ask_rule("deploy*", "ask-deploy")], &[]);
    let prov = Scripted::calls_then_done(vec![
        call("c1", "deployer", json!({ "target": "prod" })),
        call("c2", "deployer", json!({ "target": "prod" })),
    ]);
    let (rx, _keep) = step_channel(vec![StepAction::ApproveAlways("deployer".to_owned())], true);
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(paused(&o), 1);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_agent_approve_always_on_literal_command_rule_should_cover_the_same_command_line_again() {
    let (_t, dir) = canonical_tmp();
    let (tool, count) = FakeTool::fixed("deployer", false, vec![cmd("deploy prod")]);
    let p = policy(&dir, vec![ask_rule("deploy prod", "ask-literal")], &[]);
    let prov = Scripted::calls_then_done(vec![
        call("c1", "deployer", json!({})),
        call("c2", "deployer", json!({})),
    ]);
    let (rx, _keep) = step_channel(vec![StepAction::ApproveAlways("deployer".to_owned())], true);
    let mut cfg = config(prov, registry(tool), &dir, Some(p));
    cfg.step_rx = rx;

    let o = run(cfg).await;

    assert_eq!(paused(&o), 1);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
