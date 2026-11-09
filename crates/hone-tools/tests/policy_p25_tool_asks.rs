#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 tool-level asks (`docs/plans/policy-p2.5-spec.md` section 6).
//!
//! API assumptions:
//! - `Tool::builtin_ask(&self) -> Option<&'static str>` (default `None`).
//! - `ComputerControlTool` -> `Some("builtin:computer-control")`,
//!   `ScreenCaptureTool` -> `Some("builtin:screen-capture")`.
//! - With `PolicyConfig.builtins == true`, `decide` returns `Ask { rule_id: <that id> }` unless
//!   the id is in `session_approved` or `pre_approved` (approved BY ID, not by subject key).
//! - `session_approval_entries(.., id)` for a tool-level id is `vec![id]`.
//! - `validate_approvals` accepts the tool-level ids and `builtin:pipe-to-shell` only when
//!   `policy.builtins` is true.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, PolicyContext, PolicyRule, Subject, builtin_rules};
use hone_proto::{ToolCallRequest, ToolLayer};
use hone_tools::policy_gate::{
    GateDecision, PolicyConfig, decide, is_synthetic_rule, session_approval_entries,
    validate_approvals,
};
use hone_tools::{
    ComputerControlTool, EditFileTool, GlobTool, GrepTool, ReadFileTool, ScreenCaptureTool,
    ShellTool, SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolSubjects,
    WebFetchTool, WriteFileTool,
};
use serde_json::{Value, json};

struct Clicker {
    ask: Option<&'static str>,
    subjects: Vec<Subject>,
}

#[async_trait]
impl Tool for Clicker {
    fn name(&self) -> &str {
        "clicker"
    }
    fn description(&self) -> &str {
        "fake"
    }
    fn schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new(json!({})))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn subjects(&self, _i: &ToolInput, _s: &SubjectContext) -> ToolSubjects {
        ToolSubjects {
            subjects: self.subjects.clone(),
            ..Default::default()
        }
    }
    fn builtin_ask(&self) -> Option<&'static str> {
        self.ask
    }
}

struct Env {
    _wd: tempfile::TempDir,
    wd: PathBuf,
}

fn env() -> Env {
    let wd = tempfile::tempdir().unwrap();
    Env {
        wd: std::fs::canonicalize(wd.path()).unwrap(),
        _wd: wd,
    }
}

fn policy(wd: &Path, builtins: bool, user: Vec<PolicyRule>, pre: &[&str]) -> PolicyConfig {
    let context = PolicyContext::for_platform(wd.to_path_buf(), None);
    let b = if builtins {
        builtin_rules()
    } else {
        Vec::new()
    };
    let compiled = CompiledPolicy::compile(&context, &b, &user, &[]).unwrap();
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: pre.iter().map(|s| (*s).to_owned()).collect(),
        builtins,
    }
}

fn call() -> ToolCallRequest {
    ToolCallRequest {
        id: "1".into(),
        name: "clicker".into(),
        arguments: json!({}),
    }
}

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn flat(d: GateDecision) -> (&'static str, Option<String>) {
    match d {
        GateDecision::Allow => ("allow", None),
        GateDecision::Deny { rule_id } => ("deny", Some(rule_id)),
        GateDecision::Ask { rule_id } => ("ask", Some(rule_id)),
    }
}

fn run(
    p: &PolicyConfig,
    approved: &HashSet<String>,
    t: &dyn Tool,
) -> (&'static str, Option<String>) {
    flat(decide(p, approved, Some(t), &call()))
}

// -- the trait method ------------------------------------------------------------------------

#[test]
fn computer_control_and_screen_capture_declare_their_ask_ids() {
    assert_eq!(
        ComputerControlTool.builtin_ask(),
        Some("builtin:computer-control")
    );
    assert_eq!(
        ScreenCaptureTool.builtin_ask(),
        Some("builtin:screen-capture")
    );
}

#[test]
fn other_native_tools_have_no_builtin_ask() {
    let tools: Vec<(&str, Box<dyn Tool>)> = vec![
        ("shell", Box::new(ShellTool)),
        ("read_file", Box::new(ReadFileTool)),
        ("write_file", Box::new(WriteFileTool)),
        ("edit_file", Box::new(EditFileTool)),
        ("grep", Box::new(GrepTool)),
        ("glob", Box::new(GlobTool)),
        ("web_fetch", Box::new(WebFetchTool::new())),
    ];
    for (name, t) in tools {
        assert_eq!(t.builtin_ask(), None, "{name}");
    }
}

// -- decide ----------------------------------------------------------------------------------

#[test]
fn tool_level_ask_asks_when_builtins_are_on() {
    let e = env();
    let p = policy(&e.wd, true, vec![], &[]);
    let t = Clicker {
        ask: Some("builtin:computer-control"),
        subjects: vec![],
    };
    assert_eq!(
        run(&p, &set(&[]), &t),
        ("ask", Some("builtin:computer-control".to_owned()))
    );
}

#[test]
fn real_computer_control_and_screen_capture_ask_under_builtins_and_allow_without() {
    let e = env();
    for (tool, id) in [
        (
            &ComputerControlTool as &dyn Tool,
            "builtin:computer-control",
        ),
        (&ScreenCaptureTool as &dyn Tool, "builtin:screen-capture"),
    ] {
        let on = policy(&e.wd, true, vec![], &[]);
        let call = ToolCallRequest {
            id: "1".into(),
            name: tool.name().to_owned(),
            arguments: json!({}),
        };
        assert_eq!(
            flat(decide(&on, &set(&[]), Some(tool), &call)),
            ("ask", Some(id.to_owned()))
        );
        // builtins off, with an unrelated rule so the policy is non-empty
        let off = policy(
            &e.wd,
            false,
            vec![PolicyRule::command("rm *", Decision::Deny)],
            &[],
        );
        assert_eq!(
            flat(decide(&off, &set(&[]), Some(tool), &call)),
            ("allow", None),
            "{id} without built-ins"
        );
    }
}

#[test]
fn tool_level_ask_is_ignored_when_builtins_are_off() {
    let e = env();
    let p = policy(&e.wd, false, vec![], &[]);
    let t = Clicker {
        ask: Some("builtin:computer-control"),
        subjects: vec![],
    };
    assert_eq!(run(&p, &set(&[]), &t), ("allow", None));
}

#[test]
fn tool_level_ask_is_approved_by_a_session_entry_of_the_bare_id() {
    let e = env();
    let p = policy(&e.wd, true, vec![], &[]);
    let t = Clicker {
        ask: Some("builtin:computer-control"),
        subjects: vec![],
    };
    assert_eq!(
        run(&p, &set(&["builtin:computer-control"]), &t),
        ("allow", None)
    );
    // an approval of another tool-level id does not cover it
    assert_eq!(
        run(&p, &set(&["builtin:screen-capture"]), &t),
        ("ask", Some("builtin:computer-control".to_owned()))
    );
}

#[test]
fn tool_level_ask_is_approved_by_pre_approved_id() {
    let e = env();
    let p = policy(&e.wd, true, vec![], &["builtin:computer-control"]);
    let t = Clicker {
        ask: Some("builtin:computer-control"),
        subjects: vec![],
    };
    assert_eq!(run(&p, &set(&[]), &t), ("allow", None));
}

#[test]
fn tool_level_ask_never_outranks_a_parsed_deny() {
    let e = env();
    let p = policy(&e.wd, true, vec![], &[]);
    let t = Clicker {
        ask: Some("builtin:computer-control"),
        subjects: vec![Subject::Command("sudo rm x".to_owned())],
    };
    assert_eq!(
        run(&p, &set(&["builtin:computer-control"]), &t),
        ("deny", Some("builtin:sudo".to_owned()))
    );
    assert_eq!(
        run(&p, &set(&[]), &t),
        ("deny", Some("builtin:sudo".to_owned()))
    );
}

#[test]
fn tool_level_ask_does_not_hide_a_different_subject_ask() {
    let e = env();
    let p = policy(&e.wd, true, vec![], &["builtin:computer-control"]);
    let t = Clicker {
        ask: Some("builtin:computer-control"),
        subjects: vec![Subject::Command("git push origin main".to_owned())],
    };
    assert_eq!(
        run(&p, &set(&[]), &t),
        ("ask", Some("builtin:git-push".to_owned()))
    );
}

// -- remembering ----------------------------------------------------------------------------------

#[test]
fn tool_level_ids_are_not_synthetic_and_are_remembered_by_id() {
    let e = env();
    let p = policy(&e.wd, true, vec![], &[]);
    for id in ["builtin:computer-control", "builtin:screen-capture"] {
        assert!(!is_synthetic_rule(id), "{id}");
        let t = Clicker {
            ask: if id == "builtin:computer-control" {
                Some("builtin:computer-control")
            } else {
                Some("builtin:screen-capture")
            },
            subjects: vec![],
        };
        let entries = session_approval_entries(&p, Some(&t as &dyn Tool), &call(), id);
        assert_eq!(entries, vec![id.to_owned()]);
    }
}

// -- --approve validation ------------------------------------------------------------------------

#[test]
fn validate_approvals_accepts_tool_level_ids_only_with_builtins() {
    let e = env();
    let ids: Vec<String> = ["builtin:computer-control", "builtin:screen-capture"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();

    let on = policy(&e.wd, true, vec![], &[]);
    assert_eq!(validate_approvals(Some(&on), &ids), Ok(ids.clone()));

    // builtins off, policy non-empty: both are unknown ids
    let off = policy(
        &e.wd,
        false,
        vec![PolicyRule::command("rm *", Decision::Deny).with_id("no-rm")],
        &[],
    );
    for id in &ids {
        let r = validate_approvals(Some(&off), std::slice::from_ref(id));
        assert!(r.is_err(), "{id} must be rejected without built-ins");
        assert!(r.unwrap_err().contains("unknown rule id"), "{id}");
    }
}

#[test]
fn validate_approvals_rejects_pipe_to_shell_because_it_always_denies() {
    // 14.11: pipe-to-shell is a Deny, so approving it is an error even with built-ins on.
    let e = env();
    let on = policy(&e.wd, true, vec![], &[]);
    let err = validate_approvals(Some(&on), &["builtin:pipe-to-shell".to_owned()]).unwrap_err();
    assert_eq!(
        err,
        "--approve: rule 'builtin:pipe-to-shell' always denies; --approve only lifts ask rules"
    );
}

#[test]
fn validate_approvals_still_accepts_the_old_synthetic_ids_without_builtins() {
    let e = env();
    let off = policy(
        &e.wd,
        false,
        vec![PolicyRule::command("rm *", Decision::Deny)],
        &[],
    );
    let ids = vec![
        "builtin:unparsed-command".to_owned(),
        "builtin:invalid-host".to_owned(),
    ];
    assert_eq!(validate_approvals(Some(&off), &ids), Ok(ids.clone()));
}
