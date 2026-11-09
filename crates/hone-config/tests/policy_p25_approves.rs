#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! `CompiledPolicy::approves_by_id` (`docs/plans/policy-p2.5-spec.md` section 14.6): true only
//! for a literal host rule, or a literal path rule with ONE pattern and an explicit op
//! (`read` or `write`).
//!
//! Multi-pattern rules are built with `PolicyRule::or_path` (existing public API).

use std::path::PathBuf;

use hone_config::Decision::Ask;
use hone_config::policy::{CompiledPolicy, OpFilter, PolicyContext, PolicyRule};

fn ctx() -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: Some(PathBuf::from("/home/tester")),
        case_insensitive_paths: false,
    }
}

fn approves(rule: PolicyRule) -> bool {
    CompiledPolicy::compile(&ctx(), &[], &[rule.with_id("r")], &[])
        .unwrap()
        .approves_by_id("r")
}

#[test]
fn literal_command_rule_is_not_approved_by_id() {
    assert!(!approves(PolicyRule::command("git push", Ask)));
    assert!(!approves(PolicyRule::command("make deploy", Ask)));
}

#[test]
fn wildcard_command_rule_is_not_approved_by_id() {
    assert!(!approves(PolicyRule::command("git push*", Ask)));
}

#[test]
fn literal_host_rule_is_approved_by_id() {
    assert!(approves(PolicyRule::host("example.com", Ask)));
}

#[test]
fn literal_single_pattern_path_rule_with_explicit_op_is_approved_by_id() {
    assert!(approves(PolicyRule::path("/etc/hosts", OpFilter::Read, Ask)));
    assert!(approves(PolicyRule::path(
        "/etc/hosts",
        OpFilter::Write,
        Ask
    )));
}

#[test]
fn literal_path_rule_with_op_any_is_not_approved_by_id() {
    assert!(!approves(PolicyRule::path("/etc/hosts", OpFilter::Any, Ask)));
}

#[test]
fn multi_pattern_path_rule_is_not_approved_by_id() {
    let r = PolicyRule::path("/etc/hosts", OpFilter::Read, Ask).or_path("/etc/passwd");
    assert!(!approves(r));
}

#[test]
fn wildcard_host_and_path_rules_are_not_approved_by_id() {
    assert!(!approves(PolicyRule::host("*.example.com", Ask)));
    assert!(!approves(PolicyRule::path("/etc/**", OpFilter::Read, Ask)));
}

#[test]
fn unknown_ids_are_not_approved_by_id() {
    let p = CompiledPolicy::compile(&ctx(), &[], &[PolicyRule::host("a.com", Ask)], &[]).unwrap();
    assert!(!p.approves_by_id("nope"));
    assert!(!p.approves_by_id(""));
    assert!(p.approves_by_id("user:1"));
}

#[test]
fn builtin_layer_literal_command_rule_is_not_approved_by_id() {
    let p = CompiledPolicy::compile(
        &ctx(),
        &[PolicyRule::command("git push", Ask).with_id("builtin:git-push")],
        &[],
        &[],
    )
    .unwrap();
    assert!(!p.approves_by_id("builtin:git-push"));
}

#[test]
fn builtin_credential_rules_are_not_approved_by_id() {
    // multi-pattern and op = any: approving a read must not approve every credential file
    let p = CompiledPolicy::compile(
        &ctx(),
        &hone_config::policy::builtin_rules(),
        &[],
        &[],
    )
    .unwrap();
    for id in [
        "builtin:git-credentials",
        "builtin:cloud-credentials",
        "builtin:ssh",
        "builtin:env-file",
        "builtin:new-host",
        "builtin:git-push",
    ] {
        assert!(!p.approves_by_id(id), "{id}");
    }
}
