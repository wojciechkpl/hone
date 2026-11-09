//! Contract tests for policy model v2 (`docs/plans/policy-v2-spec.md`).
//!
//! Interface assumptions beyond the spec text:
//! - `PermissionError::InvalidRule { layer: Layer, index: usize, reason: String }`
//! - `PermissionError::DuplicateRuleId(String)`
//! - `PermissionError::UnsupportedVersion(<integer>)`
//! - `Layer` and `Decision` implement `Debug + PartialEq + Copy`.
//! - `PolicyRule` and `Subject` implement `Debug`.

use std::path::{Path, PathBuf};

use hone_config::permissions;
use hone_config::policy::{
    CompiledPolicy, Layer, OpFilter, PathOp, PolicyContext, PolicyRule, Subject, Verdict,
    load_policy_file,
};
use hone_config::{Decision, PermissionError};

use Decision::{Allow, Ask, Deny};

// -- helpers ------------------------------------------------------------------

fn ctx() -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: Some(PathBuf::from("/home/tester")),
        case_insensitive_paths: false,
    }
}

fn ctx_no_home() -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: None,
        case_insensitive_paths: false,
    }
}

fn compile(builtin: &[PolicyRule], user: &[PolicyRule], project: &[PolicyRule]) -> CompiledPolicy {
    CompiledPolicy::compile(&ctx(), builtin, user, project).expect("policy should compile")
}

fn compile_err(
    builtin: &[PolicyRule],
    user: &[PolicyRule],
    project: &[PolicyRule],
) -> PermissionError {
    match CompiledPolicy::compile(&ctx(), builtin, user, project) {
        Ok(_) => panic!("expected compile to fail"),
        Err(e) => e,
    }
}

fn user_only(rules: &[PolicyRule]) -> CompiledPolicy {
    compile(&[], rules, &[])
}

fn cmd(s: &str) -> Subject {
    Subject::Command(s.to_string())
}
fn host(s: &str) -> Subject {
    Subject::Host(s.to_string())
}
fn read(p: &str) -> Subject {
    Subject::Path {
        path: PathBuf::from(p),
        op: PathOp::Read,
    }
}
fn write(p: &str) -> Subject {
    Subject::Path {
        path: PathBuf::from(p),
        op: PathOp::Write,
    }
}

fn decision_of(policy: &CompiledPolicy, s: &Subject) -> Decision {
    policy.evaluate(s).decision
}

fn write_file(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

fn assert_verdict(v: &Verdict, d: Decision, id: Option<&str>, layer: Option<Layer>) {
    assert_eq!(v.decision, d, "decision");
    assert_eq!(v.rule_id.as_deref(), id, "rule_id");
    assert_eq!(v.layer, layer, "layer");
}

/// Build rules with zero/multiple matchers or `op` on non-path via TOML,
/// since the constructors can only produce well-formed rules.
fn rules_from_toml(body: &str) -> Vec<PolicyRule> {
    let dir = tempfile::tempdir().unwrap();
    let p = write_file(dir.path(), "p.toml", body);
    load_policy_file(&p).expect("toml should parse (validation happens in compile)")
}

fn assert_invalid_rule(e: &PermissionError, layer: Layer, index: usize) {
    match e {
        PermissionError::InvalidRule {
            layer: l, index: i, ..
        } => {
            assert_eq!(*l, layer, "layer");
            assert_eq!(*i, index, "index");
        }
        other => panic!("expected InvalidRule, got {other:?}"),
    }
}

// -- precedence ---------------------------------------------------------------

#[test]
fn unmatched_subject_is_allow_with_no_rule_id_or_layer() {
    let p = compile(&[], &[], &[]);
    assert_verdict(&p.evaluate(&cmd("anything")), Allow, None, None);
    assert_verdict(&p.evaluate(&read("/x")), Allow, None, None);
    assert_verdict(&p.evaluate(&host("example.com")), Allow, None, None);
}

#[test]
fn builtin_rule_applies_when_user_has_no_match() {
    let p = compile(
        &[PolicyRule::command("rm *", Deny)],
        &[PolicyRule::command("ls", Allow)],
        &[],
    );
    assert_verdict(
        &p.evaluate(&cmd("rm -rf x")),
        Deny,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn user_allow_replaces_builtin_deny() {
    let p = compile(
        &[PolicyRule::command("rm *", Deny)],
        &[PolicyRule::command("rm *", Allow)],
        &[],
    );
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn user_deny_replaces_builtin_allow() {
    let p = compile(
        &[PolicyRule::command("git *", Allow)],
        &[PolicyRule::command("git push*", Deny)],
        &[],
    );
    assert_verdict(
        &p.evaluate(&cmd("git push origin")),
        Deny,
        Some("user:1"),
        Some(Layer::User),
    );
    assert_verdict(
        &p.evaluate(&cmd("git status")),
        Allow,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn user_ask_replaces_builtin_deny_loosening() {
    let p = compile(
        &[PolicyRule::host("*.evil.com", Deny)],
        &[PolicyRule::host("*.evil.com", Ask)],
        &[],
    );
    assert_verdict(
        &p.evaluate(&host("a.evil.com")),
        Ask,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn project_deny_tightens_builtin_ask() {
    let p = compile(
        &[PolicyRule::command("git push*", Ask)],
        &[],
        &[PolicyRule::command("git push*", Deny)],
    );
    assert_verdict(
        &p.evaluate(&cmd("git push")),
        Deny,
        Some("project:1"),
        Some(Layer::Project),
    );
}

#[test]
fn project_ask_tightens_builtin_allow() {
    let p = compile(
        &[PolicyRule::command("cargo *", Allow)],
        &[],
        &[PolicyRule::command("cargo publish*", Ask)],
    );
    assert_verdict(
        &p.evaluate(&cmd("cargo publish")),
        Ask,
        Some("project:1"),
        Some(Layer::Project),
    );
    assert_verdict(
        &p.evaluate(&cmd("cargo build")),
        Allow,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn project_ask_tightens_unmatched_allow() {
    let p = compile(&[], &[], &[PolicyRule::command("make deploy", Ask)]);
    assert_verdict(
        &p.evaluate(&cmd("make deploy")),
        Ask,
        Some("project:1"),
        Some(Layer::Project),
    );
}

#[test]
fn project_tightens_user_allow() {
    let p = compile(
        &[],
        &[PolicyRule::command("rm *", Allow)],
        &[PolicyRule::command("rm *", Deny)],
    );
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("project:1"),
        Some(Layer::Project),
    );
}

#[test]
fn project_allow_never_loosens_builtin_ask() {
    let p = compile(
        &[PolicyRule::command("git push*", Ask)],
        &[],
        &[PolicyRule::command("git push*", Allow)],
    );
    assert_verdict(
        &p.evaluate(&cmd("git push")),
        Ask,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn project_allow_never_loosens_builtin_deny() {
    let p = compile(
        &[PolicyRule::command("rm *", Deny)],
        &[],
        &[PolicyRule::command("rm *", Allow)],
    );
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn project_allow_never_loosens_user_deny() {
    let p = compile(
        &[],
        &[PolicyRule::host("bad.com", Deny)],
        &[PolicyRule::host("bad.com", Allow)],
    );
    assert_verdict(
        &p.evaluate(&host("bad.com")),
        Deny,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn project_ask_does_not_loosen_builtin_deny() {
    let p = compile(
        &[PolicyRule::command("rm *", Deny)],
        &[],
        &[PolicyRule::command("rm *", Ask)],
    );
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn project_rule_equal_to_base_keeps_base_rule_id_and_layer() {
    let p = compile(
        &[PolicyRule::command("git push*", Ask).with_id("builtin:base-ask")],
        &[],
        &[PolicyRule::command("git push*", Ask)],
    );
    assert_verdict(
        &p.evaluate(&cmd("git push")),
        Ask,
        Some("builtin:base-ask"),
        Some(Layer::Builtin),
    );
}

#[test]
fn project_equal_deny_keeps_user_layer() {
    let p = compile(
        &[],
        &[PolicyRule::command("rm *", Deny)],
        &[PolicyRule::command("rm *", Deny)],
    );
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn project_allow_equal_to_unmatched_default_keeps_no_rule() {
    let p = compile(&[], &[], &[PolicyRule::command("ls", Allow)]);
    assert_verdict(&p.evaluate(&cmd("ls")), Allow, None, None);
}

#[test]
fn project_first_match_wins_within_layer_so_later_deny_is_shadowed_by_earlier_allow() {
    let p = compile(
        &[PolicyRule::command("make *", Ask)],
        &[],
        &[
            PolicyRule::command("make *", Allow),
            PolicyRule::command("make *", Deny),
        ],
    );
    assert_verdict(
        &p.evaluate(&cmd("make x")),
        Ask,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn first_match_wins_within_user_layer() {
    let p = user_only(&[
        PolicyRule::command("git *", Allow),
        PolicyRule::command("git push*", Deny),
    ]);
    assert_verdict(
        &p.evaluate(&cmd("git push")),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn first_match_wins_within_builtin_layer() {
    let p = compile(
        &[
            PolicyRule::command("git push*", Deny),
            PolicyRule::command("git *", Allow),
        ],
        &[],
        &[],
    );
    assert_verdict(
        &p.evaluate(&cmd("git push")),
        Deny,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
    assert_verdict(
        &p.evaluate(&cmd("git log")),
        Allow,
        Some("builtin:2"),
        Some(Layer::Builtin),
    );
}

#[test]
fn rules_of_different_kinds_do_not_cross_match() {
    let p = user_only(&[
        PolicyRule::command("example.com", Deny),
        PolicyRule::host("ls", Deny),
        PolicyRule::path("/etc/passwd", OpFilter::Any, Deny),
    ]);
    assert_verdict(&p.evaluate(&host("example.com")), Allow, None, None);
    assert_verdict(&p.evaluate(&cmd("ls")), Allow, None, None);
    assert_verdict(&p.evaluate(&cmd("/etc/passwd")), Allow, None, None);
}

// -- verdict ids and layers ---------------------------------------------------

#[test]
fn explicit_rule_id_is_reported() {
    let p = user_only(&[PolicyRule::command("rm *", Deny).with_id("no-rm")]);
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("no-rm"),
        Some(Layer::User),
    );
}

#[test]
fn auto_ids_are_one_based_per_layer() {
    let p = compile(
        &[
            PolicyRule::command("a", Deny),
            PolicyRule::command("b", Deny),
        ],
        &[
            PolicyRule::command("c", Deny),
            PolicyRule::command("d", Deny),
        ],
        &[
            PolicyRule::command("e", Deny),
            PolicyRule::command("f", Deny),
        ],
    );
    assert_eq!(p.evaluate(&cmd("a")).rule_id.as_deref(), Some("builtin:1"));
    assert_eq!(p.evaluate(&cmd("b")).rule_id.as_deref(), Some("builtin:2"));
    assert_eq!(p.evaluate(&cmd("c")).rule_id.as_deref(), Some("user:1"));
    assert_eq!(p.evaluate(&cmd("d")).rule_id.as_deref(), Some("user:2"));
    assert_eq!(p.evaluate(&cmd("e")).rule_id.as_deref(), Some("project:1"));
    assert_eq!(p.evaluate(&cmd("f")).rule_id.as_deref(), Some("project:2"));
}

#[test]
fn auto_id_index_counts_rules_with_explicit_ids() {
    let p = user_only(&[
        PolicyRule::command("a", Deny).with_id("first"),
        PolicyRule::command("b", Deny),
    ]);
    assert_eq!(p.evaluate(&cmd("b")).rule_id.as_deref(), Some("user:2"));
}

#[test]
fn path_and_host_rules_get_auto_ids_too() {
    let p = user_only(&[
        PolicyRule::host("h.com", Deny),
        PolicyRule::path("/p/x", OpFilter::Any, Deny),
    ]);
    assert_eq!(
        p.evaluate(&host("h.com")).rule_id.as_deref(),
        Some("user:1")
    );
    assert_eq!(p.evaluate(&read("/p/x")).rule_id.as_deref(), Some("user:2"));
}

// -- evaluate_all -------------------------------------------------------------

#[test]
fn evaluate_all_empty_is_allow_without_rule() {
    let p = user_only(&[PolicyRule::command("x", Deny)]);
    assert_verdict(&p.evaluate_all(&[]), Allow, None, None);
}

#[test]
fn evaluate_all_returns_strictest() {
    let p = user_only(&[
        PolicyRule::command("ls", Allow),
        PolicyRule::host("slow.com", Ask),
        PolicyRule::command("rm *", Deny),
    ]);
    let v = p.evaluate_all(&[cmd("ls"), host("slow.com"), cmd("rm x"), cmd("unmatched")]);
    assert_verdict(&v, Deny, Some("user:3"), Some(Layer::User));
}

#[test]
fn evaluate_all_ask_beats_allow_and_unmatched() {
    let p = user_only(&[
        PolicyRule::command("ls", Allow),
        PolicyRule::host("slow.com", Ask),
    ]);
    let v = p.evaluate_all(&[cmd("ls"), cmd("nothing"), host("slow.com")]);
    assert_verdict(&v, Ask, Some("user:2"), Some(Layer::User));
}

#[test]
fn evaluate_all_tie_goes_to_earliest_subject() {
    let p = user_only(&[
        PolicyRule::command("a", Deny),
        PolicyRule::command("b", Deny),
    ]);
    assert_eq!(
        p.evaluate_all(&[cmd("b"), cmd("a")]).rule_id.as_deref(),
        Some("user:2")
    );
    assert_eq!(
        p.evaluate_all(&[cmd("a"), cmd("b")]).rule_id.as_deref(),
        Some("user:1")
    );
}

#[test]
fn evaluate_all_allow_tie_goes_to_earliest_subject() {
    let p = user_only(&[PolicyRule::command("a", Allow)]);
    assert_verdict(
        &p.evaluate_all(&[cmd("unmatched"), cmd("a")]),
        Allow,
        None,
        None,
    );
    assert_verdict(
        &p.evaluate_all(&[cmd("a"), cmd("unmatched")]),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn evaluate_all_applies_layer_precedence_per_subject() {
    let p = compile(
        &[PolicyRule::command("git push*", Ask)],
        &[],
        &[
            PolicyRule::command("git push*", Allow),
            PolicyRule::host("x.com", Deny),
        ],
    );
    let v = p.evaluate_all(&[cmd("git push"), host("x.com")]);
    assert_verdict(&v, Deny, Some("project:2"), Some(Layer::Project));
}

// -- command matching ---------------------------------------------------------

#[test]
fn command_glob_matches_whole_command() {
    let p = user_only(&[PolicyRule::command("cargo *", Deny)]);
    assert_eq!(decision_of(&p, &cmd("cargo build --release")), Deny);
    assert_eq!(
        decision_of(&p, &cmd("cargo")),
        Allow,
        "'cargo *' requires the space"
    );
    assert_eq!(decision_of(&p, &cmd("xcargo build")), Allow);
}

#[test]
fn command_glob_star_crosses_slash_and_spaces() {
    let p = user_only(&[PolicyRule::command("rm *", Deny)]);
    assert_eq!(decision_of(&p, &cmd("rm -rf /a/b/c d")), Deny);
}

#[test]
fn command_glob_question_mark_matches_single_char() {
    let p = user_only(&[PolicyRule::command("ls -?", Deny)]);
    assert_eq!(decision_of(&p, &cmd("ls -l")), Deny);
    assert_eq!(decision_of(&p, &cmd("ls -la")), Allow);
}

#[test]
fn command_literal_matches_exact_and_word_boundary_prefix() {
    let p = user_only(&[PolicyRule::command("git push", Deny)]);
    assert_eq!(decision_of(&p, &cmd("git push")), Deny);
    assert_eq!(decision_of(&p, &cmd("git push origin main")), Deny);
    assert_eq!(decision_of(&p, &cmd("git pushx")), Allow);
    assert_eq!(decision_of(&p, &cmd("git pus")), Allow);
}

#[test]
fn command_literal_accepts_any_whitespace_boundary() {
    let p = user_only(&[PolicyRule::command("git push", Deny)]);
    assert_eq!(decision_of(&p, &cmd("git push\torigin")), Deny);
}

#[test]
fn command_subject_is_trimmed() {
    let p = user_only(&[
        PolicyRule::command("git push", Deny),
        PolicyRule::command("cargo *", Ask),
    ]);
    assert_eq!(decision_of(&p, &cmd("  git push  ")), Deny);
    assert_eq!(decision_of(&p, &cmd("\tcargo build\n")), Ask);
}

#[test]
fn command_matching_is_case_sensitive() {
    let p = user_only(&[
        PolicyRule::command("git push", Deny),
        PolicyRule::command("RM *", Deny),
    ]);
    assert_eq!(decision_of(&p, &cmd("GIT push")), Allow);
    assert_eq!(decision_of(&p, &cmd("rm x")), Allow);
}

// -- path matching ------------------------------------------------------------

fn path_policy(pattern: &str) -> CompiledPolicy {
    user_only(&[PolicyRule::path(pattern, OpFilter::Any, Deny)])
}

#[test]
fn path_tilde_expands_to_home() {
    let p = path_policy("~/.ssh/**");
    assert_eq!(decision_of(&p, &read("/home/tester/.ssh/key1")), Deny);
    assert_eq!(
        decision_of(&p, &read("/home/tester/.ssh/keys/deep/key")),
        Deny
    );
    assert_eq!(decision_of(&p, &read("/home/other/.ssh/key1")), Allow);
    assert_eq!(decision_of(&p, &read("/home/tester/.sshx/key1")), Allow);
}

#[test]
fn path_bare_tilde_is_home_itself() {
    let p = path_policy("~");
    assert_eq!(decision_of(&p, &read("/home/tester")), Deny);
    assert_eq!(decision_of(&p, &read("/home/tester/file")), Allow);
}

#[test]
fn path_tilde_rule_never_matches_when_home_is_none() {
    let p = CompiledPolicy::compile(
        &ctx_no_home(),
        &[],
        &[PolicyRule::path("~/.ssh/**", OpFilter::Any, Deny)],
        &[],
    )
    .expect("compiles even without home");
    for s in [
        read("/home/tester/.ssh/key1"),
        read("/.ssh/key1"),
        read("/work/proj/.ssh/key1"),
        read("~/.ssh/key1"),
    ] {
        assert_eq!(decision_of(&p, &s), Allow, "{s:?}");
    }
}

#[test]
fn path_workdir_placeholder_expands() {
    let p = path_policy("{workdir}/secrets/**");
    assert_eq!(decision_of(&p, &read("/work/proj/secrets/a")), Deny);
    assert_eq!(decision_of(&p, &read("/work/other/secrets/a")), Allow);
}

#[test]
fn path_workdir_placeholder_works_mid_pattern() {
    let p = path_policy("/mnt{workdir}/x");
    assert_eq!(decision_of(&p, &read("/mnt/work/proj/x")), Deny);
}

#[test]
fn path_relative_pattern_is_joined_to_workdir() {
    let p = path_policy("src/**");
    assert_eq!(decision_of(&p, &read("/work/proj/src/a.rs")), Deny);
    assert_eq!(decision_of(&p, &read("/work/proj/other/src/a.rs")), Allow);
    assert_eq!(decision_of(&p, &read("/src/a.rs")), Allow);
}

#[test]
fn path_double_star_prefix_matches_anywhere() {
    let p = path_policy("**/.env");
    assert_eq!(decision_of(&p, &read("/work/proj/.env")), Deny);
    assert_eq!(decision_of(&p, &read("/work/proj/a/b/.env")), Deny);
    assert_eq!(decision_of(&p, &read("/home/tester/.env")), Deny);
    assert_eq!(decision_of(&p, &read("/work/proj/.env.local")), Allow);
    assert_eq!(decision_of(&p, &read("/work/proj/x.env")), Allow);
}

#[test]
fn path_single_star_does_not_cross_slash() {
    let p = path_policy("/work/proj/*.txt");
    assert_eq!(decision_of(&p, &read("/work/proj/a.txt")), Deny);
    assert_eq!(decision_of(&p, &read("/work/proj/d/a.txt")), Allow);
}

#[test]
fn path_question_mark_does_not_cross_slash() {
    let p = path_policy("/work/proj/a?b");
    assert_eq!(decision_of(&p, &read("/work/proj/axb")), Deny);
    assert_eq!(decision_of(&p, &read("/work/proj/a/b")), Allow);
}

#[test]
fn path_double_star_crosses_slash() {
    let p = path_policy("/work/proj/**/x.txt");
    assert_eq!(decision_of(&p, &read("/work/proj/a/b/c/x.txt")), Deny);
}

#[test]
fn path_dir_double_star_also_matches_the_dir_itself() {
    let p = path_policy("~/.ssh/**");
    assert_eq!(decision_of(&p, &read("/home/tester/.ssh")), Deny);
    assert_eq!(decision_of(&p, &read("/home/tester/.ssh/")), Deny);
}

#[test]
fn path_matching_is_case_sensitive() {
    let p = path_policy("/work/proj/Secret");
    assert_eq!(decision_of(&p, &read("/work/proj/secret")), Allow);
    assert_eq!(decision_of(&p, &read("/work/proj/Secret")), Deny);
}

#[test]
fn path_relative_subject_is_joined_to_workdir() {
    let p = path_policy("{workdir}/src/**");
    assert_eq!(decision_of(&p, &read("src/main.rs")), Deny);
    assert_eq!(decision_of(&p, &read("./src/main.rs")), Deny);
    assert_eq!(decision_of(&p, &read("docs/main.rs")), Allow);
}

#[test]
fn path_subject_dot_segments_are_removed() {
    let p = path_policy("/work/proj/a/b");
    assert_eq!(decision_of(&p, &read("/work/proj/./a/./b")), Deny);
}

#[test]
fn path_subject_dotdot_is_lexically_normalised() {
    let p = path_policy("/work/proj/a/b");
    assert_eq!(decision_of(&p, &read("/work/proj/a/x/../b")), Deny);
    assert_eq!(decision_of(&p, &read("/work/proj/a/b/../b")), Deny);
}

#[test]
fn path_dotdot_escape_from_workdir_reaches_protected_home_dir() {
    let p = path_policy("~/.ssh/**");
    // /work/proj/../../home/tester/.ssh/key1 == /home/tester/.ssh/key1
    assert_eq!(
        decision_of(&p, &read("/work/proj/../../home/tester/.ssh/key1")),
        Deny
    );
    // relative form is joined onto the workdir first
    assert_eq!(decision_of(&p, &read("../../home/tester/.ssh/key1")), Deny);
}

#[test]
fn path_dotdot_that_leaves_the_protected_tree_does_not_match() {
    let p = path_policy("{workdir}/src/**");
    // -> /work/etc/x
    assert_eq!(decision_of(&p, &read("/work/proj/src/../../etc/x")), Allow);
}

#[test]
fn path_dotdot_above_root_stays_at_root() {
    let p = path_policy("/etc/passwd");
    assert_eq!(decision_of(&p, &read("/../../etc/passwd")), Deny);
    assert_eq!(
        decision_of(&p, &read("/work/proj/../../../../../etc/passwd")),
        Deny
    );
}

#[test]
fn path_trailing_slash_is_dropped() {
    let p = path_policy("/work/proj/build");
    assert_eq!(decision_of(&p, &read("/work/proj/build/")), Deny);
}

#[test]
fn path_pattern_with_dotdot_via_workdir_placeholder() {
    // Pattern text is expanded, not normalised: this pattern is only reachable
    // by subjects whose normalised form equals the pattern text, so the
    // pattern must be written in normalised form to match. Pin the normalised case.
    let p = path_policy("/work/sibling/.ssh/**");
    assert_eq!(
        decision_of(&p, &read("/work/proj/../sibling/.ssh/key1")),
        Deny
    );
}

#[test]
fn path_op_filter_table() {
    // (rule op, subject op, matches?)
    let cases = [
        (OpFilter::Any, PathOp::Read, true),
        (OpFilter::Any, PathOp::Write, true),
        (OpFilter::Read, PathOp::Read, true),
        (OpFilter::Read, PathOp::Write, false),
        (OpFilter::Write, PathOp::Read, false),
        (OpFilter::Write, PathOp::Write, true),
    ];
    for (rule_op, subj_op, expect) in cases {
        let p = user_only(&[PolicyRule::path("/work/proj/f", rule_op, Deny)]);
        let d = decision_of(
            &p,
            &Subject::Path {
                path: PathBuf::from("/work/proj/f"),
                op: subj_op,
            },
        );
        assert_eq!(d == Deny, expect, "rule {rule_op:?} vs subject {subj_op:?}");
    }
}

#[test]
fn path_rule_without_op_in_toml_defaults_to_any() {
    let rules = rules_from_toml("[[rules]]\npath = \"/work/proj/f\"\ndecision = \"deny\"\n");
    let p = user_only(&rules);
    assert_eq!(decision_of(&p, &read("/work/proj/f")), Deny);
    assert_eq!(decision_of(&p, &write("/work/proj/f")), Deny);
}

#[test]
fn path_read_only_deny_lets_write_fall_through_to_next_rule() {
    let p = user_only(&[
        PolicyRule::path("/work/proj/f", OpFilter::Read, Deny),
        PolicyRule::path("/work/proj/f", OpFilter::Write, Ask),
    ]);
    assert_verdict(
        &p.evaluate(&read("/work/proj/f")),
        Deny,
        Some("user:1"),
        Some(Layer::User),
    );
    assert_verdict(
        &p.evaluate(&write("/work/proj/f")),
        Ask,
        Some("user:2"),
        Some(Layer::User),
    );
}

// -- host matching ------------------------------------------------------------

#[test]
fn host_match_is_case_insensitive_for_subject_and_pattern() {
    let p = user_only(&[PolicyRule::host("Example.COM", Deny)]);
    assert_eq!(decision_of(&p, &host("EXAMPLE.com")), Deny);
    assert_eq!(decision_of(&p, &host("example.com")), Deny);
}

#[test]
fn host_trailing_dot_is_stripped() {
    let p = user_only(&[PolicyRule::host("example.com", Deny)]);
    assert_eq!(decision_of(&p, &host("example.com.")), Deny);
    let p2 = user_only(&[PolicyRule::host("example.com.", Deny)]);
    assert_eq!(decision_of(&p2, &host("example.com")), Deny);
}

#[test]
fn host_port_suffix_is_stripped() {
    let p = user_only(&[PolicyRule::host("example.com", Deny)]);
    assert_eq!(decision_of(&p, &host("example.com:8080")), Deny);
    assert_eq!(decision_of(&p, &host("example.com:443")), Deny);
    assert_eq!(decision_of(&p, &host("example.org:443")), Allow);
}

#[test]
fn host_wildcard_matches_subdomains_across_dots_but_not_apex() {
    let p = user_only(&[PolicyRule::host("*.example.com", Deny)]);
    assert_eq!(decision_of(&p, &host("a.example.com")), Deny);
    assert_eq!(decision_of(&p, &host("a.b.example.com")), Deny);
    assert_eq!(decision_of(&p, &host("a.example.com:8443")), Deny);
    assert_eq!(decision_of(&p, &host("example.com")), Allow);
    assert_eq!(decision_of(&p, &host("badexample.com")), Allow);
}

#[test]
fn host_ipv6_literal_is_not_port_stripped() {
    let p = user_only(&[
        PolicyRule::host("::1", Deny),
        PolicyRule::host("2001:db8::1", Ask),
    ]);
    assert_eq!(decision_of(&p, &host("::1")), Deny);
    assert_eq!(decision_of(&p, &host("2001:db8::1")), Ask);
}

#[test]
fn host_ipv6_literal_with_multiple_colons_is_left_untouched() {
    // If "fe80::1" were treated as host "fe80" + port, a rule for "fe80" would match.
    let p = user_only(&[PolicyRule::host("fe80", Deny)]);
    assert_eq!(decision_of(&p, &host("fe80::1")), Allow);
}

// -- rule id validation -------------------------------------------------------

#[test]
fn explicit_id_empty_is_invalid_rule() {
    let e = compile_err(&[], &[PolicyRule::command("a", Allow).with_id("")], &[]);
    assert_invalid_rule(&e, Layer::User, 1);
}

#[test]
fn explicit_id_with_whitespace_is_invalid_rule() {
    for bad in ["has space", "tab\tid", " lead", "trail ", "new\nline"] {
        let e = compile_err(
            &[],
            &[],
            &[
                PolicyRule::command("ok", Allow),
                PolicyRule::command("a", Allow).with_id(bad),
            ],
        );
        assert_invalid_rule(&e, Layer::Project, 2);
    }
}

#[test]
fn explicit_id_with_colon_is_invalid_rule() {
    let e = compile_err(
        &[PolicyRule::command("a", Allow).with_id("user:2")],
        &[],
        &[],
    );
    assert_invalid_rule(&e, Layer::Builtin, 1);
}

#[test]
fn valid_explicit_ids_are_accepted() {
    let p = user_only(&[
        PolicyRule::command("a", Allow).with_id("allow-cargo"),
        PolicyRule::command("b", Allow).with_id("rule_2.x"),
    ]);
    assert_eq!(p.evaluate(&cmd("b")).rule_id.as_deref(), Some("rule_2.x"));
}

#[test]
fn duplicate_explicit_id_across_layers_is_rejected() {
    let e = compile_err(
        &[],
        &[PolicyRule::command("a", Deny).with_id("same")],
        &[PolicyRule::command("b", Deny).with_id("same")],
    );
    match e {
        PermissionError::DuplicateRuleId(id) => assert_eq!(id, "same"),
        other => panic!("expected DuplicateRuleId, got {other:?}"),
    }
}

#[test]
fn duplicate_explicit_id_within_a_layer_is_rejected() {
    let e = compile_err(
        &[],
        &[
            PolicyRule::command("a", Deny).with_id("dup"),
            PolicyRule::host("b", Deny).with_id("dup"),
        ],
        &[],
    );
    assert!(
        matches!(e, PermissionError::DuplicateRuleId(ref id) if id == "dup"),
        "{e:?}"
    );
}

// -- rule validation ----------------------------------------------------------

#[test]
fn rule_with_no_matcher_is_invalid_rule() {
    let rules = rules_from_toml("[[rules]]\ndecision = \"deny\"\n");
    assert_invalid_rule(&compile_err(&[], &rules, &[]), Layer::User, 1);
}

#[test]
fn rule_with_multiple_matchers_is_invalid_rule() {
    let combos = [
        "command = \"a\"\nhost = \"b\"",
        "command = \"a\"\npath = \"/b\"",
        "path = \"/a\"\nhost = \"b\"",
        "command = \"a\"\nmatch = \"b\"",
        "command = \"a\"\npath = \"/b\"\nhost = \"c\"",
    ];
    for combo in combos {
        let rules = rules_from_toml(&format!(
            "[[rules]]\ncommand = \"ok\"\ndecision = \"allow\"\n\n[[rules]]\n{combo}\ndecision = \"deny\"\n"
        ));
        assert_invalid_rule(&compile_err(&[], &rules, &[]), Layer::User, 2);
        assert_invalid_rule(&compile_err(&[], &[], &rules), Layer::Project, 2);
    }
}

#[test]
fn op_on_command_rule_is_invalid_rule() {
    let rules = rules_from_toml("[[rules]]\ncommand = \"a\"\nop = \"read\"\ndecision = \"deny\"\n");
    assert_invalid_rule(&compile_err(&[], &rules, &[]), Layer::User, 1);
}

#[test]
fn op_on_host_rule_is_invalid_rule() {
    let rules = rules_from_toml("[[rules]]\nhost = \"a\"\nop = \"write\"\ndecision = \"deny\"\n");
    assert_invalid_rule(&compile_err(&rules, &[], &[]), Layer::Builtin, 1);
}

#[test]
fn op_on_v1_match_rule_is_invalid_rule() {
    let rules = rules_from_toml("[[rules]]\nmatch = \"a\"\nop = \"read\"\ndecision = \"deny\"\n");
    assert_invalid_rule(&compile_err(&[], &rules, &[]), Layer::User, 1);
}

#[test]
fn empty_pattern_is_invalid_rule_for_every_kind() {
    for rule in [
        PolicyRule::command("", Deny),
        PolicyRule::path("", OpFilter::Any, Deny),
        PolicyRule::host("", Deny),
    ] {
        let e = compile_err(&[], &[PolicyRule::command("ok", Allow), rule], &[]);
        assert_invalid_rule(&e, Layer::User, 2);
    }
}

#[test]
fn invalid_rule_index_is_per_layer_and_one_based() {
    let e = compile_err(
        &[PolicyRule::command("ok", Allow)],
        &[
            PolicyRule::command("ok", Allow),
            PolicyRule::command("ok2", Allow),
        ],
        &[
            PolicyRule::command("ok", Allow),
            PolicyRule::command("ok2", Allow),
            PolicyRule::command("", Deny),
        ],
    );
    assert_invalid_rule(&e, Layer::Project, 3);
}

#[test]
fn invalid_rule_error_has_nonempty_reason() {
    let e = compile_err(&[], &[PolicyRule::command("", Deny)], &[]);
    match e {
        PermissionError::InvalidRule { reason, .. } => assert!(!reason.is_empty()),
        other => panic!("expected InvalidRule, got {other:?}"),
    }
}

#[test]
fn bad_command_glob_is_invalid_glob() {
    let e = compile_err(&[], &[PolicyRule::command("cargo [*", Deny)], &[]);
    assert!(matches!(e, PermissionError::InvalidGlob { .. }), "{e:?}");
}

#[test]
fn bad_path_glob_is_invalid_glob() {
    let e = compile_err(
        &[],
        &[PolicyRule::path("/x/[abc", OpFilter::Any, Deny)],
        &[],
    );
    assert!(matches!(e, PermissionError::InvalidGlob { .. }), "{e:?}");
}

#[test]
fn bad_host_glob_is_invalid_glob() {
    let e = compile_err(&[], &[PolicyRule::host("[*.com", Deny)], &[]);
    assert!(matches!(e, PermissionError::InvalidGlob { .. }), "{e:?}");
}

#[test]
fn invalid_glob_in_project_layer_also_fails_compile() {
    let e = compile_err(&[], &[], &[PolicyRule::host("[*.com", Deny)]);
    assert!(matches!(e, PermissionError::InvalidGlob { .. }), "{e:?}");
}

// -- TOML loading -------------------------------------------------------------

#[test]
fn v2_toml_with_all_three_kinds_loads_and_evaluates() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(
        dir.path(),
        "p.toml",
        r#"
version = 2

[[rules]]
id = "allow-cargo"
command = "cargo *"
decision = "allow"

[[rules]]
path = "~/.ssh/**"
op = "read"
decision = "deny"

[[rules]]
host = "*.pypi.org"
decision = "allow"

[[rules]]
match = "git push*"
decision = "ask"
"#,
    );
    let rules = load_policy_file(&f).unwrap();
    assert_eq!(rules.len(), 4);
    let p = user_only(&rules);
    assert_verdict(
        &p.evaluate(&cmd("cargo build")),
        Allow,
        Some("allow-cargo"),
        Some(Layer::User),
    );
    assert_verdict(
        &p.evaluate(&read("/home/tester/.ssh/key1")),
        Deny,
        Some("user:2"),
        Some(Layer::User),
    );
    assert_eq!(
        decision_of(&p, &write("/home/tester/.ssh/key1")),
        Allow,
        "op=read does not cover writes"
    );
    assert_verdict(
        &p.evaluate(&host("files.pypi.org")),
        Allow,
        Some("user:3"),
        Some(Layer::User),
    );
    assert_verdict(
        &p.evaluate(&cmd("git push origin")),
        Ask,
        Some("user:4"),
        Some(Layer::User),
    );
}

#[test]
fn v2_toml_op_values_parse() {
    for (op, subj_op, expect) in [
        ("read", PathOp::Read, true),
        ("write", PathOp::Write, true),
        ("write", PathOp::Read, false),
        ("any", PathOp::Write, true),
    ] {
        let rules = rules_from_toml(&format!(
            "[[rules]]\npath = \"/f\"\nop = \"{op}\"\ndecision = \"deny\"\n"
        ));
        let p = user_only(&rules);
        let d = decision_of(
            &p,
            &Subject::Path {
                path: PathBuf::from("/f"),
                op: subj_op,
            },
        );
        assert_eq!(d == Deny, expect, "op={op}");
    }
}

#[test]
fn v1_toml_without_version_loads_as_command_rules_and_evaluates_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(
        dir.path(),
        "v1.toml",
        r#"
[[rules]]
match = "cargo test"
decision = "allow"

[[rules]]
match = "cargo *"
decision = "deny"

[[rules]]
match = "git push"
decision = "ask"
"#,
    );
    let rules = load_policy_file(&f).unwrap();
    assert_eq!(rules.len(), 3);

    // Compare against the v1 engine on the same file.
    let v1 = permissions::PermissionPolicy::load_from_toml(&f).unwrap();
    let p = user_only(&rules);
    for c in [
        "cargo test",
        "cargo test --lib",
        "cargo build",
        "git push",
        "git push origin",
        "git pushx",
        "ls",
    ] {
        match v1.rules.iter().find(|r| r.matches(c)).map(|r| r.decision) {
            Some(d) => assert_eq!(decision_of(&p, &cmd(c)), d, "{c}"),
            None => assert_eq!(decision_of(&p, &cmd(c)), Allow, "{c} unmatched"),
        }
    }
    assert_eq!(decision_of(&p, &cmd("cargo test")), Allow);
    assert_eq!(decision_of(&p, &cmd("cargo build")), Deny);
}

#[test]
fn version_1_explicit_is_accepted() {
    let rules = rules_from_toml("version = 1\n[[rules]]\nmatch = \"ls\"\ndecision = \"deny\"\n");
    assert_eq!(decision_of(&user_only(&rules), &cmd("ls")), Deny);
}

#[test]
fn empty_file_and_version_only_file_yield_no_rules() {
    assert!(rules_from_toml("").is_empty());
    assert!(rules_from_toml("version = 2\n").is_empty());
}

#[test]
fn unknown_key_in_rule_is_toml_error() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(
        dir.path(),
        "p.toml",
        "[[rules]]\ncommand = \"a\"\ndecision = \"deny\"\nbogus = 1\n",
    );
    assert!(matches!(
        load_policy_file(&f),
        Err(PermissionError::Toml(_))
    ));
}

#[test]
fn unknown_top_level_key_is_toml_error() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(
        dir.path(),
        "p.toml",
        "bogus = 1\n[[rules]]\ncommand = \"a\"\ndecision = \"deny\"\n",
    );
    assert!(matches!(
        load_policy_file(&f),
        Err(PermissionError::Toml(_))
    ));
}

#[test]
fn syntax_error_is_toml_error() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(dir.path(), "p.toml", "[[rules\nthis is not toml");
    assert!(matches!(
        load_policy_file(&f),
        Err(PermissionError::Toml(_))
    ));
}

#[test]
fn invalid_decision_value_is_toml_error() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(
        dir.path(),
        "p.toml",
        "[[rules]]\ncommand = \"a\"\ndecision = \"maybe\"\n",
    );
    assert!(matches!(
        load_policy_file(&f),
        Err(PermissionError::Toml(_))
    ));
}

#[test]
fn version_3_is_unsupported_version() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(dir.path(), "p.toml", "version = 3\n");
    match load_policy_file(&f) {
        Err(PermissionError::UnsupportedVersion(v)) => assert_eq!(v, 3),
        other => panic!("expected UnsupportedVersion(3), got {other:?}"),
    }
}

#[test]
fn version_0_is_unsupported_version() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(dir.path(), "p.toml", "version = 0\n");
    assert!(matches!(
        load_policy_file(&f),
        Err(PermissionError::UnsupportedVersion(_))
    ));
}

#[test]
fn missing_file_is_empty_rules() {
    let dir = tempfile::tempdir().unwrap();
    let rules = load_policy_file(&dir.path().join("nope.toml")).unwrap();
    assert!(rules.is_empty());
}

#[test]
fn toml_explicit_id_reaches_verdict() {
    let rules =
        rules_from_toml("[[rules]]\nid = \"my-id\"\nhost = \"x.com\"\ndecision = \"ask\"\n");
    assert_eq!(
        user_only(&rules)
            .evaluate(&host("x.com"))
            .rule_id
            .as_deref(),
        Some("my-id")
    );
}

#[test]
fn from_v1_rule_makes_equivalent_command_rule() {
    let v1 = permissions::Rule {
        r#match: "git push*".to_string(),
        decision: Ask,
    };
    let r: PolicyRule = PolicyRule::from(&v1);
    let p = user_only(&[r]);
    assert_verdict(
        &p.evaluate(&cmd("git push origin")),
        Ask,
        Some("user:1"),
        Some(Layer::User),
    );
    assert_eq!(decision_of(&p, &cmd("git status")), Allow);
    assert_eq!(
        decision_of(&p, &host("git.example")),
        Allow,
        "must be a command rule, not host"
    );
}

#[test]
fn from_v1_rule_is_not_a_host_rule() {
    let v1 = permissions::Rule {
        r#match: "git.example*".to_string(),
        decision: Ask,
    };
    let p = user_only(&[PolicyRule::from(&v1)]);
    assert_eq!(decision_of(&p, &cmd("git.example x")), Ask);
    assert_eq!(decision_of(&p, &host("git.example")), Allow);
}

// -- load_hierarchical_with_paths ---------------------------------------------

#[test]
fn hierarchical_user_replaces_builtin_and_project_tightens() {
    let dir = tempfile::tempdir().unwrap();
    let user = write_file(
        dir.path(),
        "user.toml",
        "[[rules]]\ncommand = \"rm *\"\ndecision = \"allow\"\n",
    );
    let project = write_file(
        dir.path(),
        "project.toml",
        "version = 2\n[[rules]]\ncommand = \"git push*\"\ndecision = \"deny\"\n\n[[rules]]\ncommand = \"rm *\"\ndecision = \"deny\"\n",
    );
    let builtin = [
        PolicyRule::command("rm *", Deny),
        PolicyRule::command("git push*", Ask),
    ];
    let p =
        CompiledPolicy::load_hierarchical_with_paths(&ctx(), &builtin, &user, &project).unwrap();
    // user allow replaces builtin deny, then project deny tightens it again
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("project:2"),
        Some(Layer::Project),
    );
    assert_verdict(
        &p.evaluate(&cmd("git push")),
        Deny,
        Some("project:1"),
        Some(Layer::Project),
    );
}

#[test]
fn hierarchical_user_file_loosens_builtin() {
    let dir = tempfile::tempdir().unwrap();
    let user = write_file(
        dir.path(),
        "user.toml",
        "[[rules]]\ncommand = \"rm *\"\ndecision = \"allow\"\n",
    );
    let project = dir.path().join("none.toml");
    let builtin = [PolicyRule::command("rm *", Deny)];
    let p =
        CompiledPolicy::load_hierarchical_with_paths(&ctx(), &builtin, &user, &project).unwrap();
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn hierarchical_project_allow_in_file_does_not_loosen() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("missing-user.toml");
    let project = write_file(
        dir.path(),
        "project.toml",
        "[[rules]]\nmatch = \"rm *\"\ndecision = \"allow\"\n",
    );
    let builtin = [PolicyRule::command("rm *", Deny)];
    let p =
        CompiledPolicy::load_hierarchical_with_paths(&ctx(), &builtin, &user, &project).unwrap();
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn hierarchical_user_and_project_layers_come_from_their_own_paths() {
    let dir = tempfile::tempdir().unwrap();
    let user = write_file(
        dir.path(),
        "u.toml",
        "[[rules]]\ncommand = \"u\"\ndecision = \"ask\"\n",
    );
    let project = write_file(
        dir.path(),
        "p.toml",
        "[[rules]]\ncommand = \"p\"\ndecision = \"ask\"\n",
    );
    let p = CompiledPolicy::load_hierarchical_with_paths(&ctx(), &[], &user, &project).unwrap();
    assert_verdict(
        &p.evaluate(&cmd("u")),
        Ask,
        Some("user:1"),
        Some(Layer::User),
    );
    assert_verdict(
        &p.evaluate(&cmd("p")),
        Ask,
        Some("project:1"),
        Some(Layer::Project),
    );
}

#[test]
fn hierarchical_both_files_missing_yields_builtin_only_policy() {
    let dir = tempfile::tempdir().unwrap();
    let p = CompiledPolicy::load_hierarchical_with_paths(
        &ctx(),
        &[PolicyRule::command("rm *", Deny)],
        &dir.path().join("a.toml"),
        &dir.path().join("b.toml"),
    )
    .unwrap();
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
    assert_verdict(&p.evaluate(&cmd("ls")), Allow, None, None);
}

#[test]
fn hierarchical_broken_project_file_is_an_error_not_silently_empty() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("none.toml");
    let project = write_file(dir.path(), "p.toml", "[[rules\ngarbage");
    let r = CompiledPolicy::load_hierarchical_with_paths(&ctx(), &[], &user, &project);
    assert!(matches!(r, Err(PermissionError::Toml(_))));
}

#[test]
fn hierarchical_broken_user_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let user = write_file(dir.path(), "u.toml", "bogus = true\n");
    let project = dir.path().join("none.toml");
    let r = CompiledPolicy::load_hierarchical_with_paths(&ctx(), &[], &user, &project);
    assert!(matches!(r, Err(PermissionError::Toml(_))));
}

#[test]
fn hierarchical_unsupported_version_propagates() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("none.toml");
    let project = write_file(dir.path(), "p.toml", "version = 3\n");
    let r = CompiledPolicy::load_hierarchical_with_paths(&ctx(), &[], &user, &project);
    assert!(matches!(r, Err(PermissionError::UnsupportedVersion(3))));
}

#[test]
fn hierarchical_validation_errors_propagate_with_layer() {
    let dir = tempfile::tempdir().unwrap();
    let user = write_file(
        dir.path(),
        "u.toml",
        "[[rules]]\ncommand = \"a\"\nhost = \"b\"\ndecision = \"deny\"\n",
    );
    let project = dir.path().join("none.toml");
    match CompiledPolicy::load_hierarchical_with_paths(&ctx(), &[], &user, &project) {
        Err(e) => assert_invalid_rule(&e, Layer::User, 1),
        Ok(_) => panic!("expected InvalidRule"),
    }
}

#[test]
fn hierarchical_duplicate_ids_across_files_propagate() {
    let dir = tempfile::tempdir().unwrap();
    let user = write_file(
        dir.path(),
        "u.toml",
        "[[rules]]\nid = \"x\"\ncommand = \"a\"\ndecision = \"deny\"\n",
    );
    let project = write_file(
        dir.path(),
        "p.toml",
        "[[rules]]\nid = \"x\"\ncommand = \"b\"\ndecision = \"deny\"\n",
    );
    let r = CompiledPolicy::load_hierarchical_with_paths(&ctx(), &[], &user, &project);
    assert!(matches!(r, Err(PermissionError::DuplicateRuleId(ref id)) if id == "x"));
}

// -- layer serde --------------------------------------------------------------

#[test]
fn layer_serializes_lowercase() {
    #[derive(serde::Serialize)]
    struct W {
        l: Layer,
    }
    for (l, s) in [
        (Layer::Builtin, "builtin"),
        (Layer::User, "user"),
        (Layer::Project, "project"),
    ] {
        let out = toml::to_string(&W { l }).unwrap();
        assert_eq!(out.trim(), format!("l = \"{s}\""));
    }
}
