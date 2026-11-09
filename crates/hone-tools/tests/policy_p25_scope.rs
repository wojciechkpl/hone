#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 section 14 gate behaviour, through the real tools and the public gate API:
//! - 14.5 remembered command approvals cover exactly what was approved;
//! - 14.6 what is approved by id (path keys include the op);
//! - 14.7 paths are checked both as written and as resolved (symlinks);
//! - 14.9 variable-derived paths ask when built-ins are on;
//! - 14.11 `--approve` rejects deny ids; refusal and warning texts.
//!
//! Everything uses temp dirs and an explicit `home`; the environment is never read.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, OpFilter, PolicyContext, PolicyRule, Subject};
use hone_proto::ToolCallRequest;
use hone_tools::policy_gate::{
    GateDecision, PolicyConfig, ReadAccess, approval_key, check_provider_gating, decide,
    read_filter, resolve_permission_policy, session_approval_entries, validate_approvals,
};
use hone_tools::{ReadFileTool, ShellTool, Tool, WriteFileTool};
use serde_json::{Value, json};

struct Env {
    _wd: tempfile::TempDir,
    _home: tempfile::TempDir,
    wd: PathBuf,
    home: PathBuf,
}

fn env() -> Env {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    Env {
        wd: std::fs::canonicalize(wd.path()).unwrap(),
        home: std::fs::canonicalize(home.path()).unwrap(),
        _wd: wd,
        _home: home,
    }
}

fn builtin_policy(e: &Env) -> PolicyConfig {
    resolve_permission_policy(&e.wd, Some(e.home.clone()), true)
        .unwrap()
        .unwrap()
}

fn user_policy(e: &Env, rules: Vec<PolicyRule>) -> PolicyConfig {
    let context = PolicyContext::for_platform(e.wd.clone(), Some(e.home.clone()));
    let compiled = CompiledPolicy::compile(&context, &[], &rules, &[]).unwrap();
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: Vec::new(),
        builtins: false,
    }
}

fn call(name: &str, args: Value) -> ToolCallRequest {
    ToolCallRequest {
        id: "1".into(),
        name: name.into(),
        arguments: args,
    }
}

fn shell(c: &str) -> ToolCallRequest {
    call("shell", json!({ "command": c }))
}

fn flat(d: GateDecision) -> (&'static str, Option<String>) {
    match d {
        GateDecision::Allow => ("allow", None),
        GateDecision::Deny { rule_id } => ("deny", Some(rule_id)),
        GateDecision::Ask { rule_id } => ("ask", Some(rule_id)),
    }
}

fn gate(
    p: &PolicyConfig,
    approved: &HashSet<String>,
    tool: &dyn Tool,
    c: &ToolCallRequest,
) -> (&'static str, Option<String>) {
    flat(decide(p, approved, Some(tool), c))
}

fn remember(p: &PolicyConfig, tool: &dyn Tool, c: &ToolCallRequest, rule: &str) -> HashSet<String> {
    session_approval_entries(p, Some(tool), c, rule)
        .into_iter()
        .collect()
}

fn none() -> HashSet<String> {
    HashSet::new()
}

// -- 14.5 command approvals cover exactly what was approved ---------------------------------------

const PUSH: &str = "git push origin main";

#[test]
fn approved_command_line_is_covered_again() {
    let e = env();
    let p = builtin_policy(&e);
    let approved = remember(&p, &ShellTool, &shell(PUSH), "builtin:git-push");
    assert_eq!(
        gate(&p, &approved, &ShellTool, &shell(PUSH)),
        ("allow", None)
    );
    // an unapproved session still asks
    assert_eq!(
        gate(&p, &none(), &ShellTool, &shell(PUSH)),
        ("ask", Some("builtin:git-push".to_owned()))
    );
}

#[test]
fn always_on_git_push_does_not_cover_assignments_wrappers_or_a_different_directory() {
    let e = env();
    let p = builtin_policy(&e);
    let approved = remember(&p, &ShellTool, &shell(PUSH), "builtin:git-push");
    for variant in [
        "GIT_SSH_COMMAND='sh -c id' git push origin main",
        "GIT_DIR=/tmp/o/.git git push origin main",
        "env X=1 git push origin main",
        "nice git push origin main",
        "cd /tmp/o && git push origin main",
    ] {
        let (kind, _) = gate(&p, &approved, &ShellTool, &shell(variant));
        assert_eq!(kind, "ask", "{variant} must ask again");
    }
}

#[test]
fn an_approval_of_a_variant_covers_only_that_variant() {
    let e = env();
    let p = builtin_policy(&e);
    for variant in [
        "env X=1 git push origin main",
        "GIT_DIR=/tmp/o/.git git push origin main",
        "cd /tmp/o && git push origin main",
    ] {
        let approved = remember(&p, &ShellTool, &shell(variant), "builtin:git-push");
        assert_eq!(
            gate(&p, &approved, &ShellTool, &shell(variant)),
            ("allow", None),
            "{variant} round trip"
        );
        let (kind, _) = gate(&p, &approved, &ShellTool, &shell(PUSH));
        assert_eq!(kind, "ask", "plain push must not ride on `{variant}`");
    }
}

// -- 14.6 what is approved by id ----------------------------------------------------------------------

#[test]
fn approving_a_netrc_read_does_not_approve_a_git_credentials_write() {
    let e = env();
    let p = builtin_policy(&e);
    let netrc = e.home.join(".netrc").to_string_lossy().into_owned();
    let gitcred = e
        .home
        .join(".git-credentials")
        .to_string_lossy()
        .into_owned();
    let read_netrc = call("read_file", json!({ "path": netrc }));
    let approved = remember(&p, &ReadFileTool, &read_netrc, "builtin:git-credentials");
    assert!(
        !approved.contains("builtin:git-credentials"),
        "multi-pattern, op-any rule must be keyed, not remembered by id: {approved:?}"
    );

    assert_eq!(
        gate(&p, &approved, &ReadFileTool, &read_netrc),
        ("allow", None)
    );
    let write_gitcred = call("write_file", json!({ "path": gitcred, "content": "x" }));
    let (kind, id) = gate(&p, &approved, &WriteFileTool, &write_gitcred);
    assert_eq!(kind, "ask");
    assert_eq!(id.as_deref(), Some("builtin:git-credentials"));
    let read_gitcred = call("read_file", json!({ "path": gitcred }));
    assert_eq!(gate(&p, &approved, &ReadFileTool, &read_gitcred).0, "ask");
}

#[test]
fn path_keys_include_the_op() {
    let e = env();
    let target = e.wd.join("f.txt");
    let pattern = target.to_string_lossy().into_owned();
    let p = user_policy(
        &e,
        vec![PolicyRule::path(pattern.clone(), OpFilter::Any, Decision::Ask).with_id("ask-f")],
    );
    let read = call("read_file", json!({ "path": pattern }));
    let approved = remember(&p, &ReadFileTool, &read, "ask-f");
    assert_eq!(gate(&p, &approved, &ReadFileTool, &read), ("allow", None));
    let write = call("write_file", json!({ "path": pattern, "content": "x" }));
    assert_eq!(
        gate(&p, &approved, &WriteFileTool, &write),
        ("ask", Some("ask-f".to_owned())),
        "approving a read must not approve a write"
    );
}

#[test]
fn literal_single_pattern_path_rule_with_explicit_op_is_remembered_by_id() {
    let e = env();
    let target = e.wd.join("f.txt").to_string_lossy().into_owned();
    let p = user_policy(
        &e,
        vec![PolicyRule::path(target.clone(), OpFilter::Read, Decision::Ask).with_id("ask-f")],
    );
    let read = call("read_file", json!({ "path": target }));
    assert_eq!(
        session_approval_entries(&p, Some(&ReadFileTool), &read, "ask-f"),
        vec!["ask-f".to_owned()]
    );
}

#[test]
fn multi_pattern_literal_path_rule_is_keyed_per_subject() {
    let e = env();
    let a = e.wd.join("a.txt").to_string_lossy().into_owned();
    let b = e.wd.join("b.txt").to_string_lossy().into_owned();
    let p = user_policy(
        &e,
        vec![
            PolicyRule::path(a.clone(), OpFilter::Read, Decision::Ask)
                .or_path(b.clone())
                .with_id("ask-ab"),
        ],
    );
    let read_a = call("read_file", json!({ "path": a }));
    let read_b = call("read_file", json!({ "path": b }));
    let approved = remember(&p, &ReadFileTool, &read_a, "ask-ab");
    assert_eq!(gate(&p, &approved, &ReadFileTool, &read_a), ("allow", None));
    assert_eq!(
        gate(&p, &approved, &ReadFileTool, &read_b),
        ("ask", Some("ask-ab".to_owned()))
    );
}

#[test]
fn approval_key_differs_between_read_and_write_of_the_same_path() {
    let path = PathBuf::from("/work/proj/f.txt");
    let r = approval_key(
        "r",
        &Subject::Path {
            path: path.clone(),
            op: hone_config::policy::PathOp::Read,
        },
    );
    let w = approval_key(
        "r",
        &Subject::Path {
            path,
            op: hone_config::policy::PathOp::Write,
        },
    );
    assert_ne!(r, w);
}

// -- 14.7 both the written and the resolved path --------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_symlinked_rc_file_that_resolves_into_the_workdir_still_asks() {
    let e = env();
    std::fs::write(e.wd.join("rc.txt"), "").unwrap();
    let link = e.home.join(".bashrc");
    std::os::unix::fs::symlink(e.wd.join("rc.txt"), &link).unwrap();
    let p = builtin_policy(&e);
    let w = call(
        "write_file",
        json!({ "path": link.to_string_lossy(), "content": "x" }),
    );
    assert_eq!(
        gate(&p, &none(), &WriteFileTool, &w),
        ("ask", Some("builtin:shell-startup".to_owned()))
    );
}

#[cfg(unix)]
#[test]
fn on_a_tie_between_written_and_resolved_rules_the_canonical_id_is_reported() {
    let e = env();
    std::fs::create_dir_all(e.home.join(".ssh")).unwrap();
    std::fs::write(e.home.join(".ssh/x"), "").unwrap();
    let link = e.home.join(".bashrc");
    std::os::unix::fs::symlink(e.home.join(".ssh/x"), &link).unwrap();
    let p = builtin_policy(&e);
    let w = call(
        "write_file",
        json!({ "path": link.to_string_lossy(), "content": "x" }),
    );
    assert_eq!(
        gate(&p, &none(), &WriteFileTool, &w),
        ("ask", Some("builtin:ssh".to_owned()))
    );
}

#[cfg(unix)]
#[test]
fn read_filter_checks_the_written_path_too() {
    let e = env();
    std::fs::write(e.wd.join("notes.txt"), "x").unwrap();
    let link = e.home.join(".netrc");
    std::os::unix::fs::symlink(e.wd.join("notes.txt"), &link).unwrap();
    let rf = read_filter(&builtin_policy(&e), &none());
    assert!(matches!(rf(&link), ReadAccess::NeedsApproval));
    assert!(matches!(rf(&e.wd.join("notes.txt")), ReadAccess::Allowed));
}

#[cfg(unix)]
#[test]
fn ordinary_paths_are_unaffected_by_the_dual_check() {
    let e = env();
    let p = builtin_policy(&e);
    let w = call(
        "write_file",
        json!({ "path": e.wd.join("src/x.rs").to_string_lossy(), "content": "x" }),
    );
    assert_eq!(gate(&p, &none(), &WriteFileTool, &w), ("allow", None));
}

// -- 14.9 variable-derived paths ask ------------------------------------------------------------------------

#[test]
fn variable_and_substitution_paths_ask_when_builtins_are_on() {
    let e = env();
    let p = builtin_policy(&e);
    let netrc = e.home.join(".netrc").display().to_string();
    for c in [
        format!("f={netrc}; cat \"$f\""),
        "cat \"$(printf '%s' x)\"".to_owned(),
        "cat $f".to_owned(),
        "cat ${f}/x".to_owned(),
        "cat `echo x`".to_owned(),
    ] {
        assert_eq!(
            gate(&p, &none(), &ShellTool, &shell(&c)),
            ("ask", Some("builtin:unparsed-command".to_owned())),
            "{c}"
        );
    }
}

#[test]
fn expansions_outside_path_positions_do_not_ask() {
    let e = env();
    let p = builtin_policy(&e);
    for c in ["echo $HOME", "echo \"$PATH\"", "ls -la", "cat README.md"] {
        assert_eq!(
            gate(&p, &none(), &ShellTool, &shell(c)),
            ("allow", None),
            "{c}"
        );
    }
}

// -- 14.11 --approve rejects ids that always deny --------------------------------------------------------------

fn deny_msg(id: &str) -> String {
    format!("--approve: rule '{id}' always denies; --approve only lifts ask rules")
}

#[test]
fn approve_rejects_builtin_deny_rules() {
    let e = env();
    let p = builtin_policy(&e);
    for id in [
        "builtin:pipe-to-shell",
        "builtin:sudo",
        "builtin:sudoedit",
        "builtin:su",
        "builtin:doas",
        "builtin:pkexec",
        "builtin:run0",
    ] {
        assert_eq!(
            validate_approvals(Some(&p), &[id.to_owned()]),
            Err(deny_msg(id)),
            "{id}"
        );
    }
}

#[test]
fn approve_rejects_user_and_project_deny_rules_but_not_ask_or_allow_rules() {
    let e = env();
    let p = user_policy(
        &e,
        vec![
            PolicyRule::command("rm *", Decision::Deny).with_id("no-rm"),
            PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy"),
            PolicyRule::command("ls*", Decision::Allow).with_id("ok-ls"),
        ],
    );
    assert_eq!(
        validate_approvals(Some(&p), &["no-rm".to_owned()]),
        Err(deny_msg("no-rm"))
    );
    let ok = vec!["ask-deploy".to_owned(), "ok-ls".to_owned()];
    assert_eq!(validate_approvals(Some(&p), &ok), Ok(ok.clone()));
    // one deny among valid ids rejects the whole list
    let mixed = vec!["ask-deploy".to_owned(), "no-rm".to_owned()];
    assert_eq!(validate_approvals(Some(&p), &mixed), Err(deny_msg("no-rm")));
}

#[test]
fn approve_rejects_a_project_file_deny_rule() {
    let e = env();
    std::fs::create_dir_all(e.wd.join(".hone")).unwrap();
    std::fs::write(
        e.wd.join(".hone/permissions.toml"),
        "version = 2\n[[rules]]\nid = \"proj-no-curl\"\ncommand = \"curl*\"\ndecision = \"deny\"\n",
    )
    .unwrap();
    let p = builtin_policy(&e);
    assert_eq!(
        validate_approvals(Some(&p), &["proj-no-curl".to_owned()]),
        Err(deny_msg("proj-no-curl"))
    );
}

#[test]
fn approving_ask_builtins_is_still_valid() {
    let e = env();
    let p = builtin_policy(&e);
    let ids = vec!["builtin:git-push".to_owned(), "builtin:ssh".to_owned()];
    assert_eq!(validate_approvals(Some(&p), &ids), Ok(ids.clone()));
}

#[test]
fn refusal_names_the_builtins_switch_when_built_ins_are_the_only_policy() {
    let e = env();
    let p = builtin_policy(&e);
    let msg = check_provider_gating(Some(&p), true, None).unwrap_err();
    assert!(msg.starts_with("this provider "), "{msg}");
    assert!(msg.contains("HONE_POLICY_BUILTINS"), "{msg}");
    assert!(msg.contains("unset"), "{msg}");
    assert!(msg.contains("HONE_ALLOW_UNGATED_PROVIDER=1"), "{msg}");
}

#[test]
fn refusal_does_not_blame_built_ins_when_policy_files_exist() {
    let e = env();
    let p = user_policy(&e, vec![PolicyRule::command("rm *", Decision::Deny)]);
    let msg = check_provider_gating(Some(&p), true, None).unwrap_err();
    assert!(!msg.contains("HONE_POLICY_BUILTINS"), "{msg}");
}

#[test]
fn override_warning_says_output_is_not_redacted_under_builtins() {
    let e = env();
    let p = builtin_policy(&e);
    let w = check_provider_gating(Some(&p), true, Some("1"))
        .unwrap()
        .expect("warning");
    assert!(w.contains("not redacted"), "{w}");
    assert!(w.contains("HONE_ALLOW_UNGATED_PROVIDER=1"), "{w}");
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
