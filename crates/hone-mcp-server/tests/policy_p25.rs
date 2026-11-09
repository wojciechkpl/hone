#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 behaviour of `hone-mcp-server` (`docs/plans/policy-p2.5-spec.md` sections 2, 8, 10).
//!
//! API assumptions:
//! - `hone_mcp_server::load_policy(working_dir, home, approve, builtins: bool)`: the binary
//!   reads `HONE_POLICY_BUILTINS` once (via `policy_gate::builtins_enabled`) and passes it.
//!   With `builtins == true` it returns `Ok(Some(_))` even with no files.
//! - `McpServer::with_policy` also derives `ToolContext::read_filter` from the policy (when it
//!   has path rules) for every `tools/call`; the context given to `McpServer::new` has `None`.
//! - With `policy.builtins`, the text content of `tools/call` results is redacted. No note
//!   line is required for MCP (the spec only asks for the redaction).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use hone_config::policy::Subject;
use hone_mcp_server::{JsonRpcId, JsonRpcRequest, McpServer, load_policy};
use hone_proto::ToolLayer;
use hone_sandbox::NullSandbox;
use hone_tools::policy_gate::PolicyConfig;
use hone_tools::{
    GlobTool, GrepTool, SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput,
    ToolRegistry, ToolSubjects,
};
use serde_json::{Value, json};

struct Fx {
    _wd: tempfile::TempDir,
    _home: tempfile::TempDir,
    wd: PathBuf,
    home: PathBuf,
}

fn put(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn fixture() -> Fx {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    Fx {
        wd: std::fs::canonicalize(wd.path()).unwrap(),
        home: std::fs::canonicalize(home.path()).unwrap(),
        _wd: wd,
        _home: home,
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

struct Fixed {
    name: &'static str,
    subjects: Vec<Subject>,
    output: Value,
}

#[async_trait]
impl Tool for Fixed {
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
        Ok(ToolOutput::new(self.output.clone()))
    }
    fn subjects(&self, _i: &ToolInput, _s: &SubjectContext) -> ToolSubjects {
        ToolSubjects {
            subjects: self.subjects.clone(),
            ..Default::default()
        }
    }
}

async fn call_tool(s: &McpServer, name: &str, args: Value) -> Value {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(JsonRpcId::Number(1)),
        method: "tools/call".into(),
        params: Some(json!({"name": name, "arguments": args})),
    };
    let resp = s.handle_request(req).await.expect("response");
    assert!(resp.error.is_none(), "{:?}", resp.error);
    resp.result.expect("result")
}

fn text(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap().to_owned()
}

// -- load_policy with the switch -----------------------------------------------------------------

#[test]
fn load_policy_with_builtins_and_no_files_is_some_with_builtins_on() {
    let f = fixture();
    let p = load_policy(&f.wd, Some(f.home.clone()), None, true)
        .unwrap()
        .expect("built-ins make a policy");
    assert!(p.builtins);
    assert!(p.compiled.has_rule_id("builtin:git-push"));
}

#[test]
fn load_policy_without_builtins_and_no_files_is_still_none() {
    let f = fixture();
    let r = load_policy(&f.wd, Some(f.home.clone()), None, false);
    assert!(matches!(r, Ok(None)), "{r:?}");
}

#[test]
fn hone_approve_accepts_builtin_ids_with_builtins_and_no_files() {
    let f = fixture();
    let p = load_policy(
        &f.wd,
        Some(f.home.clone()),
        Some("builtin:git-push, builtin:computer-control"),
        true,
    )
    .unwrap()
    .unwrap();
    let mut got = p.pre_approved.clone();
    got.sort();
    assert_eq!(
        got,
        vec![
            "builtin:computer-control".to_owned(),
            "builtin:git-push".to_owned()
        ]
    );
}

#[test]
fn hone_approve_of_a_builtin_id_without_builtins_and_without_files_is_an_error() {
    let f = fixture();
    assert!(load_policy(&f.wd, Some(f.home.clone()), Some("builtin:git-push"), false).is_err());
}

// -- gating with built-ins ---------------------------------------------------------------------------

#[tokio::test]
async fn builtin_git_push_ask_surfaces_as_requires_approval() {
    let f = fixture();
    let p = load_policy(&f.wd, Some(f.home.clone()), None, true)
        .unwrap()
        .unwrap();
    let reg = ToolRegistry::builder()
        .register(Box::new(Fixed {
            name: "act",
            subjects: vec![Subject::Command("git push origin main".into())],
            output: json!({"hello": "world"}),
        }))
        .build();
    let s = McpServer::new(reg, ctx(&f.wd)).with_policy(Some(p));
    let r = call_tool(&s, "act", json!({})).await;
    assert_eq!(r["isError"], true);
    assert_eq!(text(&r), "requires approval (rule builtin:git-push)");
}

#[tokio::test]
async fn pre_approved_builtin_id_lets_the_call_through() {
    let f = fixture();
    let p = load_policy(&f.wd, Some(f.home.clone()), Some("builtin:git-push"), true)
        .unwrap()
        .unwrap();
    let reg = ToolRegistry::builder()
        .register(Box::new(Fixed {
            name: "act",
            subjects: vec![Subject::Command("git push origin main".into())],
            output: json!({"hello": "world"}),
        }))
        .build();
    let s = McpServer::new(reg, ctx(&f.wd)).with_policy(Some(p));
    assert_eq!(call_tool(&s, "act", json!({})).await["isError"], false);
}

// -- read filter over MCP -----------------------------------------------------------------------------

fn search_fixture() -> Fx {
    let f = fixture();
    put(&f.wd.join("src/a.txt"), "needle in src\n");
    put(&f.wd.join("secret/k.txt"), "needle in secret\n");
    put(&f.wd.join("notes/private.txt"), "needle in notes\n");
    put(
        &f.wd.join(".hone/permissions.toml"),
        "version = 2\n[[rules]]\nid = \"no-secret\"\npath = \"secret/**\"\nop = \"read\"\ndecision = \"deny\"\n",
    );
    put(
        &f.home.join(".config/hone/permissions.toml"),
        "version = 2\n[[rules]]\nid = \"ask-notes\"\npath = \"notes/**\"\nop = \"read\"\ndecision = \"ask\"\n",
    );
    f
}

fn search_server(f: &Fx, policy: Option<PolicyConfig>) -> McpServer {
    let reg = ToolRegistry::builder()
        .register(Box::new(GrepTool))
        .register(Box::new(GlobTool))
        .build();
    McpServer::new(reg, ctx(&f.wd)).with_policy(policy)
}

#[tokio::test]
async fn mcp_grep_skips_protected_files_and_reports_the_count() {
    let f = search_fixture();
    let p = load_policy(&f.wd, Some(f.home.clone()), None, false)
        .unwrap()
        .unwrap();
    let s = search_server(&f, Some(p));
    let r = call_tool(&s, "grep", json!({"pattern": "needle"})).await;
    assert_eq!(r["isError"], false, "{r}");
    let t = text(&r);
    assert!(t.contains("src/a.txt"), "{t}");
    assert!(!t.contains("needle in secret"), "{t}");
    assert!(!t.contains("needle in notes"), "{t}");
    assert!(
        t.contains("(2 file(s) skipped by permission policy)"),
        "{t}"
    );
}

#[tokio::test]
async fn mcp_glob_drops_denied_entries_and_keeps_needs_approval_entries() {
    let f = search_fixture();
    let p = load_policy(&f.wd, Some(f.home.clone()), None, false)
        .unwrap()
        .unwrap();
    let s = search_server(&f, Some(p));
    let r = call_tool(&s, "glob", json!({"pattern": "**/*.txt"})).await;
    let t = text(&r);
    assert!(t.contains("src/a.txt"), "{t}");
    assert!(t.contains("notes/private.txt"), "{t}");
    assert!(!t.contains("secret"), "{t}");
}

#[tokio::test]
async fn mcp_search_without_a_policy_changes_nothing() {
    let f = search_fixture();
    let s = search_server(&f, None);
    let t = text(&call_tool(&s, "grep", json!({"pattern": "needle"})).await);
    assert!(t.contains("needle in secret"), "{t}");
    assert!(t.contains("needle in notes"), "{t}");
    assert!(!t.contains("skipped by permission policy"), "{t}");
    let g = text(&call_tool(&s, "glob", json!({"pattern": "**/*.txt"})).await);
    assert!(g.contains("secret"), "{g}");
}

// -- redaction over MCP ---------------------------------------------------------------------------------------

fn leaky_server(f: &Fx, policy: Option<PolicyConfig>) -> McpServer {
    let reg = ToolRegistry::builder()
        .register(Box::new(Fixed {
            name: "leaky",
            subjects: vec![],
            output: json!({
                "stdout": "key AKIAIOSFODNN7EXAMPLE found",
                "line": "password=hunter2hunter2",
            }),
        }))
        .build();
    McpServer::new(reg, ctx(&f.wd)).with_policy(policy)
}

#[tokio::test]
async fn mcp_redacts_result_text_when_builtins_are_on() {
    let f = fixture();
    let p = load_policy(&f.wd, Some(f.home.clone()), None, true)
        .unwrap()
        .unwrap();
    let s = leaky_server(&f, Some(p));
    let r = call_tool(&s, "leaky", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    let t = text(&r);
    assert!(!t.contains("AKIAIOSFODNN7EXAMPLE"), "{t}");
    assert!(!t.contains("hunter2hunter2"), "{t}");
    assert!(t.contains("[REDACTED:aws-key]"), "{t}");
    assert!(t.contains("[REDACTED:secret-assignment]"), "{t}");
}

#[tokio::test]
async fn mcp_does_not_redact_without_builtins() {
    let f = fixture();
    put(
        &f.wd.join(".hone/permissions.toml"),
        "version = 2\n[[rules]]\ncommand = \"rm *\"\ndecision = \"deny\"\n",
    );
    let p = load_policy(&f.wd, Some(f.home.clone()), None, false)
        .unwrap()
        .unwrap();
    assert!(!p.builtins);
    let t = text(&call_tool(&leaky_server(&f, Some(p)), "leaky", json!({})).await);
    assert!(t.contains("AKIAIOSFODNN7EXAMPLE"), "{t}");
    assert!(t.contains("hunter2hunter2"), "{t}");
}

#[tokio::test]
async fn mcp_does_not_redact_without_a_policy() {
    let f = fixture();
    let t = text(&call_tool(&leaky_server(&f, None), "leaky", json!({})).await);
    assert!(t.contains("AKIAIOSFODNN7EXAMPLE"), "{t}");
}

// -- 14.11: HONE_APPROVE rejects ids whose rule always denies ----------------------------------------

#[test]
fn hone_approve_rejects_builtin_deny_ids() {
    let f = fixture();
    for id in ["builtin:pipe-to-shell", "builtin:sudo", "builtin:run0"] {
        let e = load_policy(&f.wd, Some(f.home.clone()), Some(id), true).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("always denies"), "{id}: {msg}");
        assert!(msg.contains(id), "{id}: {msg}");
    }
}

#[test]
fn hone_approve_rejects_a_project_deny_rule_but_accepts_ask_rules() {
    let f = fixture();
    put(
        &f.wd.join(".hone/permissions.toml"),
        "version = 2\n[[rules]]\nid = \"no-curl\"\ncommand = \"curl*\"\ndecision = \"deny\"\n\n[[rules]]\nid = \"ask-deploy\"\ncommand = \"deploy*\"\ndecision = \"ask\"\n",
    );
    let e = load_policy(&f.wd, Some(f.home.clone()), Some("no-curl"), false).unwrap_err();
    assert!(e.to_string().contains("always denies"), "{e}");
    assert!(load_policy(&f.wd, Some(f.home.clone()), Some("ask-deploy"), false).is_ok());
}
