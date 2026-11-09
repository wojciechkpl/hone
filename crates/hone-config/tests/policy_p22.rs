//! P2.2 additions to the policy model (`docs/plans/policy-p2.2-spec.md` section 5).
//!
//! Assumed signature: `PolicyContext::for_platform(workdir: PathBuf, home: Option<PathBuf>)`.

use std::path::PathBuf;

use hone_config::Decision::{self, Allow, Ask, Deny};
use hone_config::policy::{
    CompiledPolicy, OpFilter, PathOp, PolicyContext, PolicyRule, Subject, load_policy_file,
};

fn ctx(case_insensitive_paths: bool) -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: Some(PathBuf::from("/home/tester")),
        case_insensitive_paths,
    }
}

fn read(p: &str) -> Subject {
    Subject::Path {
        path: PathBuf::from(p),
        op: PathOp::Read,
    }
}

fn decide(c: &PolicyContext, rules: &[PolicyRule], s: &Subject) -> Decision {
    CompiledPolicy::compile(c, &[], rules, &[])
        .expect("compiles")
        .evaluate(s)
        .decision
}

// -- case-insensitive path rules ---------------------------------------------

#[test]
fn case_variants_of_a_home_dir_rule_match_only_when_flag_is_set() {
    let rules = [PolicyRule::path("~/.ssh/**", OpFilter::Any, Deny)];
    for variant in [
        "/home/tester/.SSH/key1",
        "/home/tester/.Ssh/key1",
        "/home/tester/.ssh/KEY1",
    ] {
        assert_eq!(
            decide(&ctx(true), &rules, &read(variant)),
            Deny,
            "flag on: {variant}"
        );
        let expect_off = if variant.ends_with("KEY1") {
            Deny
        } else {
            Allow
        };
        assert_eq!(
            decide(&ctx(false), &rules, &read(variant)),
            expect_off,
            "flag off: {variant}"
        );
    }
}

#[test]
fn flag_off_keeps_path_rules_case_sensitive() {
    let rules = [PolicyRule::path("~/.ssh/**", OpFilter::Any, Deny)];
    assert_eq!(
        decide(&ctx(false), &rules, &read("/home/tester/.SSH/key1")),
        Allow
    );
    assert_eq!(
        decide(&ctx(false), &rules, &read("/home/tester/.ssh/key1")),
        Deny
    );
}

#[test]
fn flag_on_matches_mixed_case_pattern_against_lowercase_subject() {
    let rules = [PolicyRule::path("{workdir}/Secrets/**", OpFilter::Any, Ask)];
    assert_eq!(
        decide(&ctx(true), &rules, &read("/work/proj/secrets/a")),
        Ask
    );
    assert_eq!(
        decide(&ctx(true), &rules, &read("/work/proj/SECRETS/a")),
        Ask
    );
    assert_eq!(
        decide(&ctx(false), &rules, &read("/work/proj/secrets/a")),
        Allow
    );
}

#[test]
fn flag_on_applies_to_double_star_prefix_patterns() {
    let rules = [PolicyRule::path("**/.env", OpFilter::Any, Deny)];
    assert_eq!(decide(&ctx(true), &rules, &read("/work/proj/a/.ENV")), Deny);
    assert_eq!(
        decide(&ctx(false), &rules, &read("/work/proj/a/.ENV")),
        Allow
    );
}

#[test]
fn flag_on_does_not_change_command_matching() {
    let rules = [
        PolicyRule::command("git push", Deny),
        PolicyRule::command("RM *", Deny),
    ];
    let c = ctx(true);
    assert_eq!(
        decide(&c, &rules, &Subject::Command("GIT push".into())),
        Allow
    );
    assert_eq!(decide(&c, &rules, &Subject::Command("rm x".into())), Allow);
    assert_eq!(
        decide(&c, &rules, &Subject::Command("git push".into())),
        Deny
    );
    assert_eq!(decide(&c, &rules, &Subject::Command("RM x".into())), Deny);
}

#[test]
fn flag_on_does_not_change_host_matching() {
    let rules = [PolicyRule::host("example.com", Deny)];
    let on = ctx(true);
    let off = ctx(false);
    for s in ["example.com", "EXAMPLE.com", "example.org"] {
        assert_eq!(
            decide(&on, &rules, &Subject::Host(s.into())),
            decide(&off, &rules, &Subject::Host(s.into())),
            "{s}"
        );
    }
}

#[test]
fn flag_on_still_applies_op_filter() {
    let rules = [PolicyRule::path("/work/proj/F", OpFilter::Write, Deny)];
    let w = Subject::Path {
        path: PathBuf::from("/work/proj/f"),
        op: PathOp::Write,
    };
    assert_eq!(decide(&ctx(true), &rules, &w), Deny);
    assert_eq!(decide(&ctx(true), &rules, &read("/work/proj/f")), Allow);
}

// -- for_platform -------------------------------------------------------------

#[test]
fn for_platform_copies_workdir_and_home() {
    let c =
        PolicyContext::for_platform(PathBuf::from("/work/proj"), Some(PathBuf::from("/home/t")));
    assert_eq!(c.workdir, PathBuf::from("/work/proj"));
    assert_eq!(c.home, Some(PathBuf::from("/home/t")));
    let c = PolicyContext::for_platform(PathBuf::from("/w"), None);
    assert_eq!(c.home, None);
}

#[test]
fn for_platform_sets_case_insensitive_per_target_os() {
    let c = PolicyContext::for_platform(PathBuf::from("/w"), None);
    let expected = cfg!(any(target_os = "macos", target_os = "windows"));
    assert_eq!(c.case_insensitive_paths, expected);
}

#[cfg(target_os = "macos")]
#[test]
fn for_platform_on_macos_is_case_insensitive_end_to_end() {
    let c =
        PolicyContext::for_platform(PathBuf::from("/work/proj"), Some(PathBuf::from("/home/t")));
    let rules = [PolicyRule::path("~/.ssh/**", OpFilter::Any, Deny)];
    assert_eq!(decide(&c, &rules, &read("/home/t/.SSH/key1")), Deny);
}

#[cfg(target_os = "linux")]
#[test]
fn for_platform_on_linux_is_case_sensitive_end_to_end() {
    let c =
        PolicyContext::for_platform(PathBuf::from("/work/proj"), Some(PathBuf::from("/home/t")));
    let rules = [PolicyRule::path("~/.ssh/**", OpFilter::Any, Deny)];
    assert_eq!(decide(&c, &rules, &read("/home/t/.SSH/key1")), Allow);
}

// -- has_command_rules --------------------------------------------------------

fn has(builtin: &[PolicyRule], user: &[PolicyRule], project: &[PolicyRule]) -> bool {
    CompiledPolicy::compile(&ctx(false), builtin, user, project)
        .unwrap()
        .has_command_rules()
}

#[test]
fn has_command_rules_false_for_empty_policy() {
    assert!(!has(&[], &[], &[]));
}

#[test]
fn has_command_rules_false_when_only_path_and_host_rules() {
    let rules = [
        PolicyRule::path("/x", OpFilter::Any, Deny),
        PolicyRule::host("a.com", Deny),
    ];
    assert!(!has(&rules, &rules, &rules));
}

#[test]
fn has_command_rules_true_for_a_command_rule_in_any_layer() {
    let cmd = [PolicyRule::command("ls", Allow)];
    let other = [PolicyRule::host("a.com", Deny)];
    assert!(has(&cmd, &[], &[]), "builtin");
    assert!(has(&[], &cmd, &[]), "user");
    assert!(has(&[], &[], &cmd), "project");
    assert!(has(&other, &other, &cmd), "mixed, project only");
    assert!(has(&cmd, &other, &other), "mixed, builtin only");
}

#[test]
fn has_command_rules_true_for_v1_match_rules_loaded_from_toml() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("p.toml");
    std::fs::write(&f, "[[rules]]\nmatch = \"git push*\"\ndecision = \"ask\"\n").unwrap();
    let rules = load_policy_file(&f).unwrap();
    assert!(has(&[], &rules, &[]));
}

#[test]
fn has_command_rules_ignores_decision_kind() {
    for d in [Allow, Ask, Deny] {
        assert!(has(&[], &[PolicyRule::command("x", d)], &[]));
    }
}
