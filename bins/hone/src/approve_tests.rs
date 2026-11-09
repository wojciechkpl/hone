//! Tests for `crate::approve` (P2.3 spec sections 3 and 5).
//!
//! Assumed API, to be created in `bins/hone/src/approve.rs` (which includes
//! this file with `#[cfg(test)] #[path = "approve_tests.rs"] mod tests;`):
//!
//! ```ignore
//! pub(crate) fn validate_approvals(
//!     policy: Option<&hone_tools::policy_gate::PolicyConfig>,
//!     ids: &[String],
//! ) -> Result<Vec<String>, String>;
//!
//! pub(crate) fn check_provider_gating(
//!     policy: Option<&hone_tools::policy_gate::PolicyConfig>,
//!     runs_own_tools: bool,
//!     allow_env: Option<&str>,
//! ) -> Result<Option<String>, String>;
//! ```
//!
//! `Err` strings are the message WITHOUT the `hone: ` prefix and without a
//! trailing newline; tests only pin the load-bearing substrings.

use std::path::PathBuf;
use std::sync::Arc;

use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, OpFilter, PolicyContext, PolicyRule};
use hone_tools::policy_gate::PolicyConfig;

use crate::approve::{check_provider_gating, validate_approvals};

fn policy_with(rules: Vec<PolicyRule>) -> PolicyConfig {
    let workdir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let context = PolicyContext::for_platform(workdir, None::<PathBuf>);
    let compiled = CompiledPolicy::compile(&context, &[], &rules, &[]).unwrap();
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: vec![],
    }
}

/// Rule 1 has an explicit id; rule 2 gets the automatic id `user:2`.
fn sample_policy() -> PolicyConfig {
    policy_with(vec![
        PolicyRule::command("pip install*", Decision::Allow).with_id("allow-pypi"),
        PolicyRule::path("secrets/**", OpFilter::Read, Decision::Ask),
    ])
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

// -- validate_approvals --------------------------------------------------------

#[test]
fn test_validate_approvals_when_explicit_rule_id_should_accept() {
    let p = sample_policy();
    let out = validate_approvals(Some(&p), &ids(&["allow-pypi"])).unwrap();
    assert_eq!(out, ids(&["allow-pypi"]));
}

#[test]
fn test_validate_approvals_when_automatic_rule_id_should_accept() {
    let p = sample_policy();
    let out = validate_approvals(Some(&p), &ids(&["user:2"])).unwrap();
    assert_eq!(out, ids(&["user:2"]));
}

#[test]
fn test_validate_approvals_when_synthetic_ids_should_accept() {
    let p = sample_policy();
    let both = ids(&["builtin:unparsed-command", "builtin:invalid-host"]);
    assert_eq!(validate_approvals(Some(&p), &both).unwrap(), both);
}

#[test]
fn test_validate_approvals_when_several_ids_should_keep_all_in_order() {
    let p = sample_policy();
    let want = ids(&["user:2", "allow-pypi", "builtin:unparsed-command"]);
    assert_eq!(validate_approvals(Some(&p), &want).unwrap(), want);
}

#[test]
fn test_validate_approvals_when_unknown_id_should_err_naming_the_id() {
    let p = sample_policy();
    let e = validate_approvals(Some(&p), &ids(&["allow-pipy"])).unwrap_err();
    assert!(e.contains("unknown rule id"), "{e}");
    assert!(e.contains("allow-pipy"), "{e}");
}

#[test]
fn test_validate_approvals_when_unknown_id_among_known_should_err() {
    let p = sample_policy();
    let e = validate_approvals(Some(&p), &ids(&["allow-pypi", "nope"])).unwrap_err();
    assert!(e.contains("nope"), "{e}");
}

#[test]
fn test_validate_approvals_when_unknown_id_has_control_chars_should_escape_them() {
    let p = sample_policy();
    let e = validate_approvals(Some(&p), &ids(&["x\ny\u{1b}[31m"])).unwrap_err();
    assert!(!e.contains('\n'), "raw newline leaked: {e:?}");
    assert!(!e.contains('\u{1b}'), "raw ESC leaked: {e:?}");
    assert!(e.contains("\\n"), "newline should appear escaped: {e}");
}

#[test]
fn test_validate_approvals_when_ids_but_no_policy_should_err() {
    let e = validate_approvals(None, &ids(&["allow-pypi"])).unwrap_err();
    assert!(e.contains("no permission policy is loaded"), "{e}");
}

#[test]
fn test_validate_approvals_when_synthetic_id_but_no_policy_should_err() {
    let e = validate_approvals(None, &ids(&["builtin:unparsed-command"])).unwrap_err();
    assert!(e.contains("no permission policy is loaded"), "{e}");
}

#[test]
fn test_validate_approvals_when_empty_list_and_no_policy_should_be_ok_and_empty() {
    assert_eq!(validate_approvals(None, &[]).unwrap(), Vec::<String>::new());
}

#[test]
fn test_validate_approvals_when_empty_list_with_policy_should_be_ok_and_empty() {
    let p = sample_policy();
    assert_eq!(
        validate_approvals(Some(&p), &[]).unwrap(),
        Vec::<String>::new()
    );
}

// -- check_provider_gating -----------------------------------------------------

#[test]
fn test_gating_when_nonempty_policy_and_provider_runs_own_tools_should_refuse() {
    let p = sample_policy();
    let e = check_provider_gating(Some(&p), true, None).unwrap_err();
    assert!(e.contains("runs its own tools"), "{e}");
    assert!(e.contains("permission policy can't check"), "{e}");
    assert!(e.contains("HONE_ALLOW_UNGATED_PROVIDER=1"), "{e}");
}

#[test]
fn test_gating_when_override_is_1_should_allow_with_warning() {
    let p = sample_policy();
    let w = check_provider_gating(Some(&p), true, Some("1"))
        .unwrap()
        .expect("a warning line is required");
    assert!(!w.trim().is_empty());
    assert!(!w.trim_end().contains('\n'), "one warning line: {w:?}");
}

#[test]
fn test_gating_when_override_is_not_exactly_1_should_still_refuse() {
    let p = sample_policy();
    for v in ["0", "", "true", "yes", " 1x"] {
        assert!(
            check_provider_gating(Some(&p), true, Some(v)).is_err(),
            "override value {v:?} must not disable the refusal"
        );
    }
}

#[test]
fn test_gating_when_empty_policy_should_not_refuse_or_warn() {
    let p = policy_with(vec![]);
    assert_eq!(check_provider_gating(Some(&p), true, None).unwrap(), None);
    assert_eq!(
        check_provider_gating(Some(&p), true, Some("1")).unwrap(),
        None,
        "no policy to bypass => no warning either"
    );
}

#[test]
fn test_gating_when_no_policy_should_not_refuse_or_warn() {
    assert_eq!(check_provider_gating(None, true, None).unwrap(), None);
    assert_eq!(check_provider_gating(None, true, Some("1")).unwrap(), None);
}

#[test]
fn test_gating_when_provider_does_not_run_own_tools_should_not_refuse() {
    let p = sample_policy();
    assert_eq!(check_provider_gating(Some(&p), false, None).unwrap(), None);
    assert_eq!(
        check_provider_gating(Some(&p), false, Some("1")).unwrap(),
        None
    );
}
