//! Security-review regression tests for policy model v2
//! (`docs/plans/policy-v2-spec.md`, sections 3, 4, 5, 6).
//!
//! Assumed shapes (from spec section 6):
//! - `PermissionError::UnsafeFile { path, reason }`
//! - `PermissionError::InvalidContext(String)`
//! - `PermissionError::TooManyRules { layer: Layer, count: usize, max: usize }`
//! - `PermissionError::UnsupportedVersion(i64)`

use std::path::{Path, PathBuf};

use hone_config::policy::{
    CompiledPolicy, Layer, OpFilter, PathOp, PolicyContext, PolicyRule, Subject, Verdict,
    load_policy_file,
};
use hone_config::{Decision, PermissionError};

use Decision::{Allow, Ask, Deny};

// -- helpers ------------------------------------------------------------------

fn ctx_at(workdir: &str, home: Option<&str>) -> PolicyContext {
    PolicyContext {
        workdir: PathBuf::from(workdir),
        home: home.map(PathBuf::from),
    }
}

fn ctx() -> PolicyContext {
    ctx_at("/work/proj", Some("/home/tester"))
}

fn compile_in(
    c: &PolicyContext,
    builtin: &[PolicyRule],
    user: &[PolicyRule],
    project: &[PolicyRule],
) -> CompiledPolicy {
    CompiledPolicy::compile(c, builtin, user, project).expect("policy should compile")
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
    compile_in(&ctx(), &[], rules, &[])
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

fn decision_of(p: &CompiledPolicy, s: &Subject) -> Decision {
    p.evaluate(s).decision
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

fn is_unsafe_file<T>(r: &Result<T, PermissionError>) -> bool {
    matches!(r, Err(PermissionError::UnsafeFile { .. }))
}

// -- B1: glob metacharacters in workdir / home --------------------------------

fn assert_literal_workdir(workdir: &str, lookalike: &str) {
    let c = ctx_at(workdir, Some("/home/tester"));
    let p = compile_in(
        &c,
        &[PolicyRule::path(".env", OpFilter::Any, Deny)],
        &[],
        &[PolicyRule::path(
            "{workdir}/secret.txt",
            OpFilter::Any,
            Deny,
        )],
    );
    assert_eq!(
        decision_of(&p, &read(&format!("{workdir}/.env"))),
        Deny,
        "relative pattern, {workdir}"
    );
    assert_eq!(
        decision_of(&p, &read(&format!("{workdir}/secret.txt"))),
        Deny,
        "{{workdir}} pattern, {workdir}"
    );
    assert_eq!(
        decision_of(&p, &read(&format!("{lookalike}/.env"))),
        Allow,
        "lookalike {lookalike}"
    );
    assert_eq!(
        decision_of(&p, &read(&format!("{lookalike}/secret.txt"))),
        Allow,
        "lookalike {lookalike}"
    );
}

#[test]
fn workdir_with_backslash_matches_literally() {
    assert_literal_workdir("/tmp/a\\b", "/tmp/ab");
}

#[test]
fn workdir_with_character_class_matches_literally() {
    assert_literal_workdir("/work/a[1]", "/work/a1");
}

#[test]
fn workdir_with_star_matches_literally() {
    assert_literal_workdir("/work/a*", "/work/abc");
}

#[test]
fn workdir_with_question_mark_matches_literally() {
    assert_literal_workdir("/work/a?", "/work/ab");
}

#[test]
fn workdir_with_braces_matches_literally() {
    assert_literal_workdir("/work/a{x}", "/work/ax");
}

#[test]
fn workdir_with_braces_alternation_matches_literally() {
    assert_literal_workdir("/work/{a,b}", "/work/a");
}

#[test]
fn home_with_glob_metacharacters_matches_literally() {
    for (home, lookalike) in [
        ("/home/u[1]", "/home/u1"),
        ("/home/u*", "/home/uzz"),
        ("/home/u\\v", "/home/uv"),
    ] {
        let c = ctx_at("/work/proj", Some(home));
        let p = compile_in(
            &c,
            &[],
            &[PolicyRule::path("~/.ssh/**", OpFilter::Any, Deny)],
            &[],
        );
        assert_eq!(
            decision_of(&p, &read(&format!("{home}/.ssh/k"))),
            Deny,
            "{home}"
        );
        assert_eq!(
            decision_of(&p, &read(&format!("{lookalike}/.ssh/k"))),
            Allow,
            "{home}"
        );
    }
}

#[test]
fn workdir_dotdot_pattern_with_glob_chars_in_workdir_matches_sibling() {
    let c = ctx_at("/work/a[1]", Some("/home/tester"));
    let p = compile_in(
        &c,
        &[],
        &[PolicyRule::path(
            "{workdir}/../shared/**",
            OpFilter::Any,
            Deny,
        )],
        &[],
    );
    assert_eq!(decision_of(&p, &read("/work/shared/x")), Deny);
    assert_eq!(
        decision_of(&p, &read("/work/a1/../shared/x")),
        Deny,
        "subject also normalised"
    );
    assert_eq!(decision_of(&p, &read("/work/other/x")), Allow);
}

#[test]
fn pattern_dotdot_is_normalised_after_expansion() {
    let c = ctx_at("/work/proj", Some("/home/tester"));
    let p = compile_in(
        &c,
        &[],
        &[
            PolicyRule::path("{workdir}/../x/**", OpFilter::Any, Deny),
            PolicyRule::path("/a/b/../c", OpFilter::Any, Ask),
        ],
        &[],
    );
    assert_eq!(decision_of(&p, &read("/work/x/y")), Deny);
    assert_eq!(decision_of(&p, &read("/work/proj/x/y")), Allow);
    assert_eq!(decision_of(&p, &read("/a/c")), Ask);
}

#[test]
fn trailing_slash_workdir_is_normalised() {
    let c = ctx_at("/work/proj/", Some("/home/tester"));
    let p = compile_in(
        &c,
        &[],
        &[PolicyRule::path("{workdir}/.env", OpFilter::Any, Deny)],
        &[],
    );
    assert_eq!(decision_of(&p, &read("/work/proj/.env")), Deny);
}

// -- context validation -------------------------------------------------------

#[test]
fn relative_workdir_is_invalid_context() {
    let r = CompiledPolicy::compile(&ctx_at("work/proj", Some("/home/tester")), &[], &[], &[]);
    assert!(matches!(r, Err(PermissionError::InvalidContext(_))));
}

#[test]
fn relative_home_is_invalid_context() {
    let r = CompiledPolicy::compile(&ctx_at("/work/proj", Some("home/tester")), &[], &[], &[]);
    assert!(matches!(r, Err(PermissionError::InvalidContext(_))));
}

#[test]
fn empty_workdir_is_invalid_context() {
    let r = CompiledPolicy::compile(&ctx_at("", None), &[], &[], &[]);
    assert!(matches!(r, Err(PermissionError::InvalidContext(_))));
}

// -- B2: loader hardening -----------------------------------------------------

#[cfg(unix)]
#[test]
fn fifo_is_unsafe_file_and_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("p.toml");
    let st = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo available");
    assert!(st.success());

    let (tx, rx) = std::sync::mpsc::channel();
    let path = fifo.clone();
    std::thread::spawn(move || {
        let _ = tx.send(load_policy_file(&path).map(|_| ()));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(r) => assert!(is_unsafe_file(&r), "{r:?}"),
        Err(_) => panic!("load_policy_file blocked on a FIFO"),
    }
}

#[test]
fn directory_is_unsafe_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(is_unsafe_file(&load_policy_file(dir.path())));
}

#[test]
fn file_over_64_kib_is_unsafe_file() {
    let dir = tempfile::tempdir().unwrap();
    let body = "# padding\n".repeat(7000); // ~70 KB of valid TOML comments
    assert!(body.len() > 64 * 1024);
    let f = write_file(dir.path(), "big.toml", &body);
    assert!(is_unsafe_file(&load_policy_file(&f)));
}

#[test]
fn file_just_under_64_kib_loads() {
    let dir = tempfile::tempdir().unwrap();
    let mut body = String::from("[[rules]]\ncommand = \"ls\"\ndecision = \"deny\"\n");
    while body.len() < 64 * 1024 - 100 {
        body.push_str("# padding padding padding\n");
    }
    assert!(body.len() <= 64 * 1024);
    let f = write_file(dir.path(), "ok.toml", &body);
    assert_eq!(load_policy_file(&f).unwrap().len(), 1);
}

#[test]
fn toml_syntax_error_message_contains_no_file_text() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(
        dir.path(),
        "p.toml",
        "[[rules]]\ncommand = \"ok\"\nHC-CANARY-TOML-LEAK = = =\ndecision = \"deny\"\n",
    );
    match load_policy_file(&f) {
        Err(e @ PermissionError::Toml(_)) => {
            assert!(
                !e.to_string().contains("HC-CANARY-TOML-LEAK"),
                "Display leaked: {e}"
            );
            assert!(
                !format!("{e:?}").contains("HC-CANARY-TOML-LEAK"),
                "Debug leaked: {e:?}"
            );
        }
        other => panic!("expected Toml error, got {other:?}"),
    }
}

#[test]
fn toml_unknown_key_message_contains_no_file_text() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(
        dir.path(),
        "p.toml",
        "[[rules]]\ncommand = \"ok\"\ndecision = \"deny\"\nHC-CANARY-TOML-LEAK = 1\n",
    );
    match load_policy_file(&f) {
        Err(e @ PermissionError::Toml(_)) => {
            assert!(
                !e.to_string().contains("HC-CANARY-TOML-LEAK"),
                "leaked: {e}"
            );
        }
        other => panic!("expected Toml error, got {other:?}"),
    }
}

#[test]
fn toml_bad_value_message_contains_no_file_text() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(
        dir.path(),
        "p.toml",
        "[[rules]]\ncommand = \"ok\"\ndecision = \"HC-CANARY-TOML-LEAK\"\n",
    );
    match load_policy_file(&f) {
        Err(e @ PermissionError::Toml(_)) => {
            assert!(
                !e.to_string().contains("HC-CANARY-TOML-LEAK"),
                "leaked: {e}"
            );
        }
        other => panic!("expected Toml error, got {other:?}"),
    }
}

#[test]
fn toml_error_message_still_locates_the_problem() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(dir.path(), "p.toml", "[[rules]]\ncommand = = \"x\"\n");
    match load_policy_file(&f) {
        Err(PermissionError::Toml(msg)) => {
            assert!(!msg.is_empty());
            assert!(msg.contains('2'), "expected line number in {msg:?}");
        }
        other => panic!("expected Toml error, got {other:?}"),
    }
}

#[test]
fn version_negative_is_unsupported_version() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(dir.path(), "p.toml", "version = -1\n");
    match load_policy_file(&f) {
        Err(PermissionError::UnsupportedVersion(v)) => assert_eq!(v, -1i64),
        other => panic!("expected UnsupportedVersion(-1), got {other:?}"),
    }
}

#[test]
fn version_huge_is_unsupported_version() {
    let dir = tempfile::tempdir().unwrap();
    let f = write_file(dir.path(), "p.toml", "version = 4294967298\n"); // 2^32 + 2
    match load_policy_file(&f) {
        Err(PermissionError::UnsupportedVersion(v)) => assert_eq!(v, 4_294_967_298i64),
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn load_policy_file_follows_symlink_to_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let real = write_file(
        dir.path(),
        "real.toml",
        "[[rules]]\ncommand = \"ls\"\ndecision = \"deny\"\n",
    );
    let link = dir.path().join("link.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(load_policy_file(&link).unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn project_path_that_is_a_symlink_is_unsafe_file() {
    let dir = tempfile::tempdir().unwrap();
    let real = write_file(
        dir.path(),
        "real.toml",
        "[[rules]]\ncommand = \"ls\"\ndecision = \"deny\"\n",
    );
    let link = dir.path().join("project.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let r = CompiledPolicy::load_hierarchical_with_paths(
        &ctx(),
        &[],
        &dir.path().join("none.toml"),
        &link,
    );
    assert!(is_unsafe_file(&r));
}

#[cfg(unix)]
#[test]
fn project_path_symlink_to_secret_does_not_leak_contents() {
    let dir = tempfile::tempdir().unwrap();
    let secret = write_file(
        dir.path(),
        "secret.txt",
        "HC-CANARY-TOML-LEAK is not toml\n",
    );
    let link = dir.path().join("project.toml");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    match CompiledPolicy::load_hierarchical_with_paths(
        &ctx(),
        &[],
        &dir.path().join("none.toml"),
        &link,
    ) {
        Err(e) => assert!(
            !e.to_string().contains("HC-CANARY-TOML-LEAK"),
            "leaked: {e}"
        ),
        Ok(_) => panic!("expected an error"),
    }
}

#[cfg(unix)]
#[test]
fn user_path_that_is_a_symlink_loads_fine() {
    let dir = tempfile::tempdir().unwrap();
    let real = write_file(
        dir.path(),
        "real.toml",
        "[[rules]]\ncommand = \"ls\"\ndecision = \"deny\"\n",
    );
    let link = dir.path().join("user.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let p = CompiledPolicy::load_hierarchical_with_paths(
        &ctx(),
        &[],
        &link,
        &dir.path().join("none.toml"),
    )
    .unwrap();
    assert_verdict(
        &p.evaluate(&cmd("ls")),
        Deny,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn user_path_that_is_a_directory_is_unsafe_file() {
    let dir = tempfile::tempdir().unwrap();
    let r = CompiledPolicy::load_hierarchical_with_paths(
        &ctx(),
        &[],
        dir.path(),
        &dir.path().join("none.toml"),
    );
    assert!(is_unsafe_file(&r));
}

#[cfg(unix)]
#[test]
fn dangling_project_symlink_is_unsafe_file_while_missing_user_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("project.toml");
    std::os::unix::fs::symlink(dir.path().join("does-not-exist"), &link).unwrap();
    let r = CompiledPolicy::load_hierarchical_with_paths(
        &ctx(),
        &[],
        &dir.path().join("no-user.toml"),
        &link,
    );
    assert!(
        is_unsafe_file(&r),
        "dangling project symlink must not be treated as missing"
    );

    // Control: a genuinely missing user file and missing project file load as empty.
    let p = CompiledPolicy::load_hierarchical_with_paths(
        &ctx(),
        &[],
        &dir.path().join("no-user.toml"),
        &dir.path().join("no-project.toml"),
    )
    .unwrap();
    assert_verdict(&p.evaluate(&cmd("ls")), Allow, None, None);
}

// -- B3: load_hierarchical ----------------------------------------------------

fn setup_home_and_workdir() -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    (home, work)
}

#[test]
fn load_hierarchical_reads_home_config_and_workdir_hone_dir() {
    let (home, work) = setup_home_and_workdir();
    std::fs::create_dir_all(home.path().join(".config/hone")).unwrap();
    std::fs::create_dir_all(work.path().join(".hone")).unwrap();
    write_file(
        &home.path().join(".config/hone"),
        "permissions.toml",
        "[[rules]]\ncommand = \"rm *\"\ndecision = \"allow\"\n",
    );
    write_file(
        &work.path().join(".hone"),
        "permissions.toml",
        "[[rules]]\ncommand = \"git push*\"\ndecision = \"deny\"\n",
    );
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: Some(home.path().to_path_buf()),
    };
    let p = CompiledPolicy::load_hierarchical(&c, &[PolicyRule::command("rm *", Deny)]).unwrap();
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
    assert_verdict(
        &p.evaluate(&cmd("git push")),
        Deny,
        Some("project:1"),
        Some(Layer::Project),
    );
}

#[test]
fn load_hierarchical_with_no_files_is_builtin_only() {
    let (home, work) = setup_home_and_workdir();
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: Some(home.path().to_path_buf()),
    };
    let p = CompiledPolicy::load_hierarchical(&c, &[PolicyRule::command("rm *", Deny)]).unwrap();
    assert_verdict(
        &p.evaluate(&cmd("rm x")),
        Deny,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn load_hierarchical_without_home_is_invalid_context() {
    let work = tempfile::tempdir().unwrap();
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: None,
    };
    let r = CompiledPolicy::load_hierarchical(&c, &[]);
    assert!(matches!(r, Err(PermissionError::InvalidContext(_))));
}

#[test]
fn load_hierarchical_propagates_broken_project_file() {
    let (home, work) = setup_home_and_workdir();
    std::fs::create_dir_all(work.path().join(".hone")).unwrap();
    write_file(&work.path().join(".hone"), "permissions.toml", "[[rules\n");
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: Some(home.path().to_path_buf()),
    };
    assert!(matches!(
        CompiledPolicy::load_hierarchical(&c, &[]),
        Err(PermissionError::Toml(_))
    ));
}

#[cfg(unix)]
#[test]
fn load_hierarchical_hone_dir_symlink_is_unsafe_file() {
    let (home, work) = setup_home_and_workdir();
    let elsewhere = tempfile::tempdir().unwrap();
    write_file(
        elsewhere.path(),
        "permissions.toml",
        "[[rules]]\ncommand = \"ls\"\ndecision = \"deny\"\n",
    );
    std::os::unix::fs::symlink(elsewhere.path(), work.path().join(".hone")).unwrap();
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: Some(home.path().to_path_buf()),
    };
    assert!(is_unsafe_file(&CompiledPolicy::load_hierarchical(&c, &[])));
}

#[cfg(unix)]
#[test]
fn load_hierarchical_project_file_symlink_is_unsafe_file() {
    let (home, work) = setup_home_and_workdir();
    std::fs::create_dir_all(work.path().join(".hone")).unwrap();
    let real = write_file(
        work.path(),
        "real.toml",
        "[[rules]]\ncommand = \"ls\"\ndecision = \"deny\"\n",
    );
    std::os::unix::fs::symlink(&real, work.path().join(".hone/permissions.toml")).unwrap();
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: Some(home.path().to_path_buf()),
    };
    assert!(is_unsafe_file(&CompiledPolicy::load_hierarchical(&c, &[])));
}

// -- Host subjects ------------------------------------------------------------

fn assert_invalid_host(p: &CompiledPolicy, subject: &str) {
    assert_verdict(
        &p.evaluate(&host(subject)),
        Ask,
        Some("builtin:invalid-host"),
        Some(Layer::Builtin),
    );
}

#[test]
fn malformed_host_subjects_ask_even_with_allow_rules() {
    let allow = [
        PolicyRule::host("*.pypi.org", Allow),
        PolicyRule::host("*", Allow),
    ];
    let p = compile_in(&ctx(), &allow, &allow, &[]);
    let bad = [
        "files.pypi.org:x@evil.com",
        "user@files.pypi.org",
        "evil.com/x.pypi.org",
        "evil.com?.pypi.org",
        "evil.com#x.pypi.org",
        "evil.com\\x.pypi.org",
        "evil.com .pypi.org",
        "evil.com\t.pypi.org",
        "evil.com\n.pypi.org",
        "a\u{0}b.pypi.org",
        "a\u{1b}b.pypi.org",
        "\u{ff45}\u{ff58}\u{ff41}\u{ff4d}\u{ff50}\u{ff4c}\u{ff45}.com",
        "b\u{fc}cher.de",
        "evil.com:80a",
        "evil.com:abc",
        "",
        ".",
        "...",
        "[::1",
        "[zzz]",
        "[evil.com]",
        "a:b:c",
        "[::1]x",
    ];
    for s in bad {
        assert_invalid_host(&p, s);
    }
}

#[test]
fn malformed_host_asks_even_with_no_rules_at_all() {
    let p = compile_in(&ctx(), &[], &[], &[]);
    assert_invalid_host(&p, "evil.com/x");
}

#[test]
fn malformed_host_in_evaluate_all_makes_the_whole_call_ask() {
    let p = user_only(&[PolicyRule::command("ls", Allow)]);
    let v = p.evaluate_all(&[cmd("ls"), host("a@b.com")]);
    assert_verdict(&v, Ask, Some("builtin:invalid-host"), Some(Layer::Builtin));
}

#[test]
fn well_formed_hosts_are_not_flagged() {
    let p = user_only(&[PolicyRule::host("*.pypi.org", Allow)]);
    assert_verdict(
        &p.evaluate(&host("files.pypi.org")),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
    assert_verdict(
        &p.evaluate(&host("files.pypi.org:443")),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
    assert_verdict(&p.evaluate(&host("example.com")), Allow, None, None);
}

#[test]
fn ipv6_forms_match_canonical_deny_rules() {
    for pattern in ["::1", "[::1]"] {
        let p = user_only(&[PolicyRule::host(pattern, Deny)]);
        for s in [
            "::1",
            "[::1]",
            "[::1]:443",
            "0:0:0:0:0:0:0:1",
            "0000:0000:0000:0000:0000:0000:0000:0001",
        ] {
            assert_eq!(decision_of(&p, &host(s)), Deny, "rule {pattern} vs {s}");
        }
        assert_eq!(decision_of(&p, &host("::2")), Allow);
    }
}

#[test]
fn ipv4_mapped_ipv6_matches_ipv4_rule() {
    let p = user_only(&[PolicyRule::host("127.0.0.1", Deny)]);
    for s in [
        "127.0.0.1",
        "127.0.0.1:8080",
        "::ffff:127.0.0.1",
        "[::ffff:7f00:1]",
        "[::ffff:127.0.0.1]:80",
    ] {
        assert_eq!(decision_of(&p, &host(s)), Deny, "{s}");
    }
    assert_eq!(decision_of(&p, &host("127.0.0.2")), Allow);
}

#[test]
fn many_trailing_dots_are_all_stripped() {
    let p = user_only(&[PolicyRule::host("evil.com", Deny)]);
    assert_eq!(decision_of(&p, &host("evil.com..")), Deny);
    assert_eq!(decision_of(&p, &host("EVIL.com...")), Deny);
    assert_eq!(decision_of(&p, &host("evil.com..:443")), Deny);
}

#[test]
fn host_patterns_that_could_never_match_are_invalid_rules() {
    for pattern in [
        "example.com:443",
        "*.com:80",
        "[::1]:443",
        "b\u{fc}cher.de",
        "\u{ff45}xample.com",
        "user@example.com",
        "example.com/x",
        "example.com#x",
        "example.com\\x",
        "ex ample.com",
    ] {
        let e = compile_err(
            &[],
            &[
                PolicyRule::command("ok", Allow),
                PolicyRule::host(pattern, Allow),
            ],
            &[],
        );
        assert_invalid_rule(&e, Layer::User, 2);
    }
}

#[test]
fn bracketed_ipv6_pattern_is_a_literal_not_a_character_class() {
    let p = user_only(&[PolicyRule::host("[::1]", Deny)]);
    // As a character class "[::1]" would match the single characters ':' or '1'.
    assert_eq!(decision_of(&p, &host("1")), Allow);
    assert_eq!(decision_of(&p, &host("::1")), Deny);
}

// -- patterns that can never match -------------------------------------------

#[test]
fn command_pattern_with_surrounding_whitespace_is_invalid_rule() {
    for pattern in [" ls", "ls ", "ls\n", "\tls", " cargo *", "cargo * "] {
        let e = compile_err(
            &[],
            &[],
            &[
                PolicyRule::command("ok", Allow),
                PolicyRule::command(pattern, Allow),
            ],
        );
        assert_invalid_rule(&e, Layer::Project, 2);
    }
}

#[test]
fn command_pattern_with_inner_whitespace_is_fine() {
    let p = user_only(&[PolicyRule::command("git  push", Deny)]);
    assert_eq!(decision_of(&p, &cmd("git  push")), Deny);
}

#[test]
fn path_pattern_with_unresolvable_dotdot_is_invalid_rule() {
    for pattern in ["/a/**/../b", "/a/*/../b", "/a/b?/../c", "/a/[xy]/../c"] {
        let e = compile_err(&[], &[PolicyRule::path(pattern, OpFilter::Any, Deny)], &[]);
        assert_invalid_rule(&e, Layer::User, 1);
    }
}

#[test]
fn path_pattern_with_partial_double_star_is_invalid_rule() {
    for pattern in [
        "/x/**secret",
        "/x/a**",
        "**secret",
        "a**",
        "/x/**b/y",
        "/x/a**b",
    ] {
        let e = compile_err(&[], &[], &[PolicyRule::path(pattern, OpFilter::Any, Deny)]);
        assert_invalid_rule(&e, Layer::Project, 1);
    }
}

#[test]
fn path_pattern_with_whole_segment_double_star_is_fine() {
    let p = user_only(&[PolicyRule::path("/x/**/y", OpFilter::Any, Deny)]);
    assert_eq!(decision_of(&p, &read("/x/a/b/y")), Deny);
}

#[test]
fn pattern_longer_than_1024_bytes_is_invalid_rule_for_every_kind() {
    let long = "a".repeat(1025);
    for rule in [
        PolicyRule::command(long.clone(), Deny),
        PolicyRule::path(format!("/{long}"), OpFilter::Any, Deny),
        PolicyRule::host(long.clone(), Deny),
    ] {
        let e = compile_err(&[], &[rule], &[]);
        assert_invalid_rule(&e, Layer::User, 1);
    }
}

#[test]
fn command_pattern_of_exactly_1024_bytes_is_accepted() {
    let p = user_only(&[PolicyRule::command("a".repeat(1024), Deny)]);
    assert_eq!(decision_of(&p, &cmd(&"a".repeat(1024))), Deny);
}

#[test]
fn pattern_length_limit_counts_bytes_not_chars() {
    // 600 two-byte chars = 1200 bytes but only 600 chars.
    let long = "\u{e9}".repeat(600);
    let e = compile_err(&[], &[PolicyRule::command(long, Deny)], &[]);
    assert_invalid_rule(&e, Layer::User, 1);
}

// -- ids ----------------------------------------------------------------------

#[test]
fn explicit_ids_outside_the_charset_are_invalid_rules() {
    let too_long = "a".repeat(65);
    for bad in [
        "\u{1b}[31mred",
        "zero\u{200b}width",
        "caf\u{e9}",
        "a/b",
        "a;b",
        "a$b",
        too_long.as_str(),
        "a\u{0}b",
    ] {
        let e = compile_err(&[], &[PolicyRule::command("a", Allow).with_id(bad)], &[]);
        assert_invalid_rule(&e, Layer::User, 1);
    }
}

#[test]
fn explicit_ids_inside_the_charset_are_accepted() {
    let max = "a".repeat(64);
    for good in ["a.b-c_9", "A", "0", "x.y.z", max.as_str(), "-lead", ".dot"] {
        let p = user_only(&[PolicyRule::command("a", Allow).with_id(good)]);
        assert_eq!(p.evaluate(&cmd("a")).rule_id.as_deref(), Some(good));
    }
}

// -- limits -------------------------------------------------------------------

fn many_rules(n: usize) -> Vec<PolicyRule> {
    (0..n)
        .map(|i| PolicyRule::command(format!("c{i}"), Deny))
        .collect()
}

#[test]
fn one_thousand_rules_in_a_layer_is_accepted() {
    for layer in 0..3 {
        let rules = many_rules(1000);
        let r = match layer {
            0 => CompiledPolicy::compile(&ctx(), &rules, &[], &[]),
            1 => CompiledPolicy::compile(&ctx(), &[], &rules, &[]),
            _ => CompiledPolicy::compile(&ctx(), &[], &[], &rules),
        };
        assert!(r.is_ok(), "layer {layer}");
    }
}

#[test]
fn more_than_one_thousand_rules_in_a_layer_is_too_many_rules() {
    let rules = many_rules(1001);
    for (layer, r) in [
        (
            Layer::Builtin,
            CompiledPolicy::compile(&ctx(), &rules, &[], &[]),
        ),
        (
            Layer::User,
            CompiledPolicy::compile(&ctx(), &[], &rules, &[]),
        ),
        (
            Layer::Project,
            CompiledPolicy::compile(&ctx(), &[], &[], &rules),
        ),
    ] {
        match r {
            Err(PermissionError::TooManyRules {
                layer: l,
                count,
                max,
            }) => {
                assert_eq!(l, layer);
                assert_eq!(count, 1001);
                assert_eq!(max, 1000);
            }
            Err(other) => panic!("expected TooManyRules, got {other:?}"),
            Ok(_) => panic!("expected TooManyRules"),
        }
    }
}

#[test]
fn the_rule_limit_is_per_layer_not_total() {
    let r = CompiledPolicy::compile(
        &ctx(),
        &many_rules(1000),
        &many_rules(1000),
        &many_rules(1000),
    );
    assert!(r.is_ok());
}

// -- commands -----------------------------------------------------------------

#[test]
fn literal_allow_does_not_match_after_newline() {
    let p = user_only(&[PolicyRule::command("cargo test", Allow)]);
    assert_verdict(&p.evaluate(&cmd("cargo test\nrm -rf ~")), Allow, None, None);
    assert_verdict(
        &p.evaluate(&cmd("cargo test\r\nrm -rf ~")),
        Allow,
        None,
        None,
    );
}

#[test]
fn literal_newline_is_not_a_boundary_against_a_tightening_rule() {
    // Builtin asks for everything starting `cargo test`; a user allow for the literal
    // must not shadow it for a newline-smuggled command.
    let p = compile_in(
        &ctx(),
        &[PolicyRule::command("cargo*", Ask)],
        &[PolicyRule::command("cargo test", Allow)],
        &[],
    );
    assert_verdict(
        &p.evaluate(&cmd("cargo test --lib")),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
    assert_verdict(
        &p.evaluate(&cmd("cargo test\nrm -rf ~")),
        Ask,
        Some("builtin:1"),
        Some(Layer::Builtin),
    );
}

#[test]
fn literal_prefix_boundary_is_space_or_tab() {
    let p = user_only(&[PolicyRule::command("cargo test", Deny)]);
    assert_eq!(decision_of(&p, &cmd("cargo test x")), Deny);
    assert_eq!(decision_of(&p, &cmd("cargo test\tx")), Deny);
    assert_eq!(decision_of(&p, &cmd("cargo test\nx")), Allow);
    assert_eq!(decision_of(&p, &cmd("cargo test\u{b}x")), Allow);
    assert_eq!(decision_of(&p, &cmd("cargo test;x")), Allow);
}

// -- N2: non-UTF-8 path subjects ---------------------------------------------

#[cfg(unix)]
mod non_utf8 {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    fn raw(bytes: &[u8]) -> Subject {
        Subject::Path {
            path: PathBuf::from(OsStr::from_bytes(bytes)),
            op: PathOp::Read,
        }
    }

    #[test]
    fn replacement_char_rule_does_not_match_invalid_utf8_subject() {
        let p = user_only(&[PolicyRule::path("/work/proj/\u{FFFD}", OpFilter::Any, Deny)]);
        assert_eq!(decision_of(&p, &raw(b"/work/proj/\xff")), Allow);
        assert_eq!(decision_of(&p, &raw(b"/work/proj/\xfe")), Allow);
        assert_eq!(decision_of(&p, &read("/work/proj/\u{FFFD}")), Deny);
    }

    #[test]
    fn different_invalid_names_do_not_collide() {
        let p = user_only(&[PolicyRule::path(
            "/work/proj/\u{FFFD}\u{FFFD}",
            OpFilter::Any,
            Deny,
        )]);
        assert_eq!(decision_of(&p, &raw(b"/work/proj/\xff\xff")), Allow);
        assert_eq!(decision_of(&p, &raw(b"/work/proj/\xc0\xc1")), Allow);
    }

    #[test]
    fn invalid_utf8_subject_is_still_matched_by_wildcards() {
        let p = user_only(&[PolicyRule::path("/work/proj/**", OpFilter::Any, Deny)]);
        assert_eq!(decision_of(&p, &raw(b"/work/proj/\xff/x")), Deny);
        let q = user_only(&[PolicyRule::path("/work/proj/*", OpFilter::Any, Ask)]);
        assert_eq!(decision_of(&q, &raw(b"/work/proj/\xff")), Ask);
    }

    #[test]
    fn invalid_utf8_dotdot_is_normalised_bytewise() {
        let p = user_only(&[PolicyRule::path("/work/x", OpFilter::Any, Deny)]);
        assert_eq!(decision_of(&p, &raw(b"/work/proj/\xff/../../x")), Deny);
    }
}

// -- Send + Sync --------------------------------------------------------------

#[test]
fn compiled_policy_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CompiledPolicy>();
}

// -- property: project layer can only tighten --------------------------------

fn rank(d: Decision) -> u8 {
    match d {
        Allow => 0,
        Ask => 1,
        Deny => 2,
    }
}

#[test]
fn property_project_rules_never_loosen_and_yield_base_or_project_verdict() {
    let decisions = [Allow, Ask, Deny];
    let patterns = ["a", "b", "*"];
    let universe: Vec<(&str, Decision)> = patterns
        .iter()
        .flat_map(|p| decisions.iter().map(move |d| (*p, *d)))
        .collect();
    let mk = |(p, d): (&str, Decision)| PolicyRule::command(p, d);

    // all lists of length 0..=max_len over the rule universe
    let lists = |max_len: usize| -> Vec<Vec<PolicyRule>> {
        let mut out: Vec<Vec<PolicyRule>> = vec![vec![]];
        let mut frontier: Vec<Vec<(&str, Decision)>> = vec![vec![]];
        for _ in 0..max_len {
            let mut next = vec![];
            for l in &frontier {
                for r in &universe {
                    let mut n = l.clone();
                    n.push(*r);
                    out.push(n.iter().copied().map(mk).collect());
                    next.push(n);
                }
            }
            frontier = next;
        }
        out
    };

    let builtins = lists(1);
    let users = lists(1);
    let projects = lists(2);
    let subjects = ["a", "b", "c", "a x"];
    let mut checked = 0usize;

    for b in &builtins {
        for u in &users {
            let base = compile_in(&ctx(), b, u, &[]);
            for pr in &projects {
                let with = compile_in(&ctx(), b, u, pr);
                for s in subjects {
                    let sub = cmd(s);
                    let vb = base.evaluate(&sub);
                    let vw = with.evaluate(&sub);
                    assert!(
                        rank(vw.decision) >= rank(vb.decision),
                        "project loosened {s}: base {vb:?} -> {vw:?}"
                    );
                    let same_as_base = vw.decision == vb.decision
                        && vw.rule_id == vb.rule_id
                        && vw.layer == vb.layer;
                    if vw.layer == Some(Layer::Project) {
                        assert!(
                            rank(vw.decision) > rank(vb.decision),
                            "project verdict must be strictly stricter"
                        );
                    } else {
                        assert!(
                            same_as_base,
                            "non-project verdict must equal base: {vb:?} vs {vw:?}"
                        );
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 10_000, "property loop ran only {checked} cases");
}

// -- round 3: error text hygiene ----------------------------------------------

fn assert_no_control(label: &str, s: &str) {
    assert!(
        !s.chars().any(|c| c.is_control()),
        "{label} contains a raw control character: {s:?}"
    );
}

fn has_line_number(msg: &str) -> bool {
    let b: Vec<char> = msg.chars().collect();
    b.windows(3)
        .any(|w| w[0].is_ascii_digit() && w[1] == ':' && w[2].is_ascii_digit())
        || msg.contains("line ")
}

fn project_setup(body: &str) -> (tempfile::TempDir, tempfile::TempDir, PolicyContext) {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(work.path().join(".hone")).unwrap();
    write_file(&work.path().join(".hone"), "permissions.toml", body);
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: Some(home.path().to_path_buf()),
    };
    (home, work, c)
}

#[test]
fn project_file_errors_never_echo_file_text_or_control_chars() {
    // (toml source, substrings that must never appear in the error)
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("version = 'x\"HC-MARK-1'\n", vec!["HC-MARK-1"]),
        ("rules = 'a\"HC-MARK-2'\n", vec!["HC-MARK-2"]),
        ("rules = ['x\"HC-MARK-3']\n", vec!["HC-MARK-3"]),
        (
            "[[rules]]\npath = \"/f\"\nop = \"q`\\u001b]52;c;cHduZWQ=\\u0007`\"\ndecision = \"deny\"\n",
            vec!["cHduZWQ", "]52"],
        ),
        (
            "[[rules]]\ncommand = \"a\"\ndecision = \"deny\"\n\"HC-MARK-4\\\"x\" = 1\n",
            vec!["HC-MARK-4"],
        ),
        (
            "version = 'x\"IGNORE ALL PREVIOUS INSTRUCTIONS'\n",
            vec!["IGNORE", "PREVIOUS"],
        ),
        (
            "[[rules]]\ncommand = \"a\"\ndecision = 'HC-MARK-5`x'\n",
            vec!["HC-MARK-5"],
        ),
        (
            "[[rules]]\ncommand = \"a\"\ndecision = \"deny\"\nHC-MARK-6'x = 1\n",
            vec!["HC-MARK-6"],
        ),
        (
            "[[rules]]\ncommand = 'HC-MARK-7'\ndecision = \"deny\"\nid = 5\n",
            vec!["HC-MARK-7"],
        ),
    ];
    for (body, banned) in cases {
        let (_h, _w, c) = project_setup(body);
        let err = match CompiledPolicy::load_hierarchical(&c, &[]) {
            Err(e) => e,
            Ok(_) => panic!("expected error for {body:?}"),
        };
        let shown = err.to_string();
        let dbg = format!("{err:?}");
        for b in &banned {
            assert!(
                !shown.contains(b),
                "Display leaked {b:?}: {shown:?} (source {body:?})"
            );
            assert!(
                !dbg.contains(b),
                "Debug leaked {b:?}: {dbg:?} (source {body:?})"
            );
        }
        assert_no_control("Display", &shown);
        assert_no_control("Debug", &dbg);
        if let PermissionError::Toml(msg) = &err {
            assert!(has_line_number(msg), "no line number in {msg:?}");
            assert_no_control("Toml msg", msg);
            for q in ['`', '"', '\''] {
                assert!(!msg.contains(q), "Toml msg contains quote {q:?}: {msg:?}");
            }
        } else {
            panic!("expected Toml error for {body:?}, got {err:?}");
        }
    }
}

#[test]
fn control_chars_in_patterns_are_invalid_rules() {
    for bad in ["\u{1b}", "\u{7}", "\u{0}", "\u{7f}", "\n"] {
        for (i, rule) in [
            PolicyRule::command(format!("ls{bad}x"), Deny),
            PolicyRule::path(format!("/a{bad}b"), OpFilter::Any, Deny),
            PolicyRule::host(format!("a{bad}b.com"), Deny),
        ]
        .into_iter()
        .enumerate()
        {
            let e = compile_err(&[], &[PolicyRule::command("ok", Allow), rule], &[]);
            assert_invalid_rule(&e, Layer::User, 2);
            assert_no_control(&format!("kind {i} Display"), &e.to_string());
            assert_no_control(&format!("kind {i} Debug"), &format!("{e:?}"));
        }
    }
}

#[test]
fn control_chars_in_ids_are_invalid_rules_with_clean_display() {
    for bad in ["a\u{1b}b", "a\u{7}", "a\u{0}", "a\u{7f}", "a\nb"] {
        let e = compile_err(&[], &[], &[PolicyRule::command("a", Allow).with_id(bad)]);
        assert_invalid_rule(&e, Layer::Project, 1);
        assert_no_control("Display", &e.to_string());
    }
}

#[test]
fn invalid_glob_display_has_no_control_chars() {
    let e = compile_err(
        &[],
        &[PolicyRule::command("cargo [*\u{b0}\u{2603}", Deny)],
        &[],
    );
    assert!(matches!(e, PermissionError::InvalidGlob { .. }), "{e:?}");
    assert_no_control("Display", &e.to_string());
}

#[cfg(unix)]
#[test]
fn unsafe_file_display_escapes_control_chars_in_path() {
    let dir = tempfile::tempdir().unwrap();
    let weird = dir.path().join("p\u{1b}]52;x\u{7}dir");
    std::fs::create_dir(&weird).unwrap();
    let r = load_policy_file(&weird);
    match r {
        Err(e @ PermissionError::UnsafeFile { .. }) => {
            assert_no_control("Display", &e.to_string());
        }
        other => panic!("expected UnsafeFile, got {other:?}"),
    }
}

#[test]
fn invalid_context_display_has_no_control_chars() {
    let r = CompiledPolicy::compile(&ctx_at("rel\u{1b}]52;x\u{7}ative", None), &[], &[], &[]);
    match r {
        Err(e @ PermissionError::InvalidContext(_)) => assert_no_control("Display", &e.to_string()),
        other => panic!("expected InvalidContext, got {:?}", other.map(|_| ())),
    }
    let r = CompiledPolicy::compile(&ctx_at("/work/proj", Some("rel\u{7}home")), &[], &[], &[]);
    match r {
        Err(e @ PermissionError::InvalidContext(_)) => assert_no_control("Display", &e.to_string()),
        other => panic!("expected InvalidContext, got {:?}", other.map(|_| ())),
    }
}

// -- round 3: malformed host vs explicit rules --------------------------------

#[test]
fn explicit_deny_star_still_denies_malformed_host() {
    let p = user_only(&[PolicyRule::host("*", Deny)]);
    assert_verdict(
        &p.evaluate(&host("user@evil.com")),
        Deny,
        Some("user:1"),
        Some(Layer::User),
    );
}

#[test]
fn allow_star_only_leaves_malformed_host_at_ask() {
    let p = user_only(&[PolicyRule::host("*", Allow)]);
    assert_invalid_host(&p, "user@evil.com");
}

#[test]
fn ask_rule_matching_raw_malformed_host_loses_tie_to_invalid_host() {
    let p = user_only(&[PolicyRule::host("*", Ask)]);
    assert_invalid_host(&p, "user@evil.com");
}

#[test]
fn project_deny_star_tightens_malformed_host() {
    let p = compile_in(&ctx(), &[], &[], &[PolicyRule::host("*", Deny)]);
    assert_verdict(
        &p.evaluate(&host("user@evil.com")),
        Deny,
        Some("project:1"),
        Some(Layer::Project),
    );
}

// -- round 3: host character allow-list ---------------------------------------

#[test]
fn host_percent_and_comma_are_malformed() {
    let allow = [PolicyRule::host("*.pypi.org", Allow)];
    let p = compile_in(&ctx(), &allow, &allow, &[]);
    assert_invalid_host(&p, "evil.com%2f.pypi.org");
    assert_invalid_host(&p, "evil.com,.pypi.org");
    assert_invalid_host(&p, "evil.com;.pypi.org");
    assert_invalid_host(&p, "evil.com'.pypi.org");
    assert_invalid_host(&p, "evil.com(.pypi.org");
}

#[test]
fn host_underscore_is_allowed_and_not_malformed() {
    let p = user_only(&[PolicyRule::host("*.example", Allow)]);
    assert_verdict(
        &p.evaluate(&host("a_b.example")),
        Allow,
        Some("user:1"),
        Some(Layer::User),
    );
    let q = user_only(&[]);
    assert_verdict(&q.evaluate(&host("a_b.example")), Allow, None, None);
    let r = user_only(&[PolicyRule::host("a_b.example", Deny)]);
    assert_eq!(decision_of(&r, &host("A_B.example:80")), Deny);
}

// -- round 3: ancestors of the project path -----------------------------------

#[cfg(unix)]
#[test]
fn project_path_under_symlinked_hone_dir_is_unsafe_file_via_with_paths() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    write_file(
        elsewhere.path(),
        "permissions.toml",
        "[[rules]]\ncommand = \"ls\"\ndecision = \"deny\"\n",
    );
    std::os::unix::fs::symlink(elsewhere.path(), work.path().join(".hone")).unwrap();
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: Some(home.path().to_path_buf()),
    };
    let user = home.path().join("none.toml");
    let project = work.path().join(".hone").join("permissions.toml");
    let r = CompiledPolicy::load_hierarchical_with_paths(&c, &[], &user, &project);
    assert!(is_unsafe_file(&r), "{:?}", r.map(|_| ()));
}

#[cfg(unix)]
#[test]
fn project_path_under_deeper_symlinked_ancestor_is_unsafe_file() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(elsewhere.path().join(".hone")).unwrap();
    write_file(&elsewhere.path().join(".hone"), "permissions.toml", "");
    std::os::unix::fs::symlink(elsewhere.path(), work.path().join("sub")).unwrap();
    let c = PolicyContext {
        workdir: work.path().to_path_buf(),
        home: Some(home.path().to_path_buf()),
    };
    let project = work.path().join("sub/.hone/permissions.toml");
    let r = CompiledPolicy::load_hierarchical_with_paths(
        &c,
        &[],
        &home.path().join("none.toml"),
        &project,
    );
    assert!(is_unsafe_file(&r), "{:?}", r.map(|_| ()));
}
