//! P2.3 additions to `CompiledPolicy` (`docs/plans/policy-p2.3-spec.md` sections 3 and 7):
//! `has_rule_id`, `is_empty`, `is_catch_all`.

use std::path::PathBuf;

use hone_config::Decision::{Allow, Ask, Deny};
use hone_config::policy::{CompiledPolicy, OpFilter, PolicyContext, PolicyRule};

fn ctx() -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: Some(PathBuf::from("/home/tester")),
        case_insensitive_paths: false,
    }
}

fn compile(builtin: &[PolicyRule], user: &[PolicyRule], project: &[PolicyRule]) -> CompiledPolicy {
    CompiledPolicy::compile(&ctx(), builtin, user, project).unwrap()
}

// -- is_empty -------------------------------------------------------------------

#[test]
fn is_empty_true_when_no_layer_has_rules() {
    assert!(compile(&[], &[], &[]).is_empty());
}

#[test]
fn is_empty_false_when_any_layer_has_a_rule() {
    let r = [PolicyRule::command("ls", Allow)];
    assert!(!compile(&r, &[], &[]).is_empty(), "builtin");
    assert!(!compile(&[], &r, &[]).is_empty(), "user");
    assert!(!compile(&[], &[], &r).is_empty(), "project");
}

#[test]
fn is_empty_false_for_each_rule_kind() {
    for rule in [
        PolicyRule::command("ls", Allow),
        PolicyRule::path("/x", OpFilter::Any, Deny),
        PolicyRule::host("a.com", Ask),
    ] {
        assert!(!compile(&[], &[rule], &[]).is_empty());
    }
}

// -- has_rule_id ----------------------------------------------------------------

#[test]
fn has_rule_id_finds_explicit_ids_in_every_layer() {
    let p = compile(
        &[PolicyRule::command("a", Allow).with_id("builtin:b-rule")],
        &[PolicyRule::command("b", Allow).with_id("u-rule")],
        &[PolicyRule::command("c", Allow).with_id("p-rule")],
    );
    for id in ["builtin:b-rule", "u-rule", "p-rule"] {
        assert!(p.has_rule_id(id), "{id}");
    }
}

#[test]
fn has_rule_id_finds_automatic_ids() {
    let p = compile(
        &[PolicyRule::command("a", Allow)],
        &[
            PolicyRule::command("b", Allow),
            PolicyRule::host("h.com", Ask),
        ],
        &[PolicyRule::path("/x", OpFilter::Any, Deny)],
    );
    for id in ["builtin:1", "user:1", "user:2", "project:1"] {
        assert!(p.has_rule_id(id), "{id}");
    }
}

#[test]
fn has_rule_id_auto_index_counts_rules_with_explicit_ids() {
    let p = compile(
        &[],
        &[
            PolicyRule::command("a", Allow).with_id("first"),
            PolicyRule::command("b", Allow),
        ],
        &[],
    );
    assert!(p.has_rule_id("first"));
    assert!(p.has_rule_id("user:2"));
    assert!(
        !p.has_rule_id("user:1"),
        "rule 1 carries an explicit id instead"
    );
}

#[test]
fn has_rule_id_false_for_unknown_ids() {
    let p = compile(&[], &[PolicyRule::command("a", Allow)], &[]);
    for id in [
        "",
        "nope",
        "user:2",
        "user:0",
        "project:1",
        "builtin:1",
        "USER:1",
        "user:1 ",
    ] {
        assert!(!p.has_rule_id(id), "{id:?}");
    }
}

#[test]
fn has_rule_id_false_on_empty_policy() {
    assert!(!compile(&[], &[], &[]).has_rule_id("user:1"));
}

#[test]
fn has_rule_id_does_not_know_synthetic_ids() {
    // The synthetic ids are accepted by --approve validation separately; the policy
    // itself has no such rules.
    let p = compile(&[], &[PolicyRule::command("a", Allow)], &[]);
    assert!(!p.has_rule_id("builtin:unparsed-command"));
    assert!(!p.has_rule_id("builtin:invalid-host"));
}

// -- is_catch_all ---------------------------------------------------------------

fn catch_all(rule: PolicyRule) -> bool {
    compile(&[], &[rule.with_id("r")], &[]).is_catch_all("r")
}

#[test]
fn host_catch_all_examples() {
    for pat in ["*", "**", "*.*", "*.*.*"] {
        assert!(catch_all(PolicyRule::host(pat, Ask)), "host {pat}");
    }
}

#[test]
fn path_catch_all_examples() {
    for pat in ["*", "**", "/**", "**/*", "/**/*", "*.*", "/*"] {
        assert!(
            catch_all(PolicyRule::path(pat, OpFilter::Any, Ask)),
            "path {pat}"
        );
    }
}

#[test]
fn command_catch_all_only_for_a_lone_star() {
    assert!(catch_all(PolicyRule::command("*", Ask)));
    for pat in ["git *", "*push", "a*", "ls", "cargo*", "* *"] {
        assert!(!catch_all(PolicyRule::command(pat, Ask)), "command {pat}");
    }
}

#[test]
fn host_non_catch_all_examples() {
    for pat in ["*.example.com", "a*", "example.com", "*.org", "*-x"] {
        assert!(!catch_all(PolicyRule::host(pat, Ask)), "host {pat}");
    }
}

#[test]
fn path_non_catch_all_examples() {
    for pat in [
        "/work/**",
        "**/.env",
        "~/.ssh/**",
        "{workdir}/**",
        "**/*.rs",
        "/etc/*",
        "src/**",
    ] {
        assert!(
            !catch_all(PolicyRule::path(pat, OpFilter::Any, Ask)),
            "path {pat}"
        );
    }
}

#[test]
fn is_catch_all_false_for_unknown_rule_ids() {
    let p = compile(&[], &[PolicyRule::host("*", Ask).with_id("r")], &[]);
    assert!(p.is_catch_all("r"));
    assert!(!p.is_catch_all("nope"));
    assert!(!p.is_catch_all(""));
}

#[test]
fn is_catch_all_works_with_automatic_ids_and_across_layers() {
    let p = compile(
        &[PolicyRule::host("*", Ask)],
        &[PolicyRule::host("a.com", Ask)],
        &[PolicyRule::host("**", Ask)],
    );
    assert!(p.is_catch_all("builtin:1"));
    assert!(!p.is_catch_all("user:1"));
    assert!(p.is_catch_all("project:1"));
}

#[test]
fn is_catch_all_ignores_decision_and_op() {
    for d in [Allow, Ask, Deny] {
        assert!(catch_all(PolicyRule::host("*", d)));
    }
    for op in [OpFilter::Read, OpFilter::Write, OpFilter::Any] {
        assert!(catch_all(PolicyRule::path("/**", op, Ask)));
    }
}

// -- is_literal (round 2) -----------------------------------------------------------

fn literal(rule: PolicyRule) -> bool {
    compile(&[], &[rule.with_id("r")], &[]).is_literal("r")
}

#[test]
fn is_literal_true_for_patterns_without_glob_metacharacters() {
    assert!(literal(PolicyRule::command("git push", Ask)));
    assert!(literal(PolicyRule::command("make deploy", Ask)));
    assert!(literal(PolicyRule::path("/etc/hosts", OpFilter::Any, Ask)));
    assert!(literal(PolicyRule::host("example.com", Ask)));
}

#[test]
fn is_literal_false_for_any_wildcard() {
    for pat in ["git push*", "* *", "**", "*", "ls -?", "ls [ab]", "a{b,c}"] {
        assert!(!literal(PolicyRule::command(pat, Ask)), "command {pat}");
    }
    for pat in [
        "*.example",
        "host?.example",
        "[ab].example",
        "{a,b}.example",
    ] {
        assert!(!literal(PolicyRule::host(pat, Ask)), "host {pat}");
    }
    for pat in [
        "{workdir}/**",
        "~/**",
        "/etc/*",
        "**/.env",
        "/a/b?",
        "/a/[x]",
    ] {
        assert!(
            !literal(PolicyRule::path(pat, OpFilter::Any, Ask)),
            "path {pat}"
        );
    }
}

#[test]
fn is_literal_false_for_unknown_rule_ids() {
    let p = compile(&[], &[PolicyRule::command("ls", Ask).with_id("r")], &[]);
    assert!(p.is_literal("r"));
    assert!(!p.is_literal("nope"));
    assert!(!p.is_literal(""));
}

#[test]
fn is_literal_works_with_automatic_ids_and_across_layers() {
    let p = compile(
        &[PolicyRule::host("a.com", Ask)],
        &[PolicyRule::host("*.com", Ask)],
        &[PolicyRule::command("ls", Ask)],
    );
    assert!(p.is_literal("builtin:1"));
    assert!(!p.is_literal("user:1"));
    assert!(p.is_literal("project:1"));
}

#[test]
fn is_literal_is_independent_of_decision() {
    for d in [Allow, Ask, Deny] {
        assert!(literal(PolicyRule::command("ls", d)));
        assert!(!literal(PolicyRule::command("ls*", d)));
    }
}
