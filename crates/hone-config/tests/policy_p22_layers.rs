//! P2.2 review round 2: `has_path_rules` and `CompiledPolicy::load_layers`
//! (`docs/plans/policy-p2.2-spec.md` sections 6 and 7).

use std::path::{Path, PathBuf};

use hone_config::Decision::{Allow, Ask, Deny};
use hone_config::PermissionError;
use hone_config::policy::{CompiledPolicy, OpFilter, PolicyContext, PolicyRule, Subject};

fn ctx() -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: Some(PathBuf::from("/home/tester")),
        case_insensitive_paths: false,
    }
}

fn cmd(s: &str) -> Subject {
    Subject::Command(s.to_string())
}

fn write_toml(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

fn has_path(builtin: &[PolicyRule], user: &[PolicyRule], project: &[PolicyRule]) -> bool {
    CompiledPolicy::compile(&ctx(), builtin, user, project)
        .unwrap()
        .has_path_rules()
}

// -- has_path_rules -------------------------------------------------------------

#[test]
fn has_path_rules_false_for_empty_and_non_path_policies() {
    assert!(!has_path(&[], &[], &[]));
    let others = [
        PolicyRule::command("ls", Deny),
        PolicyRule::host("a.com", Deny),
    ];
    assert!(!has_path(&others, &others, &others));
}

#[test]
fn has_path_rules_true_for_a_path_rule_in_any_layer() {
    let p = [PolicyRule::path("~/.ssh/**", OpFilter::Any, Deny)];
    let other = [PolicyRule::command("ls", Allow)];
    assert!(has_path(&p, &[], &[]), "builtin");
    assert!(has_path(&[], &p, &[]), "user");
    assert!(has_path(&[], &[], &p), "project");
    assert!(has_path(&other, &other, &p), "mixed");
}

#[test]
fn has_path_rules_is_independent_of_has_command_rules() {
    let p = [PolicyRule::path("/x", OpFilter::Read, Ask)];
    let c = CompiledPolicy::compile(&ctx(), &[], &p, &[]).unwrap();
    assert!(c.has_path_rules());
    assert!(!c.has_command_rules());
    let c = CompiledPolicy::compile(&ctx(), &[], &[PolicyRule::command("ls", Ask)], &[]).unwrap();
    assert!(c.has_command_rules());
    assert!(!c.has_path_rules());
}

// -- load_layers ----------------------------------------------------------------

#[test]
fn load_layers_with_none_user_skips_the_user_layer() {
    let dir = tempfile::tempdir().unwrap();
    let project = write_toml(
        dir.path(),
        "project.toml",
        "[[rules]]\ncommand = \"rm *\"\ndecision = \"deny\"\n",
    );
    let p = CompiledPolicy::load_layers(&ctx(), &[], None, &project).unwrap();
    let v = p.evaluate(&cmd("rm x"));
    assert_eq!(v.decision, Deny);
    assert_eq!(v.rule_id.as_deref(), Some("project:1"));
    assert_eq!(p.evaluate(&cmd("ls")).decision, Allow);
}

#[test]
fn load_layers_with_some_user_loads_both_layers() {
    let dir = tempfile::tempdir().unwrap();
    let user = write_toml(
        dir.path(),
        "user.toml",
        "[[rules]]\ncommand = \"rm *\"\ndecision = \"allow\"\n",
    );
    let project = write_toml(
        dir.path(),
        "project.toml",
        "[[rules]]\ncommand = \"rm *\"\ndecision = \"ask\"\n",
    );
    let builtin = [PolicyRule::command("rm *", Deny)];
    let p = CompiledPolicy::load_layers(&ctx(), &builtin, Some(&user), &project).unwrap();
    let v = p.evaluate(&cmd("rm x"));
    assert_eq!(v.decision, Ask);
    assert_eq!(v.rule_id.as_deref(), Some("project:1"));
}

#[test]
fn load_layers_missing_files_give_builtin_only_policy() {
    let dir = tempfile::tempdir().unwrap();
    let builtin = [PolicyRule::command("rm *", Deny)];
    let p = CompiledPolicy::load_layers(
        &ctx(),
        &builtin,
        Some(&dir.path().join("none-user.toml")),
        &dir.path().join("none-project.toml"),
    )
    .unwrap();
    assert_eq!(
        p.evaluate(&cmd("rm x")).rule_id.as_deref(),
        Some("builtin:1")
    );
}

#[test]
fn load_layers_propagates_errors() {
    let dir = tempfile::tempdir().unwrap();
    let bad = write_toml(dir.path(), "bad.toml", "[[rules\n");
    let r = CompiledPolicy::load_layers(&ctx(), &[], None, &bad);
    assert!(matches!(r, Err(PermissionError::Toml(_))));
    let r = CompiledPolicy::load_layers(&ctx(), &[], Some(&bad), &dir.path().join("none.toml"));
    assert!(matches!(r, Err(PermissionError::Toml(_))));
}

#[cfg(unix)]
#[test]
fn load_layers_refuses_symlinked_project_file_but_accepts_symlinked_user_file() {
    let dir = tempfile::tempdir().unwrap();
    let real = write_toml(
        dir.path(),
        "real.toml",
        "[[rules]]\ncommand = \"ls\"\ndecision = \"deny\"\n",
    );
    let link = dir.path().join("link.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let none = dir.path().join("none.toml");
    let r = CompiledPolicy::load_layers(&ctx(), &[], None, &link);
    assert!(matches!(r, Err(PermissionError::UnsafeFile { .. })));
    let p = CompiledPolicy::load_layers(&ctx(), &[], Some(&link), &none).unwrap();
    assert_eq!(p.evaluate(&cmd("ls")).decision, Deny);
}

#[test]
fn load_hierarchical_with_paths_behaves_like_load_layers_with_some_user() {
    let dir = tempfile::tempdir().unwrap();
    let user = write_toml(
        dir.path(),
        "user.toml",
        "[[rules]]\ncommand = \"rm *\"\ndecision = \"allow\"\n",
    );
    let project = write_toml(
        dir.path(),
        "project.toml",
        "[[rules]]\ncommand = \"git push*\"\ndecision = \"deny\"\n",
    );
    let builtin = [
        PolicyRule::command("rm *", Deny),
        PolicyRule::command("git push*", Ask),
    ];
    let a =
        CompiledPolicy::load_hierarchical_with_paths(&ctx(), &builtin, &user, &project).unwrap();
    let b = CompiledPolicy::load_layers(&ctx(), &builtin, Some(&user), &project).unwrap();
    for s in ["rm x", "git push", "ls"] {
        let (va, vb) = (a.evaluate(&cmd(s)), b.evaluate(&cmd(s)));
        assert_eq!(
            (va.decision, va.rule_id, va.layer),
            (vb.decision, vb.rule_id, vb.layer),
            "{s}"
        );
    }
}

#[test]
fn load_hierarchical_with_paths_still_rejects_a_broken_project_file() {
    let dir = tempfile::tempdir().unwrap();
    let project = write_toml(dir.path(), "p.toml", "[[rules\n");
    let r = CompiledPolicy::load_hierarchical_with_paths(
        &ctx(),
        &[],
        &dir.path().join("none.toml"),
        &project,
    );
    assert!(matches!(r, Err(PermissionError::Toml(_))));
}
