#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.2 review round 2: security-review regression tests
//! (`docs/plans/policy-p2.2-spec.md` sections 3 and 4).
//!
//! Assumptions:
//! - Wrapper subjects: `sudo -u x git push` yields the inner `git push` AND a command
//!   whose argv[0] is `sudo` (the wrapper's own argv, joined).
//! - A glob/brace cap overflow is not required to set `unparsed`; only "no panic, bounded".
//! - `web_fetch` redirect output: the tool returns `Ok`, with the 3xx status and the
//!   `Location` somewhere in the JSON (field names are not pinned; we search the text).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use hone_config::policy::{PathOp, Subject};
use hone_sandbox::NullSandbox;
use hone_tools::{
    ShellParse, SubjectContext, Tool, ToolContext, ToolInput, WebFetchTool, WriteFileTool,
    parse_shell, subject_path,
};
use serde_json::json;
use tempfile::TempDir;

// -- helpers ------------------------------------------------------------------

struct Env {
    _wd: TempDir,
    _home: TempDir,
    sctx: SubjectContext,
}

impl Env {
    fn wd(&self) -> &Path {
        &self.sctx.workdir
    }
    fn home(&self) -> &Path {
        self.sctx.home.as_deref().unwrap()
    }
}

fn env() -> Env {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let sctx = SubjectContext {
        workdir: wd.path().canonicalize().unwrap(),
        home: Some(home.path().canonicalize().unwrap()),
        case_insensitive: false,
    };
    Env {
        _wd: wd,
        _home: home,
        sctx,
    }
}

fn parse(cmd: &str, e: &Env) -> ShellParse {
    parse_shell(cmd, &e.sctx)
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn has_cmd(p: &ShellParse, parts: &[&str]) -> bool {
    p.commands
        .iter()
        .any(|c| c.as_slice() == argv(parts).as_slice())
}

fn op_char(op: &PathOp) -> char {
    match op {
        PathOp::Read => 'R',
        PathOp::Write => 'W',
    }
}

fn pp(p: &ShellParse) -> Vec<String> {
    p.paths
        .iter()
        .map(|(op, path)| format!("{}:{}", op_char(op), path.display()))
        .collect()
}

fn r(p: &Path) -> String {
    format!("R:{}", p.display())
}

fn make_ctx(dir: &Path) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        working_dir: dir.to_path_buf(),
    }
}

/// Run `f` on a thread and fail instead of hanging.
fn with_timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("operation hung")
}

// -- B4: dangling symlinks ----------------------------------------------------

#[cfg(unix)]
mod dangling {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn dangling_symlink_leaf_resolves_to_its_target() {
        let e = env();
        let dir = e.home().join(".ssh");
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("authorized_keys");
        symlink(&target, e.wd().join("notes.md")).unwrap();
        assert!(!target.exists());
        assert_eq!(subject_path("notes.md", &e.sctx), target);
    }

    #[test]
    fn dangling_symlink_into_a_missing_directory_resolves_under_existing_prefix() {
        let e = env();
        let target = e.home().join(".ssh").join("authorized_keys"); // .ssh does not exist
        symlink(&target, e.wd().join("notes.md")).unwrap();
        assert_eq!(subject_path("notes.md", &e.sctx), target);
    }

    #[test]
    fn dangling_symlink_with_relative_target_is_resolved_from_the_links_directory() {
        let e = env();
        std::fs::create_dir(e.wd().join("sub")).unwrap();
        symlink("inner/target.txt", e.wd().join("sub/link")).unwrap();
        assert_eq!(
            subject_path("sub/link", &e.sctx),
            e.wd().join("sub/inner/target.txt")
        );
    }

    #[test]
    fn dangling_symlink_as_an_intermediate_component_resolves_the_rest_under_target() {
        let e = env();
        let missing_dir = e.home().join("newdir");
        symlink(&missing_dir, e.wd().join("d")).unwrap();
        assert_eq!(subject_path("d/k.txt", &e.sctx), missing_dir.join("k.txt"));
    }

    #[test]
    fn chain_of_dangling_symlinks_is_followed_to_the_end() {
        let e = env();
        let end = e.home().join(".ssh").join("authorized_keys");
        symlink(&end, e.wd().join("b")).unwrap();
        symlink(e.wd().join("b"), e.wd().join("a")).unwrap();
        assert_eq!(subject_path("a", &e.sctx), end);
    }

    #[test]
    fn long_but_finite_chain_within_the_hop_limit_resolves() {
        let e = env();
        let end = e.home().join("end.txt");
        let mut prev = end.clone();
        for i in 0..20 {
            let l = e.wd().join(format!("l{i}"));
            symlink(&prev, &l).unwrap();
            prev = l;
        }
        assert_eq!(subject_path("l19", &e.sctx), end);
    }

    #[test]
    fn symlink_loop_does_not_hang_and_stays_in_the_workdir() {
        let e = env();
        symlink(e.wd().join("b"), e.wd().join("a")).unwrap();
        symlink(e.wd().join("a"), e.wd().join("b")).unwrap();
        let sctx = e.sctx.clone();
        let out = with_timeout(move || subject_path("a", &sctx));
        assert!(out.starts_with(e.wd()), "{out:?}");
    }

    #[test]
    fn self_referential_symlink_does_not_hang() {
        let e = env();
        symlink(e.wd().join("me"), e.wd().join("me")).unwrap();
        let sctx = e.sctx.clone();
        let out = with_timeout(move || subject_path("me", &sctx));
        assert!(out.starts_with(e.wd()), "{out:?}");
    }

    #[test]
    fn write_file_through_dangling_symlink_outside_workdir_is_refused() {
        let e = env();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().canonicalize().unwrap().join("created.txt");
        symlink(&target, e.wd().join("notes.md")).unwrap();
        let ctx = make_ctx(e.wd());
        let res = with_timeout({
            let ctx = ctx.clone();
            move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                rt.block_on(WriteFileTool.call(
                    ToolInput::new(json!({"path": "notes.md", "content": "pwned"})),
                    &ctx,
                ))
                .map(|_| ())
                .map_err(|e| e.to_string())
            }
        });
        assert!(res.is_err(), "write through dangling link must fail");
        assert!(
            !target.exists(),
            "nothing may be created outside the workdir"
        );
    }

    #[test]
    fn write_file_through_dangling_directory_symlink_outside_workdir_is_refused() {
        let e = env();
        let outside = tempfile::tempdir().unwrap();
        let missing = outside.path().canonicalize().unwrap().join("newdir");
        symlink(&missing, e.wd().join("d")).unwrap();
        let ctx = make_ctx(e.wd());
        let res = with_timeout({
            let ctx = ctx.clone();
            move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                rt.block_on(WriteFileTool.call(
                    ToolInput::new(json!({"path": "d/f.txt", "content": "x"})),
                    &ctx,
                ))
                .map(|_| ())
                .map_err(|e| e.to_string())
            }
        });
        assert!(res.is_err());
        assert!(
            !missing.exists(),
            "no directory may be created outside the workdir"
        );
    }

    #[test]
    fn write_file_through_dangling_symlink_inside_workdir_still_works() {
        let e = env();
        let target = e.wd().join("real-target.txt");
        symlink(&target, e.wd().join("alias.txt")).unwrap();
        let ctx = make_ctx(e.wd());
        let res = with_timeout({
            let ctx = ctx.clone();
            move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                rt.block_on(WriteFileTool.call(
                    ToolInput::new(json!({"path": "alias.txt", "content": "ok"})),
                    &ctx,
                ))
                .map(|_| ())
                .map_err(|e| e.to_string())
            }
        });
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "ok");
    }

    #[test]
    fn write_file_subject_for_dangling_link_is_the_target() {
        let e = env();
        let dir = e.home().join(".ssh");
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("authorized_keys");
        symlink(&target, e.wd().join("notes.md")).unwrap();
        let ts = WriteFileTool.subjects(
            &ToolInput::new(json!({"path": "notes.md", "content": "x"})),
            &e.sctx,
        );
        assert!(
            ts.subjects
                .iter()
                .any(|s| matches!(s, Subject::Path { path, op: PathOp::Write } if *path == target)),
            "{:?}",
            ts.subjects
        );
    }
}

// -- S4: reserved words and grouping ------------------------------------------

#[test]
fn reserved_words_and_grouping_are_stripped_before_argv0() {
    let e = env();
    for src in [
        "{ git push; }",
        "if true; then git push; fi",
        "if true; then true; else git push; fi",
        "if false; then true; elif true; then git push; fi",
        "! git push",
        "time git push",
        "while true; do git push; done",
        "until false; do git push; done",
        "function g { git push; }",
        "function g() { git push; }",
        "g() { git push; }",
        "( git push )",
        "(git push)",
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
        assert!(!p.unparsed, "{src}");
        for c in &p.commands {
            assert!(
                ![
                    "{", "}", "(", ")", "!", "if", "then", "else", "elif", "fi", "while", "until",
                    "do", "done", "time", "function"
                ]
                .contains(&c[0].as_str()),
                "{src}: reserved word leaked as argv[0]: {:?}",
                p.commands
            );
        }
    }
}

// -- S4: wrappers -------------------------------------------------------------

#[test]
fn wrappers_forward_to_the_wrapped_command_and_keep_the_wrapper_subject() {
    let e = env();
    for (src, wrapper) in [
        ("env A=1 git push", "env"),
        ("env -i git push", "env"),
        ("env -u X git push", "env"),
        ("command git push", "command"),
        ("exec git push", "exec"),
        ("nohup git push", "nohup"),
        ("sudo git push", "sudo"),
        ("sudo -u x git push", "sudo"),
        ("doas git push", "doas"),
        ("timeout 5 git push", "timeout"),
        ("nice git push", "nice"),
        ("nice -n 5 git push", "nice"),
        ("xargs git push", "xargs"),
        ("xargs -0 git push", "xargs"),
        ("stdbuf -oL git push", "stdbuf"),
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
        assert!(
            p.commands.iter().any(|c| c[0] == wrapper),
            "{src}: wrapper subject missing: {:?}",
            p.commands
        );
        assert!(!p.unparsed, "{src}");
    }
}

#[test]
fn sudo_wrapper_subject_is_its_own_argv() {
    let e = env();
    let p = parse("sudo git push", &e);
    assert!(has_cmd(&p, &["sudo", "git", "push"]), "{:?}", p.commands);
    assert!(has_cmd(&p, &["git", "push"]), "{:?}", p.commands);
}

#[test]
fn stacked_wrappers_all_unwrap() {
    let e = env();
    let p = parse("sudo env A=1 nice -n 5 timeout 9 git push", &e);
    assert!(has_cmd(&p, &["git", "push"]), "{:?}", p.commands);
    for w in ["sudo", "env", "nice", "timeout"] {
        assert!(
            p.commands.iter().any(|c| c[0] == w),
            "{w}: {:?}",
            p.commands
        );
    }
}

#[test]
fn wrapper_with_nothing_to_wrap_does_not_panic() {
    let e = env();
    for src in [
        "sudo", "env", "timeout", "nice", "xargs", "env A=1", "sudo -u",
    ] {
        let _ = parse(src, &e);
    }
}

// -- S4: command strings ------------------------------------------------------

#[test]
fn shell_dash_c_strings_are_parsed_recursively() {
    let e = env();
    for src in [
        "sh -c 'git push'",
        "bash -c 'git push'",
        "bash -lc \"git push\"",
        "zsh -c 'git push'",
        "dash -c 'git push'",
        "bash -c 'ls; git push'",
        "sh -c \"echo hi && git push\"",
        "/bin/bash -c 'git push'",
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
        assert!(!p.unparsed, "{src}");
    }
}

#[test]
fn eval_arguments_are_parsed_recursively() {
    let e = env();
    for src in ["eval 'git push'", "eval git push", "eval \"git\" \"push\""] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
    }
}

#[test]
fn command_string_paths_are_extracted_too() {
    let e = env();
    let p = parse("bash -c 'cat ~/.ssh/k'", &e);
    assert!(
        pp(&p).contains(&r(&e.home().join(".ssh/k"))),
        "{:?}",
        pp(&p)
    );
}

#[test]
fn unterminated_command_string_is_unparsed() {
    let e = env();
    assert!(parse("sh -c 'echo \"x'", &e).unparsed);
}

#[test]
fn dash_c_nesting_counts_against_the_depth_limit() {
    let e = env();
    // sh -c 'sh -c "sh -c ..."' nested 12 deep must not recurse unboundedly.
    let mut s = String::from("git push");
    for _ in 0..12 {
        s = format!("sh -c '{}'", s.replace('\'', "'\\''"));
    }
    let p = with_timeout({
        let sctx = e.sctx.clone();
        move || parse_shell(&s, &sctx).unparsed
    });
    assert!(p, "deeply nested command strings must be unparsed");
}

// -- S4: command names built at run time --------------------------------------

#[test]
fn runtime_built_command_names_are_unparsed() {
    let e = env();
    for src in [
        "git${IFS}push",
        "G=git; $G push",
        "$(echo git) push",
        "`echo git` push",
        "{git,} push",
        "${X} push",
        "\"$G\" push",
        "$G",
    ] {
        assert!(parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn expansion_in_arguments_is_not_unparsed() {
    let e = env();
    for src in [
        "echo $FOO",
        "echo $(date)",
        "echo `date`",
        "ls {a,b}",
        "git commit -m \"$MSG\"",
    ] {
        assert!(!parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn quoted_dollar_in_command_name_is_literal_and_not_unparsed() {
    let e = env();
    assert!(!parse("'$G' push", &e).unparsed);
}

// -- S4: arithmetic -----------------------------------------------------------

#[test]
fn arithmetic_shift_is_not_a_heredoc() {
    let e = env();
    for src in [
        "echo $((1<<2))\ngit push\n2",
        "echo $(( 1 << 2 ))\ngit push\n2",
        "((x<<1))\ngit push\n2",
        "(( x << 1 ))\ngit push",
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src:?}: {:?}", p.commands);
        assert!(!p.unparsed, "{src:?}");
    }
}

#[test]
fn real_heredoc_after_arithmetic_is_still_skipped() {
    let e = env();
    let p = parse("echo $((1<<2)); cat <<EOF\nrm -rf x\nEOF\nls", &e);
    assert!(!p.commands.iter().any(|c| c[0] == "rm"), "{:?}", p.commands);
    assert!(has_cmd(&p, &["ls"]));
}

// -- S4: quoted $HOME ---------------------------------------------------------

#[test]
fn quoted_home_expands_at_start_of_word_text() {
    let e = env();
    let want = r(&e.home().join(".ssh/k"));
    for src in [
        "cat \"$HOME/.ssh/k\"",
        "cat \"$HOME\"/.ssh/k",
        "cat \"${HOME}/.ssh/k\"",
        "cat \"${HOME}\"/.ssh/k",
        "cat $HOME/.ssh/k",
    ] {
        assert_eq!(pp(&parse(src, &e)), vec![want.clone()], "{src}");
    }
}

// -- S4: globs and braces -----------------------------------------------------

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "x").unwrap();
}

#[test]
fn glob_expands_to_existing_matches_plus_directory_prefix() {
    let e = env();
    let dir = e.home().join(".ssh");
    touch(&dir.join("id_a"));
    touch(&dir.join("id_b"));
    touch(&dir.join("other"));
    let p = parse(&format!("cat {}/id_*", dir.display()), &e);
    let out = pp(&p);
    assert!(out.contains(&r(&dir.join("id_a"))), "{out:?}");
    assert!(out.contains(&r(&dir.join("id_b"))), "{out:?}");
    assert!(
        out.contains(&r(&dir)),
        "directory prefix subject missing: {out:?}"
    );
    assert!(!out.contains(&r(&dir.join("other"))), "{out:?}");
}

#[test]
fn glob_with_zero_matches_still_yields_the_directory_prefix() {
    let e = env();
    let dir = e.home().join(".ssh");
    std::fs::create_dir_all(&dir).unwrap();
    let p = parse(&format!("cat {}/zzz_*", dir.display()), &e);
    assert_eq!(pp(&p), vec![r(&dir)]);
}

#[test]
fn glob_in_home_relative_word_uses_the_expanded_prefix() {
    let e = env();
    touch(&e.home().join(".ssh/id_x"));
    let p = parse("cat ~/.ssh/id_*", &e);
    let out = pp(&p);
    assert!(out.contains(&r(&e.home().join(".ssh/id_x"))), "{out:?}");
    assert!(out.contains(&r(&e.home().join(".ssh"))), "{out:?}");
}

#[test]
fn question_mark_glob_matches_dotfile_with_literal_dot() {
    let e = env();
    touch(&e.wd().join(".env"));
    touch(&e.wd().join(".envrc2"));
    let p = parse("cat .en?", &e);
    let out = pp(&p);
    assert!(out.contains(&r(&e.wd().join(".env"))), "{out:?}");
    assert!(!out.contains(&r(&e.wd().join(".envrc2"))), "{out:?}");
}

#[test]
fn character_class_glob_matches() {
    let e = env();
    touch(&e.wd().join("a1.txt"));
    touch(&e.wd().join("b1.txt"));
    touch(&e.wd().join("c1.txt"));
    let out = pp(&parse("cat [ab]1.txt", &e));
    assert!(out.contains(&r(&e.wd().join("a1.txt"))), "{out:?}");
    assert!(out.contains(&r(&e.wd().join("b1.txt"))), "{out:?}");
    assert!(!out.contains(&r(&e.wd().join("c1.txt"))), "{out:?}");
}

#[test]
fn glob_star_does_not_cross_directories() {
    let e = env();
    touch(&e.wd().join("top.txt"));
    touch(&e.wd().join("sub/deep.txt"));
    let out = pp(&parse("cat *.txt", &e));
    assert!(out.contains(&r(&e.wd().join("top.txt"))), "{out:?}");
    assert!(!out.contains(&r(&e.wd().join("sub/deep.txt"))), "{out:?}");
}

#[test]
fn brace_expansion_yields_every_alternative() {
    let e = env();
    let out = pp(&parse("cat ~/{.ssh,x}/k", &e));
    assert!(out.contains(&r(&e.home().join(".ssh/k"))), "{out:?}");
    assert!(out.contains(&r(&e.home().join("x/k"))), "{out:?}");
}

#[test]
fn brace_expansion_runs_before_globbing() {
    let e = env();
    touch(&e.wd().join("a.rs"));
    touch(&e.wd().join("b.md"));
    touch(&e.wd().join("c.txt"));
    let out = pp(&parse("cat *.{rs,md}", &e));
    assert!(out.contains(&r(&e.wd().join("a.rs"))), "{out:?}");
    assert!(out.contains(&r(&e.wd().join("b.md"))), "{out:?}");
    assert!(!out.contains(&r(&e.wd().join("c.txt"))), "{out:?}");
}

#[test]
fn glob_matches_are_capped_at_256_without_panicking() {
    let e = env();
    let dir = e.wd().join("many");
    std::fs::create_dir(&dir).unwrap();
    for i in 0..300 {
        std::fs::write(dir.join(format!("f{i:03}")), "x").unwrap();
    }
    let p = parse("cat many/f*", &e);
    let under: usize = pp(&p)
        .iter()
        .filter(|s| s.starts_with(&format!("R:{}/f", dir.display())))
        .count();
    assert!(under >= 1 && under <= 256, "matches under dir: {under}");
    assert!(pp(&p).contains(&r(&dir)), "directory prefix must remain");
}

#[test]
fn brace_expansion_is_capped_at_64_words_without_panicking() {
    let e = env();
    let items: Vec<String> = (0..200).map(|i| format!("w{i}")).collect();
    let p = parse(&format!("cat {{{}}}", items.join(",")), &e);
    assert!(pp(&p).len() <= 65, "got {}", pp(&p).len());
    assert!(!pp(&p).is_empty());
    // exponential blow-up: 2^12 combinations
    let nested = format!("cat {}", "{a,b}".repeat(12));
    let p = with_timeout({
        let sctx = e.sctx.clone();
        move || parse_shell(&nested, &sctx)
    });
    assert!(pp(&p).len() <= 65, "got {}", pp(&p).len());
}

#[test]
fn glob_in_writer_command_gives_write_for_matches() {
    let e = env();
    touch(&e.wd().join("x1.log"));
    let out = pp(&parse("rm *.log", &e));
    assert!(
        out.contains(&format!("W:{}", e.wd().join("x1.log").display())),
        "{out:?}"
    );
}

// -- S4: cd tracking ----------------------------------------------------------

#[test]
fn cd_home_then_relative_path_yields_both_candidates() {
    let e = env();
    let out = pp(&parse("cd ~ && cat .ssh/k", &e));
    assert!(out.contains(&r(&e.home().join(".ssh/k"))), "{out:?}");
    assert!(out.contains(&r(&e.wd().join(".ssh/k"))), "{out:?}");
}

#[test]
fn cd_relative_dir_then_relative_path_yields_both_candidates() {
    let e = env();
    let out = pp(&parse("cd sub; cat f", &e));
    assert!(out.contains(&r(&e.wd().join("sub/f"))), "{out:?}");
    assert!(out.contains(&r(&e.wd().join("f"))), "{out:?}");
}

#[test]
fn cd_absolute_dir_then_relative_path() {
    let e = env();
    let out = pp(&parse(&format!("cd {} && cat f", e.home().display()), &e));
    assert!(out.contains(&r(&e.home().join("f"))), "{out:?}");
    assert!(out.contains(&r(&e.wd().join("f"))), "{out:?}");
}

#[test]
fn paths_before_cd_are_not_affected() {
    let e = env();
    let out = pp(&parse("cat f; cd sub", &e));
    assert!(out.contains(&r(&e.wd().join("f"))), "{out:?}");
    assert!(!out.contains(&r(&e.wd().join("sub/f"))), "{out:?}");
}

#[test]
fn absolute_paths_after_cd_are_not_duplicated_against_the_new_directory() {
    let e = env();
    let abs = e.home().join("abs.txt");
    let out = pp(&parse(&format!("cd sub; cat {}", abs.display()), &e));
    assert_eq!(
        out.iter().filter(|s| s.ends_with("abs.txt")).count(),
        1,
        "{out:?}"
    );
}

#[test]
fn cd_with_non_literal_target_does_not_panic() {
    let e = env();
    let _ = parse("cd $X && cat f", &e);
    let _ = parse("cd && cat f", &e);
}

// -- S3: web_fetch redirects --------------------------------------------------

fn read_request_path(stream: &mut TcpStream) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    text.lines()
        .next()?
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
}

/// Start a local server; `handler(path) -> raw HTTP response`. Counts requests.
fn serve(
    listener: TcpListener,
    hits: Arc<AtomicUsize>,
    handler: impl Fn(&str) -> String + Send + 'static,
) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(path) = read_request_path(&mut stream) else {
                continue;
            };
            hits.fetch_add(1, Ordering::SeqCst);
            let _ = stream.write_all(handler(&path).as_bytes());
            let _ = stream.flush();
        }
    });
}

fn http(status: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_host_redirect_is_not_followed_and_surfaces_status_and_location() {
    let a = TcpListener::bind("127.0.0.1:0").unwrap();
    let b = TcpListener::bind("127.0.0.1:0").unwrap();
    let (pa, pb) = (
        a.local_addr().unwrap().port(),
        b.local_addr().unwrap().port(),
    );
    let (hits_a, hits_b) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    // Different *host* (localhost vs 127.0.0.1), so the redirect crosses hosts.
    let location = format!("http://localhost:{pb}/secret");
    let loc = location.clone();
    serve(a, hits_a.clone(), move |_| {
        http("302 Found", &format!("Location: {loc}\r\n"), "")
    });
    serve(b, hits_b.clone(), |_| http("200 OK", "", "SECRET-BODY"));

    let dir = tempfile::tempdir().unwrap();
    let out = WebFetchTool::new()
        .call(
            ToolInput::new(json!({"url": format!("http://127.0.0.1:{pa}/start")})),
            &make_ctx(dir.path()),
        )
        .await
        .expect("a blocked redirect is a normal result, not an error");

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(hits_a.load(Ordering::SeqCst), 1);
    assert_eq!(
        hits_b.load(Ordering::SeqCst),
        0,
        "the other host must not receive a request"
    );
    let text = out.value.to_string();
    assert!(text.contains("302"), "status missing: {text}");
    assert!(text.contains(&location), "Location missing: {text}");
    assert!(!text.contains("SECRET-BODY"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_port_redirect_on_same_ip_counts_as_a_different_origin_only_if_host_differs() {
    // Same host (127.0.0.1), different port: the spec's rule is by *host*, so this IS followed.
    let a = TcpListener::bind("127.0.0.1:0").unwrap();
    let b = TcpListener::bind("127.0.0.1:0").unwrap();
    let (pa, pb) = (
        a.local_addr().unwrap().port(),
        b.local_addr().unwrap().port(),
    );
    let (hits_a, hits_b) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let loc = format!("http://127.0.0.1:{pb}/landing");
    serve(a, hits_a.clone(), move |_| {
        http("302 Found", &format!("Location: {loc}\r\n"), "")
    });
    serve(b, hits_b.clone(), |_| {
        http("200 OK", "", "<html><body><p>LANDED</p></body></html>")
    });

    let dir = tempfile::tempdir().unwrap();
    let out = WebFetchTool::new()
        .call(
            ToolInput::new(json!({"url": format!("http://127.0.0.1:{pa}/start")})),
            &make_ctx(dir.path()),
        )
        .await
        .unwrap();
    assert_eq!(hits_b.load(Ordering::SeqCst), 1);
    assert!(out.value.to_string().contains("LANDED"));
}

#[tokio::test(flavor = "multi_thread")]
async fn same_host_redirect_is_followed() {
    let a = TcpListener::bind("127.0.0.1:0").unwrap();
    let pa = a.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    serve(a, hits.clone(), |path| match path {
        "/start" => http("302 Found", "Location: /next\r\n", ""),
        _ => http("200 OK", "", "<html><body><p>ARRIVED</p></body></html>"),
    });
    let dir = tempfile::tempdir().unwrap();
    let out = WebFetchTool::new()
        .call(
            ToolInput::new(json!({"url": format!("http://127.0.0.1:{pa}/start")})),
            &make_ctx(dir.path()),
        )
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    let text = out.value.to_string();
    assert!(text.contains("ARRIVED"), "{text}");
    assert!(text.contains("200"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn redirect_loop_on_the_same_host_terminates() {
    let a = TcpListener::bind("127.0.0.1:0").unwrap();
    let pa = a.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    serve(a, hits.clone(), |_| {
        http("302 Found", "Location: /again\r\n", "")
    });
    let dir = tempfile::tempdir().unwrap();
    let url = format!("http://127.0.0.1:{pa}/start");
    let ctx = make_ctx(dir.path());
    let res = tokio::time::timeout(
        Duration::from_secs(20),
        WebFetchTool::new().call(ToolInput::new(json!({"url": url})), &ctx),
    )
    .await;
    assert!(res.is_ok(), "redirect loop must terminate");
    assert!(hits.load(Ordering::SeqCst) <= 10);
}

// -- round 3: caps fail closed --------------------------------------------------

#[test]
fn directory_with_more_than_4096_entries_makes_a_glob_unparsed() {
    let e = env();
    let dir = e.wd().join("many");
    std::fs::create_dir(&dir).unwrap();
    for i in 0..4200 {
        std::fs::write(dir.join(format!("e{i:04}")), "").unwrap();
    }
    let p = with_timeout({
        let sctx = e.sctx.clone();
        move || parse_shell("cat many/*", &sctx)
    });
    assert!(p.unparsed, "target may lie past the directory read cap");
}

#[test]
fn directory_just_under_the_read_cap_is_still_parsed_when_matches_fit() {
    let e = env();
    let dir = e.wd().join("fewer");
    std::fs::create_dir(&dir).unwrap();
    for i in 0..20 {
        std::fs::write(dir.join(format!("e{i:02}")), "").unwrap();
    }
    assert!(!parse("cat fewer/*", &e).unparsed);
}

#[test]
fn more_than_256_glob_matches_make_the_line_unparsed() {
    let e = env();
    let dir = e.wd().join("matches");
    std::fs::create_dir(&dir).unwrap();
    for i in 0..300 {
        std::fs::write(dir.join(format!("f{i:03}")), "").unwrap();
    }
    assert!(parse("cat matches/f*", &e).unparsed);
}

#[test]
fn brace_word_with_more_than_64_alternatives_is_unparsed() {
    let e = env();
    let mut items: Vec<String> = (0..70).map(|i| format!("sub/a{i:02}")).collect();
    items.push("sub/.env".to_string());
    let p = parse(&format!("cat {{{}}}", items.join(",")), &e);
    assert!(
        p.unparsed,
        "the alternative that was cut off could be the one that matters"
    );
}

#[test]
fn brace_word_within_the_cap_is_parsed() {
    let e = env();
    let items: Vec<String> = (0..60).map(|i| format!("sub/a{i:02}")).collect();
    assert!(!parse(&format!("cat {{{}}}", items.join(",")), &e).unparsed);
}

// -- round 3: shell state the parser cannot model ---------------------------------

#[test]
fn glob_changing_shell_state_makes_the_line_unparsed() {
    let e = env();
    for src in [
        "shopt -s dotglob; cat *",
        "setopt globdots",
        "shopt -s globstar",
        "set -o globdots",
        "GLOBIGNORE=x cat *",
        "export GLOBIGNORE='*'; cat *",
        "CDPATH=~ cd .cred && cat k",
        "ls && shopt -s nullglob",
    ] {
        assert!(parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn unrelated_set_and_export_are_not_unparsed() {
    let e = env();
    for src in ["set -e", "export FOO=1", "FOO=1 ls"] {
        assert!(!parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn shells_reading_commands_from_stdin_are_unparsed() {
    let e = env();
    for src in [
        "echo 'git push' | sh",
        "bash <<< 'git push'",
        "sh -s",
        "zsh",
        "dash",
        "ksh",
        "cat script | bash",
    ] {
        assert!(parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn shells_with_c_or_a_script_argument_are_not_unparsed() {
    let e = env();
    for src in [
        "bash script.sh",
        "sh -c 'x'",
        "sh ./run.sh arg",
        "bash -c 'ls'",
    ] {
        assert!(!parse(src, &e).unparsed, "{src}");
    }
}

// -- round 3: pushd ----------------------------------------------------------------

#[test]
fn pushd_is_tracked_like_cd() {
    let e = env();
    let out = pp(&parse("pushd ~ && cat .cred/k", &e));
    assert!(out.contains(&r(&e.home().join(".cred/k"))), "{out:?}");
    assert!(out.contains(&r(&e.wd().join(".cred/k"))), "{out:?}");
}

#[test]
fn pushd_relative_directory_yields_both_candidates() {
    let e = env();
    let out = pp(&parse("pushd sub; cat f", &e));
    assert!(out.contains(&r(&e.wd().join("sub/f"))), "{out:?}");
    assert!(out.contains(&r(&e.wd().join("f"))), "{out:?}");
}

// -- round 3: more wrappers ---------------------------------------------------------

#[test]
fn more_wrappers_forward_to_the_wrapped_command() {
    let e = env();
    for (src, wrapper) in [
        ("time -p git push", "time"),
        ("/usr/bin/time git push", "time"),
        ("arch -arm64 git push", "arch"),
        ("caffeinate -i git push", "caffeinate"),
        ("caffeinate git push", "caffeinate"),
        ("setsid git push", "setsid"),
        ("coproc git push", "coproc"),
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
        assert!(
            p.commands.iter().any(|c| c[0] == wrapper),
            "{src}: wrapper subject missing: {:?}",
            p.commands
        );
        assert!(!p.unparsed, "{src}");
    }
}

#[test]
fn su_dash_c_string_is_parsed_recursively() {
    let e = env();
    let p = parse("su -c 'git push'", &e);
    assert!(has_cmd(&p, &["git", "push"]), "{:?}", p.commands);
    let p = parse("su root -c 'git push'", &e);
    assert!(has_cmd(&p, &["git", "push"]), "{:?}", p.commands);
}

#[test]
fn shell_option_arguments_are_skipped_when_looking_for_dash_c() {
    let e = env();
    for src in [
        "bash -o pipefail -c 'git push'",
        "sh -o errexit -c 'git push'",
        "bash -O extglob -c 'git push'",
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
        assert!(!p.unparsed, "{src}");
    }
}

#[test]
fn more_than_eight_nested_wrappers_are_unparsed() {
    let e = env();
    let src = format!("{}git push", "nohup ".repeat(12));
    assert!(parse(&src, &e).unparsed);
    let ok = format!("{}git push", "nohup ".repeat(3));
    assert!(!parse(&ok, &e).unparsed);
}

// -- round 4 ------------------------------------------------------------------------

#[test]
fn noglob_only_narrows_globbing_and_is_not_unparsed() {
    let e = env();
    for src in ["set -o noglob", "set +o noglob; cat *", "set -f"] {
        assert!(!parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn bundled_o_option_consumes_the_next_word_when_scanning_for_dash_c() {
    let e = env();
    for src in [
        "bash -euo pipefail -c 'git push'",
        "bash -eo pipefail -c 'git push'",
        "bash -xo pipefail -c 'git push'",
        "sh -euo pipefail -c 'git push'",
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
        assert!(!p.unparsed, "{src}");
    }
}

#[test]
fn bundled_o_option_command_string_paths_are_extracted() {
    let e = env();
    let p = parse("sh -euo pipefail -c 'cat ~/.cred/k'", &e);
    assert!(
        pp(&p).contains(&r(&e.home().join(".cred/k"))),
        "{:?}",
        pp(&p)
    );
    assert!(!p.unparsed);
}

#[test]
fn common_set_forms_and_shell_info_flags_are_not_unparsed() {
    let e = env();
    for src in [
        "set -euo pipefail; cargo test",
        "set -o pipefail; ls | tail",
        "set -e",
        "set -u",
        "bash --version",
        "sh --help",
        "zsh --version",
    ] {
        assert!(!parse(src, &e).unparsed, "{src}");
    }
    let p = parse("set -euo pipefail; cargo test", &e);
    assert!(has_cmd(&p, &["cargo", "test"]), "{:?}", p.commands);
}

#[test]
fn widening_glob_options_are_still_unparsed() {
    let e = env();
    for src in [
        "shopt -s dotglob; cat *",
        "shopt -s globstar",
        "shopt -s extglob",
        "shopt -s nocaseglob",
        "shopt -s failglob",
        "setopt globdots",
        "setopt extendedglob",
        "set -o globdots",
        "GLOBIGNORE=x cat *",
    ] {
        assert!(parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn su_long_command_option_is_parsed_recursively() {
    let e = env();
    for src in [
        "su --command='git push'",
        "su --command 'git push'",
        "su root --command='git push'",
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
    }
}

#[test]
fn env_split_string_is_parsed_recursively() {
    let e = env();
    for src in [
        "env -S 'git push'",
        "env --split-string='git push'",
        "env --split-string 'git push'",
        "env -i -S 'git push'",
    ] {
        let p = parse(src, &e);
        assert!(has_cmd(&p, &["git", "push"]), "{src}: {:?}", p.commands);
    }
}

#[allow(dead_code)]
fn _unused(_: PathBuf) {}
