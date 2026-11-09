//! Tests for `crate::policy_load::resolve_permission_policy` (spec section 7).
//!
//! This file is pulled into `policy_load.rs` with
//! `#[cfg(test)] #[path = "policy_load_tests.rs"] mod tests;`.

use std::path::{Path, PathBuf};

use hone_config::Decision;
use hone_config::policy::Subject;

use crate::policy_load::resolve_permission_policy;

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn user_file(home: &Path) -> PathBuf {
    home.join(".config/hone/permissions.toml")
}

fn project_file(wd: &Path) -> PathBuf {
    wd.join(".hone/permissions.toml")
}

const PROJECT_DENY: &str = r#"
version = 2
[[rules]]
id = "proj-no-push"
command = "git push*"
decision = "deny"
"#;

const USER_ASK: &str = r#"
version = 2
[[rules]]
id = "user-ask-deploy"
command = "deploy*"
decision = "ask"
"#;

fn verdict(p: &hone_core::PolicyConfig, command: &str) -> (Decision, Option<String>) {
    let v = p.compiled.evaluate(&Subject::Command(command.to_owned()));
    (v.decision, v.rule_id)
}

#[test]
fn test_resolve_when_no_files_should_return_none() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let r = resolve_permission_policy(wd.path(), Some(home.path().to_path_buf()));
    assert!(matches!(r, Ok(None)), "{r:?}");
}

#[test]
fn test_resolve_when_no_files_and_no_home_should_return_none() {
    let wd = tempfile::tempdir().unwrap();
    let r = resolve_permission_policy(wd.path(), None);
    assert!(matches!(r, Ok(None)), "{r:?}");
}

#[test]
fn test_resolve_when_home_none_and_project_file_should_use_project_rules_only() {
    let wd = tempfile::tempdir().unwrap();
    write(&project_file(wd.path()), PROJECT_DENY);

    let p = resolve_permission_policy(wd.path(), None)
        .expect("loads")
        .expect("project file present => Some");

    assert_eq!(
        verdict(&p, "git push origin"),
        (Decision::Deny, Some("proj-no-push".to_owned()))
    );
    assert_eq!(verdict(&p, "deploy prod").0, Decision::Allow);
    assert!(p.context.home.is_none());
    assert!(p.pre_approved.is_empty());
}

#[test]
fn test_resolve_when_home_is_relative_should_treat_it_as_none() {
    let wd = tempfile::tempdir().unwrap();
    let rel = Some(PathBuf::from("relative/home"));

    // No project file: nothing to load, and a relative home must not be an error.
    let r = resolve_permission_policy(wd.path(), rel.clone());
    assert!(matches!(r, Ok(None)), "{r:?}");

    // With a project file: loads, and the context carries no home.
    write(&project_file(wd.path()), PROJECT_DENY);
    let p = resolve_permission_policy(wd.path(), rel)
        .expect("relative home must not fail the load")
        .expect("Some");
    assert!(p.context.home.is_none());
}

#[test]
fn test_resolve_when_only_user_file_should_return_some_with_user_rules() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(&user_file(home.path()), USER_ASK);

    let p = resolve_permission_policy(wd.path(), Some(home.path().to_path_buf()))
        .expect("loads")
        .expect("user file present => Some");

    assert_eq!(
        verdict(&p, "deploy prod"),
        (Decision::Ask, Some("user-ask-deploy".to_owned()))
    );
    assert!(p.context.home.is_some());
}

#[test]
fn test_resolve_when_both_files_should_merge_layers() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(&user_file(home.path()), USER_ASK);
    write(&project_file(wd.path()), PROJECT_DENY);

    let p = resolve_permission_policy(wd.path(), Some(home.path().to_path_buf()))
        .unwrap()
        .unwrap();

    assert_eq!(verdict(&p, "deploy prod").0, Decision::Ask);
    assert_eq!(verdict(&p, "git push origin").0, Decision::Deny);
}

#[test]
fn test_resolve_when_project_file_is_broken_should_return_err() {
    let wd = tempfile::tempdir().unwrap();
    write(&project_file(wd.path()), "this is [[ not toml");
    let r = resolve_permission_policy(wd.path(), None);
    assert!(r.is_err(), "{r:?}");
}

#[test]
fn test_resolve_when_user_file_is_broken_should_return_err() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(&user_file(home.path()), "decision = = =");
    let r = resolve_permission_policy(wd.path(), Some(home.path().to_path_buf()));
    assert!(r.is_err(), "{r:?}");
}

#[test]
fn test_resolve_when_workdir_missing_and_no_files_should_return_none() {
    let base = tempfile::tempdir().unwrap();
    let gone = base.path().join("does/not/exist");
    assert!(gone.is_absolute());
    let r = resolve_permission_policy(&gone, None);
    assert!(matches!(r, Ok(None)), "{r:?}");
}

#[test]
fn test_resolve_when_workdir_missing_but_user_file_exists_should_return_err() {
    let base = tempfile::tempdir().unwrap();
    let gone = base.path().join("does/not/exist");
    let home = tempfile::tempdir().unwrap();
    write(&user_file(home.path()), USER_ASK);
    let r = resolve_permission_policy(&gone, Some(home.path().to_path_buf()));
    assert!(r.is_err(), "{r:?}");
}

#[test]
fn test_resolve_should_canonicalise_workdir_in_context() {
    let wd = tempfile::tempdir().unwrap();
    write(&project_file(wd.path()), PROJECT_DENY);
    let p = resolve_permission_policy(wd.path(), None).unwrap().unwrap();
    assert_eq!(p.context.workdir, std::fs::canonicalize(wd.path()).unwrap());
}
