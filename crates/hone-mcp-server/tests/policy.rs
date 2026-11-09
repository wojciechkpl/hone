#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.3 contract tests for gating `tools/call` in `hone-mcp-server`
//! (`docs/plans/policy-p2.3-spec.md` section 6).
//!
//! API assumptions (designed here; the spec only fixes behaviour):
//! - `McpServer::with_policy(self, Option<hone_tools::policy_gate::PolicyConfig>) -> McpServer`.
//!   Pre-approved ids travel in `PolicyConfig::pre_approved`; there is no interactive
//!   channel, so the session-approved set is always empty.
//! - `hone_mcp_server::load_policy(working_dir: &Path, home: Option<PathBuf>,
//!   approve: Option<&str>, builtins: bool) -> Result<Option<PolicyConfig>, E>` with `E: Display + Debug`.
//!   `approve` is the raw `HONE_APPROVE` value (comma-separated, items trimmed, empty
//!   items ignored). The binary calls it at startup and exits 2 on `Err`.
//!   - no files and no approve -> `Ok(None)`
//!   - no files but approve non-empty -> `Err` (like `--approve` with no policy)
//!   - unknown id -> `Err` whose message contains the id; known explicit/automatic ids
//!     and the synthetic `builtin:unparsed-command` / `builtin:invalid-host` -> accepted
//!   - broken policy file -> `Err`
//! - Dev-dependencies needed by this file: `hone-config`, `tempfile`, `serde_json`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, PolicyContext, PolicyRule, Subject};
use hone_mcp_server::{JsonRpcId, JsonRpcRequest, McpServer, load_policy};
use hone_proto::ToolLayer;
use hone_sandbox::NullSandbox;
use hone_tools::policy_gate::PolicyConfig;
use hone_tools::{
    SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry, ToolSubjects,
};
use serde_json::{Value, json};

// -- fixtures -------------------------------------------------------------------

struct FixedTool {
    name: &'static str,
    subjects: Vec<Subject>,
    unparsed: bool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for FixedTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "fixed"
    }
    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::new(json!({"hello": "world"})))
    }
    fn subjects(&self, _i: &ToolInput, _s: &SubjectContext) -> ToolSubjects {
        ToolSubjects {
            subjects: self.subjects.clone(),
            unparsed: self.unparsed,
            ..Default::default()
        }
    }
}

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        working_dir: dir.to_path_buf(),
        read_filter: None,
    }
}

fn cmd(s: &str) -> Subject {
    Subject::Command(s.to_owned())
}

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    Fixture { _tmp: tmp, dir }
}

fn policy(dir: &Path, rules: Vec<PolicyRule>, pre: &[&str]) -> PolicyConfig {
    let context = PolicyContext::for_platform(dir.to_path_buf(), None);
    let compiled = CompiledPolicy::compile(&context, &[], &rules, &[]).unwrap();
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: pre.iter().map(|s| (*s).to_owned()).collect(),
        builtins: false,
    }
}

fn server(
    f: &Fixture,
    subjects: Vec<Subject>,
    unparsed: bool,
    pol: Option<PolicyConfig>,
) -> (McpServer, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let tool = FixedTool {
        name: "act",
        subjects,
        unparsed,
        calls: calls.clone(),
    };
    let reg = ToolRegistry::builder().register(Box::new(tool)).build();
    let s = McpServer::new(reg, ctx(&f.dir));
    (s.with_policy(pol), calls)
}

async fn call_act(s: &McpServer) -> Value {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(JsonRpcId::Number(1)),
        method: "tools/call".into(),
        params: Some(json!({"name": "act", "arguments": {}})),
    };
    let resp = s.handle_request(req).await.expect("response");
    assert!(
        resp.error.is_none(),
        "policy outcomes are tool results, not JSON-RPC errors"
    );
    resp.result.expect("result")
}

fn text(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap().to_owned()
}

// -- tools/call gating ----------------------------------------------------------

#[tokio::test]
async fn deny_returns_is_error_with_rule_and_does_not_call_the_tool() {
    let f = fixture();
    let p = policy(
        &f.dir,
        vec![PolicyRule::command("git push*", Decision::Deny).with_id("no-push")],
        &[],
    );
    let (s, calls) = server(&f, vec![cmd("git push origin")], false, Some(p));
    let r = call_act(&s).await;
    assert_eq!(r["isError"], true);
    assert_eq!(r["content"][0]["type"], "text");
    assert_eq!(text(&r), "denied by policy (rule no-push)");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ask_returns_is_error_requires_approval_and_does_not_call_the_tool() {
    let f = fixture();
    let p = policy(
        &f.dir,
        vec![PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
        &[],
    );
    let (s, calls) = server(&f, vec![cmd("deploy prod")], false, Some(p));
    let r = call_act(&s).await;
    assert_eq!(r["isError"], true);
    assert_eq!(text(&r), "requires approval (rule ask-deploy)");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn automatic_rule_ids_appear_in_the_message() {
    let f = fixture();
    let p = policy(
        &f.dir,
        vec![PolicyRule::command("deploy*", Decision::Ask)],
        &[],
    );
    let (s, _) = server(&f, vec![cmd("deploy prod")], false, Some(p));
    assert_eq!(text(&call_act(&s).await), "requires approval (rule user:1)");
}

#[tokio::test]
async fn allow_proceeds_as_without_a_policy() {
    let f = fixture();
    let p = policy(
        &f.dir,
        vec![PolicyRule::command("git push*", Decision::Deny).with_id("no-push")],
        &[],
    );
    let (s, calls) = server(&f, vec![cmd("ls")], false, Some(p));
    let r = call_act(&s).await;
    assert_eq!(r["isError"], false);
    let parsed: Value = serde_json::from_str(&text(&r)).unwrap();
    assert_eq!(parsed["hello"], "world");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pre_approved_ask_rule_proceeds() {
    let f = fixture();
    let p = policy(
        &f.dir,
        vec![PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
        &["ask-deploy"],
    );
    let (s, calls) = server(&f, vec![cmd("deploy prod")], false, Some(p));
    assert_eq!(call_act(&s).await["isError"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pre_approval_never_lifts_a_deny() {
    let f = fixture();
    let p = policy(
        &f.dir,
        vec![PolicyRule::command("rm*", Decision::Deny).with_id("no-rm")],
        &["no-rm"],
    );
    let (s, calls) = server(&f, vec![cmd("rm x")], false, Some(p));
    let r = call_act(&s).await;
    assert_eq!(r["isError"], true);
    assert_eq!(text(&r), "denied by policy (rule no-rm)");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unparsed_shell_with_rules_asks_for_the_synthetic_rule() {
    let f = fixture();
    let p = policy(
        &f.dir,
        vec![PolicyRule::command("rm*", Decision::Deny)],
        &[],
    );
    let (s, calls) = server(&f, vec![], true, Some(p));
    assert_eq!(
        text(&call_act(&s).await),
        "requires approval (rule builtin:unparsed-command)"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn catch_all_ask_is_not_remembered_between_calls() {
    // No session state: every call asks again.
    let f = fixture();
    let p = policy(
        &f.dir,
        vec![PolicyRule::command("*", Decision::Ask).with_id("ask-all")],
        &[],
    );
    let (s, calls) = server(&f, vec![cmd("anything")], false, Some(p));
    assert_eq!(call_act(&s).await["isError"], true);
    assert_eq!(call_act(&s).await["isError"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn no_policy_leaves_behaviour_unchanged() {
    let f = fixture();
    let (s, calls) = server(&f, vec![cmd("git push origin")], true, None);
    let r = call_act(&s).await;
    assert_eq!(r["isError"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // And a server that never had with_policy called behaves the same.
    let calls2 = Arc::new(AtomicUsize::new(0));
    let reg = ToolRegistry::builder()
        .register(Box::new(FixedTool {
            name: "act",
            subjects: vec![cmd("git push")],
            unparsed: true,
            calls: calls2.clone(),
        }))
        .build();
    let plain = McpServer::new(reg, ctx(&f.dir));
    assert_eq!(call_act(&plain).await["isError"], false);
    assert_eq!(calls2.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn empty_policy_leaves_behaviour_unchanged() {
    let f = fixture();
    let (s, calls) = server(
        &f,
        vec![cmd("git push")],
        true,
        Some(policy(&f.dir, vec![], &[])),
    );
    assert_eq!(call_act(&s).await["isError"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unknown_tool_is_still_a_jsonrpc_error_under_a_policy() {
    let f = fixture();
    let p = policy(&f.dir, vec![PolicyRule::command("*", Decision::Deny)], &[]);
    let (s, _) = server(&f, vec![], false, Some(p));
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(JsonRpcId::Number(2)),
        method: "tools/call".into(),
        params: Some(json!({"name": "ghost", "arguments": {}})),
    };
    let resp = s.handle_request(req).await.unwrap();
    assert!(resp.error.is_some());
}

#[tokio::test]
async fn tools_list_is_not_filtered_by_policy() {
    let f = fixture();
    let p = policy(&f.dir, vec![PolicyRule::command("*", Decision::Deny)], &[]);
    let (s, _) = server(&f, vec![cmd("x")], false, Some(p));
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(JsonRpcId::Number(3)),
        method: "tools/list".into(),
        params: None,
    };
    let result = s.handle_request(req).await.unwrap().result.unwrap();
    assert_eq!(result["tools"].as_array().unwrap().len(), 1);
}

// -- load_policy (startup + HONE_APPROVE) -----------------------------------------

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

const PROJECT: &str = "version = 2\n\
[[rules]]\nid = \"no-push\"\ncommand = \"git push*\"\ndecision = \"deny\"\n\n\
[[rules]]\nid = \"ask-deploy\"\ncommand = \"deploy*\"\ndecision = \"ask\"\n\n\
[[rules]]\ncommand = \"make*\"\ndecision = \"ask\"\n";

fn project_dir() -> (tempfile::TempDir, tempfile::TempDir) {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(&wd.path().join(".hone/permissions.toml"), PROJECT);
    (wd, home)
}

#[test]
fn load_policy_without_files_is_none() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let r = load_policy(wd.path(), Some(home.path().to_path_buf()), None, false);
    assert!(matches!(r, Ok(None)), "{r:?}");
    let r = load_policy(wd.path(), Some(home.path().to_path_buf()), Some(""), false);
    assert!(
        matches!(r, Ok(None)),
        "empty HONE_APPROVE is no approval: {r:?}"
    );
}

#[test]
fn load_policy_with_files_loads_rules() {
    let (wd, home) = project_dir();
    let p = load_policy(wd.path(), Some(home.path().to_path_buf()), None, false)
        .unwrap()
        .unwrap();
    assert!(p.pre_approved.is_empty());
    let v = p.compiled.evaluate(&cmd("git push"));
    assert_eq!(v.decision, Decision::Deny);
}

#[test]
fn load_policy_parses_comma_separated_approvals_with_trimming() {
    let (wd, home) = project_dir();
    let p = load_policy(
        wd.path(),
        Some(home.path().to_path_buf()),
        Some(" ask-deploy , project:3 ,, builtin:unparsed-command,builtin:invalid-host "),
        false,
    )
    .unwrap()
    .unwrap();
    let mut got = p.pre_approved.clone();
    got.sort();
    let mut want = vec![
        "ask-deploy".to_owned(),
        "project:3".to_owned(),
        "builtin:unparsed-command".to_owned(),
        "builtin:invalid-host".to_owned(),
    ];
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn load_policy_unknown_approve_id_is_an_error_naming_the_id() {
    let (wd, home) = project_dir();
    let e = load_policy(
        wd.path(),
        Some(home.path().to_path_buf()),
        Some("ask-deploy,nope-x"),
        false,
    )
    .unwrap_err();
    assert!(e.to_string().contains("nope-x"), "{e}");
}

#[test]
fn load_policy_approve_without_any_policy_is_an_error() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let r = load_policy(
        wd.path(),
        Some(home.path().to_path_buf()),
        Some("user:1"),
        false,
    );
    assert!(r.is_err());
}

#[test]
fn load_policy_broken_file_is_an_error() {
    let wd = tempfile::tempdir().unwrap();
    write(
        &wd.path().join(".hone/permissions.toml"),
        "this is [[ not toml",
    );
    assert!(load_policy(wd.path(), None, None, false).is_err());
}

#[test]
fn load_policy_unknown_id_error_escapes_control_characters() {
    let (wd, home) = project_dir();
    let e = load_policy(
        wd.path(),
        Some(home.path().to_path_buf()),
        Some("a\u{1b}]52;x\u{7}b"),
        false,
    )
    .unwrap_err();
    assert!(!e.to_string().chars().any(|c| c.is_control()), "{e:?}");
}

#[tokio::test]
async fn approved_ids_from_load_policy_flow_into_the_gate() {
    let (wd, home) = project_dir();
    let dir = std::fs::canonicalize(wd.path()).unwrap();
    let p = load_policy(
        wd.path(),
        Some(home.path().to_path_buf()),
        Some("ask-deploy"),
        false,
    )
    .unwrap()
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let reg = ToolRegistry::builder()
        .register(Box::new(FixedTool {
            name: "act",
            subjects: vec![cmd("deploy prod")],
            unparsed: false,
            calls: calls.clone(),
        }))
        .build();
    let s = McpServer::new(reg, ctx(&dir)).with_policy(Some(p));
    assert_eq!(call_act(&s).await["isError"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Same policy without the approval asks.
    let p2 = load_policy(wd.path(), Some(home.path().to_path_buf()), None, false)
        .unwrap()
        .unwrap();
    let calls2 = Arc::new(AtomicUsize::new(0));
    let reg2 = ToolRegistry::builder()
        .register(Box::new(FixedTool {
            name: "act",
            subjects: vec![cmd("deploy prod")],
            unparsed: false,
            calls: calls2.clone(),
        }))
        .build();
    let s2 = McpServer::new(reg2, ctx(&dir)).with_policy(Some(p2));
    assert_eq!(
        text(&call_act(&s2).await),
        "requires approval (rule ask-deploy)"
    );
    assert_eq!(calls2.load(Ordering::SeqCst), 0);
}
