#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 built-in rule set (`docs/plans/policy-p2.5-spec.md` section 3, revised by 14).
//!
//! API assumption: `hone_config::policy::builtin_rules() -> Vec<PolicyRule>`; it is
//! compiled as the Builtin layer with `CompiledPolicy::compile(ctx, &builtin_rules(), user, project)`.
//! Subjects are evaluated with absolute paths (`/work/proj` workdir, `/home/tester` home).

use std::path::PathBuf;

use hone_config::Decision::{self, Allow, Ask, Deny};
use hone_config::policy::{
    CompiledPolicy, Layer, OpFilter, PathOp, PolicyContext, PolicyRule, Subject, builtin_rules,
};

fn ctx(case_insensitive: bool) -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: Some(PathBuf::from("/home/tester")),
        case_insensitive_paths: case_insensitive,
    }
}

fn policy_with(user: &[PolicyRule], project: &[PolicyRule]) -> CompiledPolicy {
    CompiledPolicy::compile(&ctx(false), &builtin_rules(), user, project)
        .expect("builtin rules compile as the Builtin layer")
}

fn policy() -> CompiledPolicy {
    policy_with(&[], &[])
}

fn rd(p: &str) -> Subject {
    Subject::Path {
        path: PathBuf::from(p),
        op: PathOp::Read,
    }
}
fn wr(p: &str) -> Subject {
    Subject::Path {
        path: PathBuf::from(p),
        op: PathOp::Write,
    }
}
fn cmd(c: &str) -> Subject {
    Subject::Command(c.to_owned())
}
fn host(h: &str) -> Subject {
    Subject::Host(h.to_owned())
}

/// `(decision, rule id)` of the verdict.
fn v(p: &CompiledPolicy, s: &Subject) -> (Decision, Option<String>) {
    let x = p.evaluate(s);
    (x.decision, x.rule_id)
}

fn is(p: &CompiledPolicy, s: &Subject, d: Decision, id: &str) {
    assert_eq!(v(p, s), (d, Some(id.to_owned())), "{s:?}");
}

/// Allowed (by default or by an explicit allow rule).
fn allowed(p: &CompiledPolicy, s: &Subject) {
    assert_eq!(p.evaluate(s).decision, Allow, "{s:?}");
}

/// The decision only (for rows where several rules could legitimately report the ask).
fn asks(p: &CompiledPolicy, s: &Subject) {
    assert_eq!(p.evaluate(s).decision, Ask, "{s:?}");
}

const ALL_IDS: [&str; 39] = [
    "builtin:env-template",
    "builtin:env-file",
    "builtin:ssh",
    "builtin:cloud-credentials",
    "builtin:git-credentials",
    "builtin:package-credentials",
    "builtin:gnupg",
    "builtin:shell-history",
    "builtin:hone-config",
    "builtin:shell-startup",
    "builtin:git-hooks",
    "builtin:git-config",
    "builtin:autostart",
    "builtin:user-bin",
    "builtin:toolchain-config",
    "builtin:workdir-write",
    "builtin:temp-write",
    "builtin:outside-workdir",
    "builtin:git-push",
    "builtin:chmod-777",
    "builtin:git-config-get",
    "builtin:git-config-list",
    "builtin:git-config-list-long",
    "builtin:git-config-cmd",
    "builtin:git-inline-config",
    "builtin:git-remote-set-url",
    "builtin:git-remote-add",
    "builtin:sudo",
    "builtin:sudoedit",
    "builtin:su",
    "builtin:doas",
    "builtin:pkexec",
    "builtin:run0",
    "builtin:crontab-list",
    "builtin:crontab",
    "builtin:at",
    "builtin:systemd-run",
    "builtin:launchctl",
    "builtin:new-host",
];

// -- the set itself ----------------------------------------------------------------

#[test]
fn builtin_rules_has_exactly_the_39_specified_rules() {
    assert_eq!(builtin_rules().len(), 39);
    let p = policy();
    for id in ALL_IDS {
        assert!(p.has_rule_id(id), "{id}");
    }
    assert!(!p.is_empty());
}

#[test]
fn builtin_rule_ids_are_all_distinct() {
    let mut seen = std::collections::HashSet::new();
    for id in ALL_IDS {
        assert!(seen.insert(id), "duplicate id in the expected list: {id}");
    }
    // compiling rejects duplicate explicit ids across the layer
    assert!(CompiledPolicy::compile(&ctx(false), &builtin_rules(), &[], &[]).is_ok());
    assert_eq!(builtin_rules().len(), ALL_IDS.len());
}

#[test]
fn builtin_rules_do_not_use_reserved_or_automatic_ids() {
    let p = policy();
    for id in [
        "builtin:unparsed-command",
        "builtin:invalid-host",
        "builtin:pipe-to-shell",
        "builtin:computer-control",
        "builtin:screen-capture",
        "builtin:1",
    ] {
        assert!(!p.has_rule_id(id), "{id} must not be a built-in rule id");
    }
}

#[test]
fn builtin_rules_compile_without_home_and_still_work() {
    let c = PolicyContext {
        workdir: PathBuf::from("/work/proj"),
        home: None,
        case_insensitive_paths: false,
    };
    let p = CompiledPolicy::compile(&c, &builtin_rules(), &[], &[]).expect("compiles");
    // Rules that need `~` can never match, but the rest still work.
    is(&p, &cmd("git push"), Ask, "builtin:git-push");
}

#[test]
fn builtin_rule_verdicts_report_the_builtin_layer() {
    let p = policy();
    assert_eq!(p.evaluate(&cmd("git push")).layer, Some(Layer::Builtin));
}

// -- .env ----------------------------------------------------------------------------

#[test]
fn env_file_asks_for_read_and_write_anywhere() {
    let p = policy();
    for path in ["/work/proj/.env", "/work/proj/sub/dir/.env", "/srv/app/.env"] {
        is(&p, &rd(path), Ask, "builtin:env-file");
        is(&p, &wr(path), Ask, "builtin:env-file");
    }
}

#[test]
fn env_dot_suffix_files_ask_except_the_workdir_templates() {
    let p = policy();
    is(&p, &rd("/work/proj/.env.local"), Ask, "builtin:env-file");
    is(
        &p,
        &rd("/work/proj/.env.production"),
        Ask,
        "builtin:env-file",
    );
    for t in [".env.example", ".env.sample", ".env.template"] {
        let path = format!("/work/proj/{t}");
        is(&p, &rd(&path), Allow, "builtin:env-template");
        is(&p, &wr(&path), Allow, "builtin:env-template");
        let nested = format!("/work/proj/sub/dir/{t}");
        is(&p, &rd(&nested), Allow, "builtin:env-template");
    }
}

#[test]
fn env_template_is_anchored_at_the_workdir() {
    let p = policy();
    // outside the workdir a template no longer shadows an ask elsewhere
    asks(&p, &rd("/home/tester/.ssh/.env.example"));
    asks(&p, &rd("/srv/app/.env.example"));
    asks(&p, &wr("/srv/app/.env.template"));
}

#[test]
fn more_env_file_shapes_ask() {
    let p = policy();
    for path in ["/work/proj/prod.env", "/work/proj/.envrc", "/srv/app/.envrc"] {
        is(&p, &rd(path), Ask, "builtin:env-file");
    }
}

#[test]
fn env_negatives_are_not_matched() {
    let p = policy();
    for path in [
        "/work/proj/env",
        "/work/proj/dotenv.rs",
        "/work/proj/.environment",
        "/work/proj/src/env.rs",
    ] {
        allowed(&p, &rd(path));
        assert_ne!(
            p.evaluate(&rd(path)).rule_id.as_deref(),
            Some("builtin:env-file"),
            "{path}"
        );
    }
    is(&p, &wr("/work/proj/env"), Allow, "builtin:workdir-write");
}

// -- credential directories --------------------------------------------------------------

#[test]
fn ssh_dir_asks_for_read_and_write() {
    let p = policy();
    is(&p, &rd("/home/tester/.ssh/id_ed25519"), Ask, "builtin:ssh");
    is(
        &p,
        &wr("/home/tester/.ssh/authorized_keys"),
        Ask,
        "builtin:ssh",
    );
    // nearby negatives: a different directory with the same prefix, another user's home
    allowed(&p, &rd("/home/tester/.sshx/key"));
    allowed(&p, &rd("/home/other/.ssh/id_rsa"));
}

#[test]
fn cloud_credentials_ask() {
    let p = policy();
    for path in [
        "/home/tester/.aws/credentials",
        "/home/tester/.config/gcloud/application_default_credentials.json",
        "/home/tester/.azure/accessTokens.json",
        "/home/tester/.kube/config",
        "/home/tester/.docker/config.json",
        "/home/tester/.vault-token",
        "/home/tester/.terraform.d/credentials.tfrc.json",
    ] {
        is(&p, &rd(path), Ask, "builtin:cloud-credentials");
    }
    allowed(&p, &rd("/home/tester/.docker/other.json"));
    allowed(&p, &rd("/home/tester/.awsome/x"));
}

#[test]
fn git_credentials_ask() {
    let p = policy();
    for path in [
        "/home/tester/.git-credentials",
        "/home/tester/.config/git/credentials",
        "/home/tester/.netrc",
        "/home/tester/.config/gh/hosts.yml",
    ] {
        is(&p, &rd(path), Ask, "builtin:git-credentials");
    }
    allowed(&p, &rd("/home/tester/.config/gh/config.yml"));
}

#[test]
fn package_credentials_ask_for_read_and_write() {
    let p = policy();
    for path in [
        "/home/tester/.npmrc",
        "/work/proj/.npmrc",
        "/work/proj/sub/.pypirc",
        "/home/tester/.cargo/credentials",
        "/home/tester/.cargo/credentials.toml",
        "/home/tester/.gem/credentials",
    ] {
        is(&p, &rd(path), Ask, "builtin:package-credentials");
        is(&p, &wr(path), Ask, "builtin:package-credentials");
    }
    allowed(&p, &rd("/home/tester/.cargo/config.toml"));
}

#[test]
fn gnupg_asks() {
    let p = policy();
    is(
        &p,
        &rd("/home/tester/.gnupg/private-keys-v1.d/k"),
        Ask,
        "builtin:gnupg",
    );
    allowed(&p, &rd("/home/tester/.gnupg-notes"));
}

#[test]
fn shell_history_asks() {
    let p = policy();
    for path in [
        "/home/tester/.bash_history",
        "/home/tester/.zsh_history",
        "/home/tester/.local/share/fish/fish_history",
        "/home/tester/.python_history",
        "/home/tester/.psql_history",
        "/home/tester/.mysql_history",
    ] {
        is(&p, &rd(path), Ask, "builtin:shell-history");
    }
}

// -- self-protection and persistence ------------------------------------------------------

#[test]
fn hone_config_write_asks_but_read_is_allowed() {
    let p = policy();
    for path in [
        "/home/tester/.config/hone/permissions.toml",
        "/work/proj/.hone/permissions.toml",
        "/work/proj/.hone/hooks.toml",
        "/work/proj/.hone/agents/evil.md",
        // 14: every write under any .hone asks
        "/work/proj/.hone/mcp.json",
        "/work/proj/.hone/skills/x/SKILL.md",
        "/work/proj/.hone/notes.md",
        "/work/proj/sub/.hone/mcp.json",
    ] {
        is(&p, &wr(path), Ask, "builtin:hone-config");
        allowed(&p, &rd(path));
    }
    // nearby negative: a directory merely named like it
    is(&p, &wr("/work/proj/hone/x"), Allow, "builtin:workdir-write");
}

#[test]
fn shell_startup_write_asks_but_read_is_allowed() {
    let p = policy();
    for f in [
        ".bashrc",
        ".bash_profile",
        ".bash_login",
        ".bash_logout",
        ".profile",
        ".zshrc",
        ".zprofile",
        ".zshenv",
        ".zlogin",
        ".zlogout",
        ".config/fish/config.fish",
    ] {
        let path = format!("/home/tester/{f}");
        is(&p, &wr(&path), Ask, "builtin:shell-startup");
        allowed(&p, &rd(&path));
    }
}

#[test]
fn every_write_under_any_dot_git_asks() {
    let p = policy();
    for path in [
        "/work/proj/.git/hooks/pre-commit",
        "/work/proj/.git/config",
        "/work/proj/.git/commondir",
        "/work/proj/.git/modules/x/config",
        "/work/proj/sub/.git/config",
        "/work/proj/.git/HEAD",
        "/work/proj/.git",
        "/work/proj/sub/.git",
    ] {
        is(&p, &wr(path), Ask, "builtin:git-hooks");
        allowed(&p, &rd(path));
    }
    // nearby negatives
    is(&p, &wr("/work/proj/.gitignore"), Allow, "builtin:workdir-write");
    is(&p, &wr("/work/proj/.github/ci.yml"), Allow, "builtin:workdir-write");
}

#[test]
fn global_git_config_write_asks() {
    let p = policy();
    for path in ["/home/tester/.gitconfig", "/home/tester/.config/git/config"] {
        is(&p, &wr(path), Ask, "builtin:git-config");
        allowed(&p, &rd(path));
    }
}

#[test]
fn autostart_write_asks() {
    let p = policy();
    for path in [
        "/home/tester/.config/systemd/user/x.service",
        "/home/tester/.config/autostart/x.desktop",
        "/home/tester/Library/LaunchAgents/x.plist",
    ] {
        is(&p, &wr(path), Ask, "builtin:autostart");
        allowed(&p, &rd(path));
    }
}

#[test]
fn user_bin_write_asks() {
    let p = policy();
    for path in [
        "/home/tester/.local/bin/tool",
        "/home/tester/bin/tool",
        "/home/tester/.cargo/bin/cargo-evil",
    ] {
        is(&p, &wr(path), Ask, "builtin:user-bin");
        allowed(&p, &rd(path));
    }
}

#[test]
fn cargo_config_write_asks_but_read_is_allowed() {
    let p = policy();
    for path in [
        "/home/tester/.cargo/config.toml",
        "/home/tester/.cargo/config",
    ] {
        is(&p, &wr(path), Ask, "builtin:toolchain-config");
        allowed(&p, &rd(path));
    }
}

#[test]
fn git_config_reads_are_allowed_and_changes_ask() {
    let p = policy();
    for c in [
        "git config --get user.name",
        "git config --get-all remote.origin.url",
    ] {
        is(&p, &cmd(c), Allow, "builtin:git-config-get");
    }
    is(&p, &cmd("git config -l"), Allow, "builtin:git-config-list");
    is(
        &p,
        &cmd("git config --list"),
        Allow,
        "builtin:git-config-list-long",
    );
    is(
        &p,
        &cmd("git config --list --show-origin"),
        Allow,
        "builtin:git-config-list-long",
    );
    for c in [
        "git config core.hooksPath /tmp/h",
        "git config --global core.hooksPath /tmp/h",
        "git config core.fsmonitor 'sh /tmp/h/x'",
        "git config --file .git/config core.pager x",
        "git config user.name",
    ] {
        is(&p, &cmd(c), Ask, "builtin:git-config-cmd");
    }
}

#[test]
fn git_inline_config_and_remote_changes_ask() {
    let p = policy();
    is(
        &p,
        &cmd("git -c core.fsmonitor=x status"),
        Ask,
        "builtin:git-inline-config",
    );
    is(
        &p,
        &cmd("git remote set-url origin https://e.example/r.git"),
        Ask,
        "builtin:git-remote-set-url",
    );
    is(
        &p,
        &cmd("git remote set-url --push origin https://e.example/r.git"),
        Ask,
        "builtin:git-remote-set-url",
    );
    is(
        &p,
        &cmd("git remote add o https://e.example/r.git"),
        Ask,
        "builtin:git-remote-add",
    );
    // nearby negatives
    allowed(&p, &cmd("git remote -v"));
    allowed(&p, &cmd("git remote show origin"));
    allowed(&p, &cmd("git status"));
    allowed(&p, &cmd("git configure"));
}

// -- writes in and out of the workdir ------------------------------------------------------

#[test]
fn workdir_write_is_allowed() {
    let p = policy();
    is(&p, &wr("/work/proj/src/x"), Allow, "builtin:workdir-write");
    is(&p, &wr("/work/proj/a/b/c.txt"), Allow, "builtin:workdir-write");
}

#[test]
fn temp_writes_are_allowed_in_both_macos_and_linux_forms() {
    let p = policy();
    for path in [
        "/tmp/x",
        "/private/tmp/x",
        "/var/tmp/x",
        "/private/var/folders/ab/cd/T/x",
        "/private/var/folders/ab/cd/T/sub/dir/x",
        "/var/folders/ab/cd/T/x",
        "/private/var/tmp/x",
        "/dev/null",
        "/dev/stdout",
        "/dev/stderr",
        "/dev/tty",
        "/dev/fd/3",
    ] {
        is(&p, &wr(path), Allow, "builtin:temp-write");
    }
}

#[test]
fn only_the_per_user_t_directory_under_var_folders_is_temp() {
    let p = policy();
    for path in [
        "/private/var/folders/xx/yy/C/x",
        "/private/var/folders/xx/yy/0/x",
        "/var/folders/xx/yy/C/x",
        "/private/var/folders/xx/x",
        "/private/var/folders/x",
    ] {
        is(&p, &wr(path), Ask, "builtin:outside-workdir");
    }
    is(
        &p,
        &wr("/private/var/folders/xx/yy/T/x"),
        Allow,
        "builtin:temp-write",
    );
}

#[test]
fn write_outside_workdir_asks_but_read_is_allowed() {
    let p = policy();
    is(&p, &wr("/srv/x"), Ask, "builtin:outside-workdir");
    is(
        &p,
        &wr("/home/tester/notes.txt"),
        Ask,
        "builtin:outside-workdir",
    );
    allowed(&p, &rd("/srv/x"));
    // nearby negative: a sibling directory sharing the workdir prefix is outside
    is(
        &p,
        &wr("/work/proj-other/x"),
        Ask,
        "builtin:outside-workdir",
    );
}

#[test]
fn specific_asks_win_over_the_broad_workdir_allow() {
    let p = policy();
    is(&p, &wr("/work/proj/.env"), Ask, "builtin:env-file");
    is(
        &p,
        &wr("/work/proj/.git/hooks/post-merge"),
        Ask,
        "builtin:git-hooks",
    );
}

#[test]
fn case_insensitive_context_matches_paths_regardless_of_case() {
    let p = CompiledPolicy::compile(&ctx(true), &builtin_rules(), &[], &[]).unwrap();
    is(&p, &rd("/home/tester/.SSH/id_rsa"), Ask, "builtin:ssh");
    // and the case-sensitive context does not
    allowed(&policy(), &rd("/home/tester/.SSH/id_rsa"));
}

// -- commands -------------------------------------------------------------------------------

#[test]
fn git_push_asks_with_any_arguments() {
    let p = policy();
    for c in ["git push", "git push --force", "git push origin main"] {
        is(&p, &cmd(c), Ask, "builtin:git-push");
    }
    allowed(&p, &cmd("git status"));
    allowed(&p, &cmd("git pushover"));
}

#[test]
fn chmod_777_asks() {
    let p = policy();
    is(&p, &cmd("chmod -R 777 /srv"), Ask, "builtin:chmod-777");
    is(&p, &cmd("chmod 777 f"), Ask, "builtin:chmod-777");
    allowed(&p, &cmd("chmod 644 f"));
    allowed(&p, &cmd("chmod +x f"));
}

#[test]
fn privilege_escalation_commands_are_denied() {
    let p = policy();
    is(&p, &cmd("sudo rm x"), Deny, "builtin:sudo");
    is(&p, &cmd("sudo"), Deny, "builtin:sudo");
    is(&p, &cmd("sudoedit /etc/hosts"), Deny, "builtin:sudoedit");
    is(&p, &cmd("su root"), Deny, "builtin:su");
    is(&p, &cmd("doas ls"), Deny, "builtin:doas");
    is(&p, &cmd("pkexec ls"), Deny, "builtin:pkexec");
    is(&p, &cmd("run0 ls"), Deny, "builtin:run0");
    // nearby negatives (word boundary)
    allowed(&p, &cmd("sudoku"));
    allowed(&p, &cmd("suspend"));
}

#[test]
fn scheduling_and_service_commands_ask() {
    let p = policy();
    is(&p, &cmd("at now"), Ask, "builtin:at");
    is(&p, &cmd("systemd-run --user x"), Ask, "builtin:systemd-run");
    is(&p, &cmd("launchctl load x.plist"), Ask, "builtin:launchctl");
    allowed(&p, &cmd("atom file"));
    allowed(&p, &cmd("attrib x"));
}

#[test]
fn crontab_listing_is_allowed_and_editing_asks() {
    let p = policy();
    is(&p, &cmd("crontab -l"), Allow, "builtin:crontab-list");
    is(&p, &cmd("crontab -e"), Ask, "builtin:crontab");
    is(&p, &cmd("crontab"), Ask, "builtin:crontab");
    is(&p, &cmd("crontab file.txt"), Ask, "builtin:crontab");
}

// -- hosts and layering ---------------------------------------------------------------------

#[test]
fn any_host_asks() {
    let p = policy();
    is(&p, &host("example.com"), Ask, "builtin:new-host");
    is(&p, &host("api.github.com"), Ask, "builtin:new-host");
}

#[test]
fn user_layer_host_allow_replaces_the_builtin_ask_for_that_host_only() {
    let p = policy_with(
        &[PolicyRule::host("example.com", Allow).with_id("ok-example")],
        &[],
    );
    is(&p, &host("example.com"), Allow, "ok-example");
    is(&p, &host("other.org"), Ask, "builtin:new-host");
}

#[test]
fn project_layer_allow_cannot_loosen_a_builtin_ask() {
    let p = policy_with(
        &[],
        &[PolicyRule::host("*", Allow).with_id("proj-allow-all")],
    );
    is(&p, &host("example.com"), Ask, "builtin:new-host");
    let p = policy_with(
        &[],
        &[PolicyRule::path("/srv/**", OpFilter::Write, Allow).with_id("proj-allow-srv")],
    );
    is(&p, &wr("/srv/x"), Ask, "builtin:outside-workdir");
}

#[test]
fn project_layer_deny_tightens_a_builtin_allow() {
    let p = policy_with(
        &[],
        &[PolicyRule::path("secret/**", OpFilter::Any, Deny).with_id("no-secret")],
    );
    is(&p, &rd("/work/proj/secret/k"), Deny, "no-secret");
    is(&p, &wr("/work/proj/secret/k"), Deny, "no-secret");
}

#[test]
fn user_layer_can_loosen_a_builtin_path_ask() {
    let p = policy_with(
        &[PolicyRule::path("~/.ssh/config", OpFilter::Read, Allow).with_id("ssh-config-ok")],
        &[],
    );
    is(&p, &rd("/home/tester/.ssh/config"), Allow, "ssh-config-ok");
    is(&p, &rd("/home/tester/.ssh/id_rsa"), Ask, "builtin:ssh");
}
