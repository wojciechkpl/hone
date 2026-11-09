#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 "always" on command rules (`docs/plans/policy-p2.5-spec.md` section 7):
//! approving a command rule remembers the (rule id, subject) pair, never the bare id.
//!
//! The flow mirrors the agent loop: `session_approval_entries` produces what is remembered,
//! and `decide` is then given that set.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, PolicyContext, PolicyRule, Subject, builtin_rules};
use hone_proto::{ToolCallRequest, ToolLayer};
use hone_tools::policy_gate::{
    GateDecision, PolicyConfig, approval_key, decide, session_approval_entries,
};
use hone_tools::{
    SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolSubjects,
};
use serde_json::{Value, json};

struct Fake {
    subjects: Vec<Subject>,
}

#[async_trait]
impl Tool for Fake {
    fn name(&self) -> &str {
        "fake"
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
}

fn cmd_tool(c: &str) -> Fake {
    Fake {
        subjects: vec![Subject::Command(c.to_owned())],
    }
}

fn host_tool(h: &str) -> Fake {
    Fake {
        subjects: vec![Subject::Host(h.to_owned())],
    }
}

fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    (tmp, dir)
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
        name: "fake".into(),
        arguments: json!({}),
    }
}

fn flat(d: GateDecision) -> (&'static str, Option<String>) {
    match d {
        GateDecision::Allow => ("allow", None),
        GateDecision::Deny { rule_id } => ("deny", Some(rule_id)),
        GateDecision::Ask { rule_id } => ("ask", Some(rule_id)),
    }
}

fn run(p: &PolicyConfig, approved: &HashSet<String>, t: &Fake) -> (&'static str, Option<String>) {
    flat(decide(p, approved, Some(t as &dyn Tool), &call()))
}

fn remember(p: &PolicyConfig, t: &Fake, rule_id: &str) -> HashSet<String> {
    session_approval_entries(p, Some(t as &dyn Tool), &call(), rule_id)
        .into_iter()
        .collect()
}

fn push_rule() -> PolicyRule {
    PolicyRule::command("git push", Decision::Ask).with_id("ask-push")
}

#[test]
fn always_on_a_literal_command_rule_remembers_the_subject_not_the_id() {
    let (_t, d) = canonical_tmp();
    let p = policy(&d, false, vec![push_rule()], &[]);
    let t = cmd_tool("git push origin main");
    let entries = session_approval_entries(&p, Some(&t as &dyn Tool), &call(), "ask-push");
    assert_eq!(
        entries,
        vec![approval_key(
            "ask-push",
            &Subject::Command("git push origin main".to_owned())
        )]
    );
    assert!(!entries.contains(&"ask-push".to_owned()));
}

#[test]
fn always_on_git_push_origin_main_does_not_cover_a_force_push() {
    let (_t, d) = canonical_tmp();
    let p = policy(&d, false, vec![push_rule()], &[]);
    let approved = remember(&p, &cmd_tool("git push origin main"), "ask-push");

    assert_eq!(
        run(&p, &approved, &cmd_tool("git push origin main")),
        ("allow", None),
        "the approved command line is covered"
    );
    assert_eq!(
        run(&p, &approved, &cmd_tool("git push --force origin main")),
        ("ask", Some("ask-push".to_owned())),
        "a different command line asks again"
    );
    assert_eq!(
        run(&p, &approved, &cmd_tool("git push")),
        ("ask", Some("ask-push".to_owned()))
    );
}

#[test]
fn whitespace_differences_do_not_defeat_a_remembered_command() {
    let (_t, d) = canonical_tmp();
    let p = policy(&d, false, vec![push_rule()], &[]);
    let approved = remember(&p, &cmd_tool("git push origin main"), "ask-push");
    assert_eq!(
        run(&p, &approved, &cmd_tool("git  push   origin main")),
        ("allow", None)
    );
}

#[test]
fn bare_id_in_the_session_set_does_not_approve_a_command_rule() {
    let (_t, d) = canonical_tmp();
    let p = policy(&d, false, vec![push_rule()], &[]);
    let approved: HashSet<String> = ["ask-push".to_owned()].into_iter().collect();
    assert_eq!(
        run(&p, &approved, &cmd_tool("git push --force")),
        ("ask", Some("ask-push".to_owned()))
    );
}

#[test]
fn pre_approve_still_covers_every_command_line() {
    let (_t, d) = canonical_tmp();
    let p = policy(&d, false, vec![push_rule()], &["ask-push"]);
    for c in [
        "git push",
        "git push origin main",
        "git push --force origin main",
    ] {
        assert_eq!(
            run(&p, &HashSet::new(), &cmd_tool(c)),
            ("allow", None),
            "{c}"
        );
    }
}

#[test]
fn builtin_git_push_is_remembered_per_command_line() {
    let (_t, d) = canonical_tmp();
    let p = policy(&d, true, vec![], &[]);
    let approved = remember(&p, &cmd_tool("git push origin dev"), "builtin:git-push");
    assert_eq!(
        run(&p, &approved, &cmd_tool("git push origin dev")),
        ("allow", None)
    );
    assert_eq!(
        run(&p, &approved, &cmd_tool("git push --force origin dev")),
        ("ask", Some("builtin:git-push".to_owned()))
    );
}

#[test]
fn a_literal_host_rule_is_still_approved_by_id() {
    let (_t, d) = canonical_tmp();
    let p = policy(
        &d,
        false,
        vec![PolicyRule::host("example.com", Decision::Ask).with_id("ask-host")],
        &[],
    );
    let t = host_tool("example.com");
    let entries = session_approval_entries(&p, Some(&t as &dyn Tool), &call(), "ask-host");
    assert_eq!(entries, vec!["ask-host".to_owned()]);
    let approved: HashSet<String> = entries.into_iter().collect();
    assert_eq!(run(&p, &approved, &t), ("allow", None));
    assert_eq!(
        run(&p, &approved, &host_tool("EXAMPLE.com:8443")),
        ("allow", None)
    );
}

#[test]
fn a_literal_path_rule_is_still_approved_by_id() {
    let (_t, d) = canonical_tmp();
    let p = policy(
        &d,
        false,
        vec![
            PolicyRule::path(
                "/etc/hosts",
                hone_config::policy::OpFilter::Write,
                Decision::Ask,
            )
            .with_id("ask-hosts-file"),
        ],
        &[],
    );
    let t = Fake {
        subjects: vec![Subject::Path {
            path: PathBuf::from("/etc/hosts"),
            op: hone_config::policy::PathOp::Write,
        }],
    };
    let entries = session_approval_entries(&p, Some(&t as &dyn Tool), &call(), "ask-hosts-file");
    assert_eq!(entries, vec!["ask-hosts-file".to_owned()]);
}

#[test]
fn a_literal_path_rule_with_op_any_is_keyed_per_subject() {
    // 14.6: only an explicit read/write op is approved by bare id.
    let (_t, d) = canonical_tmp();
    let p = policy(
        &d,
        false,
        vec![
            PolicyRule::path(
                "/etc/hosts",
                hone_config::policy::OpFilter::Any,
                Decision::Ask,
            )
            .with_id("ask-hosts-any"),
        ],
        &[],
    );
    let subject = Subject::Path {
        path: PathBuf::from("/etc/hosts"),
        op: hone_config::policy::PathOp::Write,
    };
    let t = Fake {
        subjects: vec![subject.clone()],
    };
    let entries = session_approval_entries(&p, Some(&t as &dyn Tool), &call(), "ask-hosts-any");
    assert_eq!(entries, vec![approval_key("ask-hosts-any", &subject)]);
}

#[test]
fn wildcard_command_rules_stay_keyed_by_subject() {
    let (_t, d) = canonical_tmp();
    let p = policy(
        &d,
        false,
        vec![PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
        &[],
    );
    let approved = remember(&p, &cmd_tool("deploy prod"), "ask-deploy");
    assert_eq!(
        run(&p, &approved, &cmd_tool("deploy prod")),
        ("allow", None)
    );
    assert_eq!(
        run(&p, &approved, &cmd_tool("deploy staging")),
        ("ask", Some("ask-deploy".to_owned()))
    );
}
