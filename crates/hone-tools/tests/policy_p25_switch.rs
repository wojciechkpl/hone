#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 opt-in switch (`docs/plans/policy-p2.5-spec.md` section 2).
//!
//! API assumptions:
//! - `policy_gate::POLICY_BUILTINS_ENV: &str == "HONE_POLICY_BUILTINS"`
//! - `policy_gate::builtins_enabled(Option<&str>) -> bool` (never reads the environment)
//! - `resolve_permission_policy(&Path, Option<PathBuf>, bool)`; `PolicyConfig.builtins: bool`.
//! - Tests pass `home` explicitly and never read the real environment.

use std::path::{Path, PathBuf};

use hone_config::Decision;
use hone_config::policy::Subject;
use hone_tools::policy_gate::{
    POLICY_BUILTINS_ENV, PolicyConfig, builtins_enabled, check_provider_gating,
    resolve_permission_policy, validate_approvals,
};

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap()
}

const USER_ALLOW_HOST: &str =
    "version = 2\n[[rules]]\nid = \"ok-example\"\nhost = \"example.com\"\ndecision = \"allow\"\n";
const PROJECT_DENY: &str =
    "version = 2\n[[rules]]\nid = \"proj-no-curl\"\ncommand = \"curl*\"\ndecision = \"deny\"\n";

fn resolve(wd: &Path, home: &Path, builtins: bool) -> Option<PolicyConfig> {
    resolve_permission_policy(wd, Some(home.to_path_buf()), builtins).unwrap()
}

fn host_verdict(p: &PolicyConfig, h: &str) -> (Decision, Option<String>) {
    let v = p.compiled.evaluate(&Subject::Host(h.to_owned()));
    (v.decision, v.rule_id)
}

// -- the env var and the parser ------------------------------------------------------

#[test]
fn env_var_name_is_hone_policy_builtins() {
    assert_eq!(POLICY_BUILTINS_ENV, "HONE_POLICY_BUILTINS");
}

#[test]
fn builtins_enabled_is_true_only_for_exactly_1() {
    assert!(builtins_enabled(Some("1")));
    for v in [
        None,
        Some(""),
        Some("0"),
        Some("true"),
        Some("TRUE"),
        Some("yes"),
        Some("on"),
        Some(" 1"),
        Some("1 "),
        Some("11"),
        Some("01"),
        Some("1\n"),
    ] {
        assert!(!builtins_enabled(v), "{v:?}");
    }
}

// -- resolve_permission_policy with the third parameter ---------------------------------

#[test]
fn resolve_without_builtins_and_without_files_is_none() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    assert!(resolve(wd.path(), home.path(), false).is_none());
}

#[test]
fn resolve_with_builtins_and_without_files_is_some_with_the_builtin_layer() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let p = resolve(wd.path(), home.path(), true).expect("built-ins make a policy");
    assert!(p.builtins);
    assert!(p.compiled.has_rule_id("builtin:git-push"));
    assert!(!p.compiled.is_empty());
    assert!(p.pre_approved.is_empty());
}

#[test]
fn resolve_with_builtins_and_no_home_is_still_some() {
    let wd = tempfile::tempdir().unwrap();
    let p = resolve_permission_policy(wd.path(), None, true)
        .unwrap()
        .expect("some");
    assert!(p.builtins);
    assert!(p.compiled.has_rule_id("builtin:git-push"));
    assert!(p.context.home.is_none());
}

#[test]
fn resolve_without_builtins_but_with_files_has_builtins_false_and_no_builtin_rules() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(&wd.path().join(".hone/permissions.toml"), PROJECT_DENY);
    let p = resolve(wd.path(), home.path(), false).unwrap();
    assert!(!p.builtins);
    assert!(!p.compiled.has_rule_id("builtin:git-push"));
}

#[test]
fn resolve_with_builtins_and_files_merges_all_three_layers() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".config/hone/permissions.toml"),
        USER_ALLOW_HOST,
    );
    write(&wd.path().join(".hone/permissions.toml"), PROJECT_DENY);
    let p = resolve(wd.path(), home.path(), true).unwrap();
    assert!(p.builtins);
    // user layer replaces the built-in host ask for that host only
    assert_eq!(
        host_verdict(&p, "example.com"),
        (Decision::Allow, Some("ok-example".to_owned()))
    );
    assert_eq!(
        host_verdict(&p, "other.org"),
        (Decision::Ask, Some("builtin:new-host".to_owned()))
    );
    // project rule applies
    let v = p
        .compiled
        .evaluate(&Subject::Command("curl https://x".to_owned()));
    assert_eq!(v.decision, Decision::Deny);
    assert_eq!(v.rule_id.as_deref(), Some("proj-no-curl"));
    // built-in command rule applies
    let v = p
        .compiled
        .evaluate(&Subject::Command("git push".to_owned()));
    assert_eq!(v.rule_id.as_deref(), Some("builtin:git-push"));
}

#[test]
fn resolve_with_builtins_canonicalises_the_workdir() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let p = resolve(wd.path(), home.path(), true).unwrap();
    assert_eq!(p.context.workdir, canon(wd.path()));
    // {workdir}/** is allowed to be written, which proves the workdir was substituted
    let v = p.compiled.evaluate(&Subject::Path {
        path: canon(wd.path()).join("src/x"),
        op: hone_config::policy::PathOp::Write,
    });
    assert_eq!(v.rule_id.as_deref(), Some("builtin:workdir-write"));
}

#[test]
fn resolve_with_builtins_and_unresolvable_workdir_is_an_error() {
    let base = tempfile::tempdir().unwrap();
    let gone = base.path().join("does/not/exist");
    assert!(resolve_permission_policy(&gone, None, true).is_err());
    // without built-ins and without files, nothing changes (Ok(None))
    assert!(matches!(
        resolve_permission_policy(&gone, None, false),
        Ok(None)
    ));
}

#[test]
fn resolve_with_builtins_still_rejects_a_broken_file() {
    let wd = tempfile::tempdir().unwrap();
    write(
        &wd.path().join(".hone/permissions.toml"),
        "this is [[ not toml",
    );
    assert!(resolve_permission_policy(wd.path(), None, true).is_err());
}

// -- consequences ----------------------------------------------------------------------------

#[test]
fn builtins_policy_is_non_empty_so_ungated_providers_are_refused() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let p = resolve(wd.path(), home.path(), true).unwrap();
    let refusal = check_provider_gating(Some(&p), true, None).unwrap_err();
    assert!(refusal.starts_with("this provider "), "{refusal}");
    assert!(
        check_provider_gating(Some(&p), true, Some("1"))
            .unwrap()
            .is_some()
    );
    assert_eq!(check_provider_gating(Some(&p), false, None), Ok(None));
}

#[test]
fn approving_a_builtin_rule_id_is_valid_with_no_files() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let p = resolve(wd.path(), home.path(), true).unwrap();
    let ids = vec!["builtin:git-push".to_owned()];
    assert_eq!(validate_approvals(Some(&p), &ids), Ok(ids.clone()));
    let bad = vec!["builtin:no-such-rule".to_owned()];
    assert!(validate_approvals(Some(&p), &bad).is_err());
}
