#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 built-in rule ids (`docs/plans/policy-p2.5-spec.md` section 4).
//!
//! Uses only pre-existing API, so it compiles today and fails by assertion until the
//! Builtin layer accepts `builtin:[a-z0-9-]{1,48}` ids and nothing else.

use std::path::PathBuf;

use hone_config::Decision::Allow;
use hone_config::PermissionError;
use hone_config::policy::{CompiledPolicy, Layer, PolicyContext, PolicyRule};

fn ctx() -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: Some(PathBuf::from("/home/tester")),
        case_insensitive_paths: false,
    }
}

fn rule(id: &str) -> PolicyRule {
    PolicyRule::command("a", Allow).with_id(id)
}

fn compile(
    b: &[PolicyRule],
    u: &[PolicyRule],
    p: &[PolicyRule],
) -> Result<CompiledPolicy, PermissionError> {
    CompiledPolicy::compile(&ctx(), b, u, p)
}

#[test]
fn builtin_layer_accepts_a_builtin_prefixed_id() {
    let p = compile(&[rule("builtin:x")], &[], &[]).expect("accepted");
    assert!(p.has_rule_id("builtin:x"));
}

#[test]
fn builtin_layer_accepts_digits_and_hyphens_up_to_48_chars() {
    let ok48 = format!("builtin:{}", "a".repeat(48));
    assert!(compile(&[rule("builtin:a-b-9")], &[], &[]).is_ok());
    assert!(compile(&[rule(&ok48)], &[], &[]).is_ok());
}

#[test]
fn builtin_layer_rejects_ids_outside_the_builtin_charset() {
    let too_long = format!("builtin:{}", "a".repeat(49));
    for bad in [
        "builtin:",
        "builtin:X",
        "builtin:a_b",
        "builtin:a.b",
        "builtin:a:b",
        "plain-id",
        "BUILTIN:x",
        too_long.as_str(),
    ] {
        match compile(&[rule(bad)], &[], &[]) {
            Err(PermissionError::InvalidRule { layer, .. }) => {
                assert_eq!(layer, Layer::Builtin, "{bad}");
            }
            other => panic!("{bad:?}: expected InvalidRule in the builtin layer, got {other:?}"),
        }
    }
}

#[test]
fn user_and_project_layers_reject_builtin_prefixed_ids() {
    match compile(&[], &[rule("builtin:x")], &[]) {
        Err(PermissionError::InvalidRule { layer, .. }) => assert_eq!(layer, Layer::User),
        other => panic!("expected InvalidRule(User), got {other:?}"),
    }
    match compile(&[], &[], &[rule("builtin:x")]) {
        Err(PermissionError::InvalidRule { layer, .. }) => assert_eq!(layer, Layer::Project),
        other => panic!("expected InvalidRule(Project), got {other:?}"),
    }
}

#[test]
fn user_and_project_layers_keep_the_old_charset() {
    assert!(compile(&[], &[rule("allow-cargo")], &[]).is_ok());
    assert!(compile(&[], &[rule("rule_2.x")], &[]).is_ok());
    assert!(compile(&[], &[], &[rule("Proj.Rule_1")]).is_ok());
}

#[test]
fn builtin_layer_rejects_each_reserved_id() {
    for reserved in [
        "builtin:unparsed-command",
        "builtin:invalid-host",
        "builtin:pipe-to-shell",
        "builtin:computer-control",
        "builtin:screen-capture",
    ] {
        let r = compile(&[rule(reserved)], &[], &[]);
        assert!(
            matches!(
                r,
                Err(PermissionError::InvalidRule {
                    layer: Layer::Builtin,
                    ..
                })
            ),
            "{reserved}: {r:?}"
        );
    }
}
