#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Contract tests for `Tool::subjects()`, the shell parser and `subject_path`
//! (`docs/plans/policy-p2.2-spec.md` sections 3 and 4).
//!
//! Assumptions:
//! - `ToolSubjects: Default + Debug`, `ShellParse: Debug`, `SubjectContext: Clone`.
//! - `Tool::subjects(&self, &ToolInput, &SubjectContext) -> ToolSubjects`.
//! - Subjects are compared as sorted sets of strings (see `view`), so emission order
//!   among a tool's subjects is not pinned; `ShellParse::paths` order IS pinned
//!   ("in source order").
//! - `git_checkout` really reads `branch`; the spec text says `name`. Tests pass both
//!   keys with the same value so either reading satisfies them.
//! - `$HOME` expansion is exercised through `parse_shell` only (spec: "only in shell words").

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hone_config::policy::{PathOp, Subject};
use hone_tools::lsp::{LspCodeActionTool, LspDiagnosticsTool, LspHoverTool, MockLspBridge};
use hone_tools::{
    ComputerControlTool, EditFileTool, GitBranchTool, GitCheckoutTool, GitCommitTool, GitDiffTool,
    GitRedoTool, GitStatusTool, GitUndoTool, GlobTool, GrepTool, MultiEditTool, PatchTool,
    ReadFileTool, ScreenCaptureTool, ShellParse, ShellTool, SubjectContext, Tool, ToolInput,
    ToolSubjects, WebFetchTool, WebSearchTool, WriteFileTool, parse_shell, subject_path,
};
use serde_json::{Value, json};
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
    fn with_case_insensitive(mut self, v: bool) -> Self {
        self.sctx.case_insensitive = v;
        self
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

fn op_char(op: &PathOp) -> char {
    match op {
        PathOp::Read => 'R',
        PathOp::Write => 'W',
    }
}

fn view_subject(s: &Subject) -> String {
    match s {
        Subject::Command(c) => format!("C:{c}"),
        Subject::Path { path, op } => format!("{}:{}", op_char(op), path.display()),
        Subject::Host(h) => format!("H:{h}"),
    }
}

/// Sorted, de-duplicated string view of a tool's subjects.
fn view(ts: &ToolSubjects) -> Vec<String> {
    let mut v: Vec<String> = ts.subjects.iter().map(view_subject).collect();
    v.sort();
    v.dedup();
    v
}

fn expect(items: &[String]) -> Vec<String> {
    let mut v = items.to_vec();
    v.sort();
    v.dedup();
    v
}

fn r(p: &Path) -> String {
    format!("R:{}", p.display())
}
fn w(p: &Path) -> String {
    format!("W:{}", p.display())
}

fn subj(tool: &dyn Tool, input: Value, e: &Env) -> ToolSubjects {
    tool.subjects(&ToolInput::new(input), &e.sctx)
}

fn argvs(p: &ShellParse) -> Vec<Vec<String>> {
    p.commands.clone()
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn pp(p: &ShellParse) -> Vec<String> {
    p.paths
        .iter()
        .map(|(op, path)| format!("{}:{}", op_char(op), path.display()))
        .collect()
}

fn parse(cmd: &str, e: &Env) -> ShellParse {
    parse_shell(cmd, &e.sctx)
}

fn has_cmd(p: &ShellParse, parts: &[&str]) -> bool {
    p.commands
        .iter()
        .any(|c| c.as_slice() == argv(parts).as_slice())
}

// -- shell: splitting ---------------------------------------------------------

#[test]
fn shell_splits_on_all_separators_outside_quotes() {
    let e = env();
    let p = parse("a1; b1 && c1 || d1 | e1 & f1\ng1", &e);
    let heads: Vec<&str> = p.commands.iter().map(|c| c[0].as_str()).collect();
    assert_eq!(heads, ["a1", "b1", "c1", "d1", "e1", "f1", "g1"]);
    assert!(!p.unparsed);
}

#[test]
fn shell_separators_inside_quotes_do_not_split() {
    let e = env();
    assert_eq!(
        argvs(&parse("echo \"a;b && c\"", &e)),
        vec![argv(&["echo", "a;b && c"])]
    );
    assert_eq!(
        argvs(&parse("echo 'x | y\nz'", &e)),
        vec![argv(&["echo", "x | y\nz"])]
    );
}

#[test]
fn shell_escaped_separator_does_not_split() {
    let e = env();
    assert_eq!(
        argvs(&parse("echo a\\;b", &e)),
        vec![argv(&["echo", "a;b"])]
    );
}

#[test]
fn shell_empty_and_whitespace_input_has_no_commands() {
    let e = env();
    for s in ["", "   ", "\n\n", ";;"] {
        let p = parse(s, &e);
        assert!(p.commands.is_empty(), "{s:?}");
        assert!(!p.unparsed, "{s:?}");
    }
}

// -- shell: words -------------------------------------------------------------

#[test]
fn shell_adjacent_quoted_parts_join_into_one_word() {
    let e = env();
    assert_eq!(
        argvs(&parse("g''it push", &e)),
        vec![argv(&["git", "push"])]
    );
    assert_eq!(
        argvs(&parse("\"git\" push", &e)),
        vec![argv(&["git", "push"])]
    );
    assert_eq!(
        argvs(&parse("g\"\"i't' push", &e)),
        vec![argv(&["git", "push"])]
    );
    assert_eq!(
        argvs(&parse("'gi't push", &e)),
        vec![argv(&["git", "push"])]
    );
}

#[test]
fn shell_backslash_newline_is_joined_before_splitting() {
    let e = env();
    assert_eq!(
        argvs(&parse("git\\\n push", &e)),
        vec![argv(&["git", "push"])]
    );
    assert_eq!(
        argvs(&parse("gi\\\nt push", &e)),
        vec![argv(&["git", "push"])]
    );
}

#[test]
fn shell_single_quotes_are_literal() {
    let e = env();
    assert_eq!(
        argvs(&parse("echo '\\n $x \"q\"'", &e)),
        vec![argv(&["echo", "\\n $x \"q\""])]
    );
}

#[test]
fn shell_double_quote_escapes() {
    let e = env();
    assert_eq!(
        argvs(&parse("echo \"a\\\"b\"", &e)),
        vec![argv(&["echo", "a\"b"])]
    );
    assert_eq!(
        argvs(&parse("echo \"a\\\\b\"", &e)),
        vec![argv(&["echo", "a\\b"])]
    );
    assert_eq!(
        argvs(&parse("echo \"a\\$b\"", &e)),
        vec![argv(&["echo", "a$b"])]
    );
}

#[test]
fn shell_backslash_escapes_outside_quotes() {
    let e = env();
    assert_eq!(
        argvs(&parse("ls my\\ file", &e)),
        vec![argv(&["ls", "my file"])]
    );
}

#[test]
fn shell_dollar_single_quote_standard_escapes() {
    let e = env();
    assert_eq!(
        argvs(&parse("echo $'a\\tb'", &e)),
        vec![argv(&["echo", "a\tb"])]
    );
    assert_eq!(
        argvs(&parse("echo $'a\\nb'", &e)),
        vec![argv(&["echo", "a\nb"])]
    );
    assert_eq!(
        argvs(&parse("echo $'it\\'s'", &e)),
        vec![argv(&["echo", "it's"])]
    );
}

#[test]
fn shell_leading_assignments_are_dropped() {
    let e = env();
    assert_eq!(
        argvs(&parse("FOO=1 BAR='x y' git push", &e)),
        vec![argv(&["git", "push"])]
    );
}

#[test]
fn shell_assignment_looking_word_after_argv0_is_kept() {
    let e = env();
    assert_eq!(
        argvs(&parse("ls FOO=1", &e)),
        vec![argv(&["ls", "FOO=1"])]
    );
}

#[test]
fn shell_env_wrapper_is_unwrapped_and_kept_as_its_own_subject() {
    let e = env();
    assert_eq!(
        argvs(&parse("env FOO=1 ls", &e)),
        vec![argv(&["env", "FOO=1", "ls"]), argv(&["ls"])]
    );
}

// -- shell: argv[0] normalisation --------------------------------------------

#[test]
fn shell_argv0_is_reduced_to_basename() {
    let e = env();
    assert_eq!(
        argvs(&parse("/usr/bin/git push", &e)),
        vec![argv(&["git", "push"])]
    );
    assert_eq!(
        argvs(&parse("./tools/run.sh a", &e)),
        vec![argv(&["run.sh", "a"])]
    );
}

#[test]
fn shell_argv0_case_is_preserved_unless_case_insensitive() {
    let e = env();
    assert_eq!(argvs(&parse("GIT push", &e)), vec![argv(&["GIT", "push"])]);
    let e = env().with_case_insensitive(true);
    assert_eq!(argvs(&parse("GIT Push", &e)), vec![argv(&["git", "Push"])]);
    assert_eq!(
        argvs(&parse("/usr/bin/GIT push", &e)),
        vec![argv(&["git", "push"])]
    );
}

#[test]
fn shell_command_subject_collapses_whitespace() {
    let e = env();
    let ts = parse("git   push \t origin", &e).into_subjects();
    assert_eq!(
        view(&ts)
            .iter()
            .filter(|s| s.starts_with("C:"))
            .cloned()
            .collect::<Vec<_>>(),
        ["C:git push origin"]
    );
}

#[test]
fn shell_each_simple_command_gives_one_command_subject() {
    let e = env();
    let ts = parse("ls; git push && rm x", &e).into_subjects();
    let cmds: Vec<String> = view(&ts)
        .into_iter()
        .filter(|s| s.starts_with("C:"))
        .collect();
    assert_eq!(cmds, ["C:git push", "C:ls", "C:rm x"]);
}

#[test]
fn shell_into_subjects_carries_unparsed_flag_and_paths() {
    let e = env();
    let ts = parse("cat a.txt; echo 'oops", &e).into_subjects();
    assert!(ts.unparsed);
    assert!(view(&ts).contains(&"C:cat a.txt".to_string()));
    assert!(view(&ts).contains(&r(&e.wd().join("a.txt"))));
    assert!(!parse("ls", &e).into_subjects().unparsed);
}

// -- shell: substitution ------------------------------------------------------

#[test]
fn shell_dollar_paren_inner_commands_become_subjects() {
    let e = env();
    let p = parse("echo $(git push)", &e);
    assert!(has_cmd(&p, &["git", "push"]), "{:?}", p.commands);
    assert!(p.commands.iter().any(|c| c[0] == "echo"));
}

#[test]
fn shell_backtick_inner_commands_become_subjects() {
    let e = env();
    let p = parse("echo `rm -rf build`", &e);
    assert!(has_cmd(&p, &["rm", "-rf", "build"]), "{:?}", p.commands);
}

#[test]
fn shell_substitution_inside_double_quotes_is_parsed() {
    let e = env();
    let p = parse("echo \"x $(git push) y\"", &e);
    assert!(has_cmd(&p, &["git", "push"]), "{:?}", p.commands);
}

#[test]
fn shell_substitution_inside_single_quotes_is_not_parsed() {
    let e = env();
    let p = parse("echo '$(git push)'", &e);
    assert!(!has_cmd(&p, &["git", "push"]), "{:?}", p.commands);
    assert!(!p.unparsed);
}

#[test]
fn shell_nested_substitution_is_parsed() {
    let e = env();
    let p = parse("echo $(echo $(git push))", &e);
    assert!(has_cmd(&p, &["git", "push"]), "{:?}", p.commands);
    assert!(!p.unparsed);
}

#[test]
fn shell_substitution_with_separators_inside_yields_each_command() {
    let e = env();
    let p = parse("echo $(ls; rm x)", &e);
    assert!(
        has_cmd(&p, &["ls"]) && has_cmd(&p, &["rm", "x"]),
        "{:?}",
        p.commands
    );
}

fn nested(depth: usize) -> String {
    format!("{}git push{}", "echo $(".repeat(depth), ")".repeat(depth))
}

#[test]
fn shell_nesting_up_to_depth_8_is_parsed() {
    let e = env();
    let p = parse(&nested(8), &e);
    assert!(!p.unparsed);
    assert!(has_cmd(&p, &["git", "push"]));
}

#[test]
fn shell_nesting_deeper_than_8_is_unparsed() {
    let e = env();
    for d in [10, 30] {
        assert!(parse(&nested(d), &e).unparsed, "depth {d}");
    }
}

#[test]
fn shell_deep_backtick_free_garbage_does_not_panic() {
    let e = env();
    let p = parse(&"$(".repeat(5000), &e);
    assert!(p.unparsed);
}

// -- shell: heredocs ----------------------------------------------------------

#[test]
fn shell_heredoc_body_is_skipped() {
    let e = env();
    let p = parse("cat <<EOF\nrm -rf x\ngit push\nEOF\nls", &e);
    assert!(!p.unparsed);
    assert!(
        has_cmd(&p, &["cat"]) && has_cmd(&p, &["ls"]),
        "{:?}",
        p.commands
    );
    assert!(
        !p.commands.iter().any(|c| c[0] == "rm" || c[0] == "git"),
        "{:?}",
        p.commands
    );
    assert!(p.paths.is_empty(), "{:?}", p.paths);
}

#[test]
fn shell_heredoc_quoted_and_dash_forms_are_skipped() {
    let e = env();
    for src in [
        "cat <<'EOF'\nrm -rf x\nEOF\nls",
        "cat <<\"EOF\"\nrm -rf x\nEOF\nls",
        "cat <<-EOF\n\trm -rf x\n\tEOF\nls",
    ] {
        let p = parse(src, &e);
        assert!(!p.unparsed, "{src:?}");
        assert!(
            !p.commands.iter().any(|c| c[0] == "rm"),
            "{src:?}: {:?}",
            p.commands
        );
        assert!(has_cmd(&p, &["ls"]), "{src:?}");
    }
}

#[test]
fn shell_heredoc_without_terminator_is_unparsed_but_keeps_earlier_commands() {
    let e = env();
    let p = parse("ls\ncat <<EOF\nbody\nmore", &e);
    assert!(p.unparsed);
    assert!(has_cmd(&p, &["ls"]));
}

// -- shell: redirects ---------------------------------------------------------

#[test]
fn shell_output_redirects_are_write_paths() {
    let e = env();
    for op in [">", ">>", ">|", "&>", "2>", "3>"] {
        let p = parse(&format!("echo hi {op} out.txt"), &e);
        assert!(
            pp(&p).contains(&w(&e.wd().join("out.txt"))),
            "{op}: {:?}",
            pp(&p)
        );
        assert_eq!(
            argvs(&p),
            vec![argv(&["echo", "hi"])],
            "{op}: redirect is not an argument"
        );
    }
}

#[test]
fn shell_redirect_without_space_is_recognised() {
    let e = env();
    let p = parse("echo hi >out.txt 2>err.txt", &e);
    assert!(pp(&p).contains(&w(&e.wd().join("out.txt"))));
    assert!(pp(&p).contains(&w(&e.wd().join("err.txt"))));
}

#[test]
fn shell_input_redirect_is_read_path() {
    let e = env();
    let p = parse("wc -l < in.txt", &e);
    assert!(pp(&p).contains(&r(&e.wd().join("in.txt"))), "{:?}", pp(&p));
    assert_eq!(argvs(&p), vec![argv(&["wc", "-l"])]);
}

#[test]
fn shell_redirect_to_absolute_path() {
    let e = env();
    let p = parse("echo x > /var/tmp/zz-nonexistent-leaf", &e);
    assert!(
        pp(&p)
            .iter()
            .any(|s| s.starts_with("W:") && s.ends_with("zz-nonexistent-leaf"))
    );
}

// -- shell: argument paths ----------------------------------------------------

#[test]
fn shell_plain_arguments_are_read_paths_but_flags_are_not() {
    let e = env();
    let p = parse("cat -n a.txt sub/b.txt", &e);
    assert_eq!(
        expect(&pp(&p)),
        expect(&[r(&e.wd().join("a.txt")), r(&e.wd().join("sub/b.txt"))])
    );
}

#[test]
fn shell_argv0_itself_is_not_a_path() {
    let e = env();
    assert!(pp(&parse("ls", &e)).is_empty());
}

#[test]
fn shell_arguments_with_variables_or_globs_are_skipped() {
    let e = env();
    for src in [
        "cat $FOO/x",
        "cat ${BAR}",
        "cat *.txt",
        "cat a?.txt",
        "cat [ab].txt",
        "cat a$X",
    ] {
        let p = parse(src, &e);
        assert!(pp(&p).is_empty(), "{src}: {:?}", pp(&p));
        assert!(!p.unparsed, "{src}");
    }
}

#[test]
fn shell_home_forms_expand() {
    let e = env();
    let want = r(&e.home().join("notes.txt"));
    for src in [
        "cat ~/notes.txt",
        "cat $HOME/notes.txt",
        "cat ${HOME}/notes.txt",
    ] {
        assert_eq!(pp(&parse(src, &e)), vec![want.clone()], "{src}");
    }
}

#[test]
fn shell_bare_tilde_is_home() {
    let e = env();
    assert_eq!(pp(&parse("ls ~", &e)), vec![r(e.home())]);
}

#[test]
fn shell_paths_are_in_source_order() {
    let e = env();
    let p = parse("cat a > b; cat c", &e);
    assert_eq!(
        pp(&p),
        vec![
            r(&e.wd().join("a")),
            w(&e.wd().join("b")),
            r(&e.wd().join("c"))
        ]
    );
}

#[test]
fn shell_dotdot_in_argument_is_normalised() {
    let e = env();
    let p = parse("cat sub/../x", &e);
    assert_eq!(pp(&p), vec![r(&e.wd().join("x"))]);
}

// -- shell: writer commands ---------------------------------------------------

#[test]
fn shell_writer_commands_give_write_and_read_for_arguments() {
    let e = env();
    for cmd in [
        "rm", "rmdir", "mv", "cp", "touch", "mkdir", "ln", "tee", "truncate", "install",
    ] {
        let p = parse(&format!("{cmd} target.dat"), &e);
        let t = e.wd().join("target.dat");
        assert!(pp(&p).contains(&w(&t)), "{cmd}: {:?}", pp(&p));
        assert!(pp(&p).contains(&r(&t)), "{cmd}: {:?}", pp(&p));
    }
}

#[test]
fn shell_chmod_and_chown_arguments_are_written() {
    let e = env();
    for src in ["chmod 600 f.dat", "chown me f.dat"] {
        let p = parse(src, &e);
        assert!(
            pp(&p).contains(&w(&e.wd().join("f.dat"))),
            "{src}: {:?}",
            pp(&p)
        );
    }
}

#[test]
fn shell_writer_command_via_absolute_path_and_every_argument() {
    let e = env();
    let p = parse("/bin/mv a b", &e);
    for n in ["a", "b"] {
        assert!(pp(&p).contains(&w(&e.wd().join(n))), "{:?}", pp(&p));
    }
}

#[test]
fn shell_writer_command_flags_are_not_paths() {
    let e = env();
    let p = parse("rm -rf build", &e);
    assert_eq!(
        expect(&pp(&p)),
        expect(&[r(&e.wd().join("build")), w(&e.wd().join("build"))])
    );
}

#[test]
fn shell_sed_i_writes_but_plain_sed_does_not() {
    let e = env();
    let f = e.wd().join("f.txt");
    assert!(pp(&parse("sed -i s/a/b/ f.txt", &e)).contains(&w(&f)));
    assert!(pp(&parse("sed -i.bak s/a/b/ f.txt", &e)).contains(&w(&f)));
    let plain = pp(&parse("sed s/a/b/ f.txt", &e));
    assert!(plain.contains(&r(&f)));
    assert!(!plain.contains(&w(&f)), "{plain:?}");
}

#[test]
fn shell_non_writer_command_never_writes_its_arguments() {
    let e = env();
    let out = pp(&parse("cat a; ls b; grep x c", &e));
    assert!(out.iter().all(|s| s.starts_with("R:")), "{out:?}");
}

#[test]
fn shell_dd_of_is_write_and_if_is_read() {
    let e = env();
    let p = parse("dd if=src.img of=dst.img bs=1M", &e);
    assert_eq!(
        expect(&pp(&p)),
        expect(&[r(&e.wd().join("src.img")), w(&e.wd().join("dst.img"))])
    );
}

#[test]
fn shell_dd_of_with_home_expansion() {
    let e = env();
    let p = parse("dd of=~/disk.img", &e);
    assert!(
        pp(&p).contains(&w(&e.home().join("disk.img"))),
        "{:?}",
        pp(&p)
    );
}

// -- shell: unparsed ----------------------------------------------------------

#[test]
fn shell_unbalanced_quotes_are_unparsed() {
    let e = env();
    for src in ["echo \"abc", "echo 'abc", "echo $'abc", "echo \"a\\\""] {
        assert!(parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn shell_unclosed_substitution_is_unparsed() {
    let e = env();
    for src in [
        "echo $(ls",
        "echo `ls",
        "echo \"$(ls\"",
        "echo $(echo $(ls)",
    ] {
        assert!(parse(src, &e).unparsed, "{src}");
    }
}

#[test]
fn shell_unparsed_still_returns_what_was_parsed_before_the_error() {
    let e = env();
    let p = parse("ls; git push; echo 'oops", &e);
    assert!(p.unparsed);
    assert!(
        has_cmd(&p, &["ls"]) && has_cmd(&p, &["git", "push"]),
        "{:?}",
        p.commands
    );
}

#[test]
fn shell_input_over_64_kib_is_unparsed() {
    let e = env();
    let big = format!("echo {}", "a".repeat(64 * 1024));
    assert!(big.len() > 64 * 1024);
    assert!(parse(&big, &e).unparsed);
}

#[test]
fn shell_input_of_exactly_64_kib_is_parsed() {
    let e = env();
    let mut s = String::from("echo ");
    s.push_str(&"a".repeat(64 * 1024 - s.len()));
    assert_eq!(s.len(), 64 * 1024);
    assert!(!parse(&s, &e).unparsed);
}

#[test]
fn shell_wellformed_input_is_not_unparsed() {
    let e = env();
    for src in [
        "ls -la",
        "echo 'a b' \"c d\"",
        "git commit -m \"msg $(date)\"",
        "cat <<E\nx\nE\n",
    ] {
        assert!(!parse(src, &e).unparsed, "{src}");
    }
}

// -- subject_path -------------------------------------------------------------

#[test]
fn subject_path_relative_is_joined_to_workdir() {
    let e = env();
    assert_eq!(subject_path("a/b.txt", &e.sctx), e.wd().join("a/b.txt"));
}

#[test]
fn subject_path_nonexistent_leaf_is_appended() {
    let e = env();
    std::fs::create_dir(e.wd().join("dir")).unwrap();
    assert_eq!(
        subject_path("dir/new/deeper.txt", &e.sctx),
        e.wd().join("dir/new/deeper.txt")
    );
}

#[test]
fn subject_path_tilde_expands_to_home() {
    let e = env();
    assert_eq!(subject_path("~/x/y", &e.sctx), e.home().join("x/y"));
    assert_eq!(subject_path("~", &e.sctx), e.home().to_path_buf());
}

#[test]
fn subject_path_dot_and_dotdot_are_normalised() {
    let e = env();
    std::fs::create_dir(e.wd().join("sub")).unwrap();
    assert_eq!(subject_path("./sub/./f", &e.sctx), e.wd().join("sub/f"));
    assert_eq!(subject_path("sub/../f", &e.sctx), e.wd().join("f"));
    assert_eq!(subject_path("nope/../f", &e.sctx), e.wd().join("f"));
    assert_eq!(subject_path("a/b/../../c", &e.sctx), e.wd().join("c"));
}

#[test]
fn subject_path_has_no_containment_check() {
    let e = env();
    let out = subject_path("../outside.txt", &e.sctx);
    assert_eq!(out, e.wd().parent().unwrap().join("outside.txt"));
}

#[cfg(unix)]
#[test]
fn subject_path_resolves_symlinked_prefix() {
    let e = env();
    let real = e.wd().join("real");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, e.wd().join("link")).unwrap();
    assert_eq!(
        subject_path("link/file.txt", &e.sctx),
        real.join("file.txt")
    );
    std::fs::write(real.join("exists.txt"), "x").unwrap();
    assert_eq!(
        subject_path("link/exists.txt", &e.sctx),
        real.join("exists.txt")
    );
}

#[cfg(unix)]
#[test]
fn subject_path_symlink_pointing_outside_workdir_resolves_to_target() {
    let e = env();
    let outside = tempfile::tempdir().unwrap();
    let outside_c = outside.path().canonicalize().unwrap();
    std::os::unix::fs::symlink(&outside_c, e.wd().join("escape")).unwrap();
    assert_eq!(subject_path("escape/k", &e.sctx), outside_c.join("k"));
}

#[cfg(unix)]
#[test]
fn subject_path_dotdot_after_symlink_follows_the_real_directory() {
    let e = env();
    let real = e.wd().join("real");
    std::fs::create_dir_all(real.join("sub")).unwrap();
    std::os::unix::fs::symlink(real.join("sub"), e.wd().join("link")).unwrap();
    // OS semantics: link/.. is real/, not the workdir.
    assert_eq!(subject_path("link/../x", &e.sctx), real.join("x"));
}

#[cfg(unix)]
#[test]
fn subject_path_symlinked_file_leaf_resolves() {
    let e = env();
    std::fs::write(e.wd().join("target.txt"), "x").unwrap();
    std::os::unix::fs::symlink(e.wd().join("target.txt"), e.wd().join("alias.txt")).unwrap();
    assert_eq!(
        subject_path("alias.txt", &e.sctx),
        e.wd().join("target.txt")
    );
}

#[cfg(unix)]
#[test]
fn subject_path_dangling_symlink_does_not_panic() {
    let e = env();
    std::os::unix::fs::symlink(e.wd().join("missing-target"), e.wd().join("dangling")).unwrap();
    let out = subject_path("dangling", &e.sctx);
    assert!(
        out.starts_with(e.wd()) || out.ends_with("missing-target"),
        "{out:?}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn subject_path_restores_on_disk_case_on_macos() {
    let e = env().with_case_insensitive(true);
    std::fs::create_dir(e.wd().join("RealDir")).unwrap();
    if !e.wd().join("realdir").exists() {
        return; // case-sensitive volume: nothing to assert
    }
    assert_eq!(
        subject_path("realdir/new.txt", &e.sctx),
        e.wd().join("RealDir/new.txt")
    );
    assert_eq!(subject_path("REALDIR", &e.sctx), e.wd().join("RealDir"));
}

#[cfg(target_os = "macos")]
#[test]
fn shell_paths_restore_on_disk_case_on_macos() {
    let e = env().with_case_insensitive(true);
    std::fs::create_dir(e.wd().join(".Hidden")).unwrap();
    if !e.wd().join(".hidden").exists() {
        return;
    }
    let p = parse("cat .hidden/key1", &e);
    assert_eq!(pp(&p), vec![r(&e.wd().join(".Hidden/key1"))]);
}

// -- Tool::subjects: file tools ----------------------------------------------

#[test]
fn read_file_gives_path_read() {
    let e = env();
    let ts = subj(&ReadFileTool, json!({"path": "a.txt"}), &e);
    assert_eq!(view(&ts), vec![r(&e.wd().join("a.txt"))]);
    assert!(!ts.unparsed);
}

#[test]
fn read_file_absolute_and_tilde_paths() {
    let e = env();
    let abs = e.wd().join("x/y.txt");
    assert_eq!(
        view(&subj(
            &ReadFileTool,
            json!({"path": abs.to_str().unwrap()}),
            &e
        )),
        vec![r(&abs)]
    );
    assert_eq!(
        view(&subj(&ReadFileTool, json!({"path": "~/n.txt"}), &e)),
        vec![r(&e.home().join("n.txt"))]
    );
}

#[test]
fn read_file_without_path_has_no_subjects() {
    let e = env();
    assert!(subj(&ReadFileTool, json!({}), &e).subjects.is_empty());
}

#[test]
fn write_file_gives_path_write() {
    let e = env();
    let ts = subj(
        &WriteFileTool,
        json!({"path": "out.txt", "content": "x"}),
        &e,
    );
    assert_eq!(view(&ts), vec![w(&e.wd().join("out.txt"))]);
}

#[test]
fn edit_file_gives_read_and_write() {
    let e = env();
    let ts = subj(
        &EditFileTool,
        json!({"path": "a.rs", "old_str": "a", "new_str": "b"}),
        &e,
    );
    let p = e.wd().join("a.rs");
    assert_eq!(view(&ts), expect(&[r(&p), w(&p)]));
}

#[test]
fn multi_edit_gives_read_and_write_for_each_distinct_path() {
    let e = env();
    let ts = subj(
        &MultiEditTool,
        json!({"edits": [
            {"path": "a.rs", "old_str": "1", "new_str": "2"},
            {"path": "a.rs", "old_str": "3", "new_str": "4"},
            {"path": "b.rs", "old_str": "5", "new_str": "6"},
        ]}),
        &e,
    );
    let (a, b) = (e.wd().join("a.rs"), e.wd().join("b.rs"));
    assert_eq!(view(&ts), expect(&[r(&a), w(&a), r(&b), w(&b)]));
    assert_eq!(
        ts.subjects.len(),
        4,
        "distinct paths only: {:?}",
        ts.subjects
    );
}

#[test]
fn multi_edit_with_no_edits_has_no_subjects() {
    let e = env();
    assert!(
        subj(&MultiEditTool, json!({"edits": []}), &e)
            .subjects
            .is_empty()
    );
}

#[test]
fn apply_patch_modify_uses_headers_with_prefixes_stripped() {
    let e = env();
    let diff = "--- a/src/x.rs\n+++ b/src/x.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n";
    let ts = subj(&PatchTool, json!({"diff": diff}), &e);
    let p = e.wd().join("src/x.rs");
    assert_eq!(view(&ts), expect(&[r(&p), w(&p)]));
}

#[test]
fn apply_patch_new_file_skips_dev_null() {
    let e = env();
    let diff = "--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1,1 @@\n+fn main() {}\n";
    let ts = subj(&PatchTool, json!({"diff": diff}), &e);
    let p = e.wd().join("new.rs");
    assert_eq!(view(&ts), expect(&[r(&p), w(&p)]));
}

#[test]
fn apply_patch_deleted_file_skips_dev_null() {
    let e = env();
    let diff = "--- a/old.rs\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-gone\n";
    let ts = subj(&PatchTool, json!({"diff": diff}), &e);
    let p = e.wd().join("old.rs");
    assert_eq!(view(&ts), expect(&[r(&p), w(&p)]));
}

#[test]
fn apply_patch_without_prefixes_and_multiple_files() {
    let e = env();
    let diff = "--- one.txt\n+++ one.txt\n@@ -1 +1 @@\n-a\n+b\n--- a/two.txt\n+++ b/two.txt\n@@ -1 +1 @@\n-c\n+d\n";
    let ts = subj(&PatchTool, json!({"diff": diff}), &e);
    let (one, two) = (e.wd().join("one.txt"), e.wd().join("two.txt"));
    assert_eq!(view(&ts), expect(&[r(&one), w(&one), r(&two), w(&two)]));
}

#[test]
fn apply_patch_unparseable_diff_has_no_path_subjects() {
    let e = env();
    let ts = subj(&PatchTool, json!({"diff": "this is not a diff"}), &e);
    assert!(ts.subjects.is_empty(), "{:?}", ts.subjects);
}

#[test]
fn grep_and_glob_read_the_search_root() {
    let e = env();
    for (tool, name) in [
        (&GrepTool as &dyn Tool, "grep"),
        (&GlobTool as &dyn Tool, "glob"),
    ] {
        let with = subj(tool, json!({"pattern": "x", "path": "src"}), &e);
        assert_eq!(
            view(&with),
            vec![r(&e.wd().join("src"))],
            "{name} with path"
        );
        let without = subj(tool, json!({"pattern": "x"}), &e);
        assert_eq!(view(&without), vec![r(e.wd())], "{name} default root");
    }
}

#[test]
fn lsp_tools_read_the_file() {
    let e = env();
    let bridge = Arc::new(MockLspBridge::builder().build());
    let diag = LspDiagnosticsTool::new(bridge.clone());
    let hover = LspHoverTool::new(bridge.clone());
    let actions = LspCodeActionTool::new(bridge);
    let p = e.wd().join("foo.rs");
    for (tool, name) in [
        (&diag as &dyn Tool, "lsp_diagnostics"),
        (&hover as &dyn Tool, "lsp_hover"),
        (&actions as &dyn Tool, "lsp_code_actions"),
    ] {
        let ts = subj(tool, json!({"file": "foo.rs", "line": 1, "col": 1}), &e);
        assert_eq!(view(&ts), vec![r(&p)], "{name}");
    }
}

// -- Tool::subjects: shell ----------------------------------------------------

#[test]
fn shell_tool_subjects_come_from_the_parser() {
    let e = env();
    let ts = subj(
        &ShellTool,
        json!({"command": "git push origin; cat a.txt > b.txt"}),
        &e,
    );
    let v = view(&ts);
    assert!(v.contains(&"C:git push origin".to_string()), "{v:?}");
    assert!(v.contains(&"C:cat a.txt".to_string()), "{v:?}");
    assert!(v.contains(&r(&e.wd().join("a.txt"))), "{v:?}");
    assert!(v.contains(&w(&e.wd().join("b.txt"))), "{v:?}");
    assert!(!ts.unparsed);
}

#[test]
fn shell_tool_reports_unparsed() {
    let e = env();
    assert!(subj(&ShellTool, json!({"command": "echo 'x"}), &e).unparsed);
}

#[test]
fn shell_tool_matches_parse_shell_into_subjects() {
    let e = env();
    let cmd = "FOO=1 /usr/bin/git push && rm -rf build";
    let via_tool = view(&subj(&ShellTool, json!({"command": cmd}), &e));
    let via_parser = view(&parse_shell(cmd, &e.sctx).into_subjects());
    assert_eq!(via_tool, via_parser);
}

#[test]
fn shell_tool_honours_case_insensitive_context() {
    let e = env().with_case_insensitive(true);
    let v = view(&subj(&ShellTool, json!({"command": "GIT push"}), &e));
    assert!(v.contains(&"C:git push".to_string()), "{v:?}");
}

// -- Tool::subjects: web_fetch ------------------------------------------------

fn fetch_hosts(url: &str, e: &Env) -> Vec<String> {
    view(&subj(&WebFetchTool::new(), json!({"url": url}), e))
}

#[test]
fn web_fetch_gives_host_from_url() {
    let e = env();
    assert_eq!(
        fetch_hosts("https://example.com/a/b?q=1", &e),
        ["H:example.com"]
    );
    assert_eq!(
        fetch_hosts("https://Example.COM:8443/p", &e),
        ["H:example.com"]
    );
    assert_eq!(
        fetch_hosts("https://user:pw@files.pypi.org/x", &e),
        ["H:files.pypi.org"]
    );
}

#[test]
fn web_fetch_canonicalises_short_ipv4() {
    let e = env();
    assert_eq!(fetch_hosts("http://127.1/", &e), ["H:127.0.0.1"]);
    assert_eq!(fetch_hosts("http://2130706433/", &e), ["H:127.0.0.1"]);
    assert_eq!(fetch_hosts("http://0x7f.0.0.1/", &e), ["H:127.0.0.1"]);
}

#[test]
fn web_fetch_ipv6_literal_is_accepted_form() {
    let e = env();
    let v = fetch_hosts("http://[::1]:8080/", &e);
    assert!(v == ["H:[::1]"] || v == ["H:::1"], "{v:?}");
}

#[test]
fn web_fetch_unparseable_url_gives_raw_host_subject() {
    let e = env();
    assert_eq!(fetch_hosts("not a url", &e), ["H:not a url"]);
    assert_eq!(fetch_hosts("http://", &e), ["H:http://"]);
}

#[test]
fn web_fetch_without_url_has_no_subjects() {
    let e = env();
    assert!(
        subj(&WebFetchTool::new(), json!({}), &e)
            .subjects
            .is_empty()
    );
}

// -- Tool::subjects: git ------------------------------------------------------

#[test]
fn git_tools_map_to_git_subcommand_commands() {
    let e = env();
    let cases: Vec<(&dyn Tool, &str)> = vec![
        (&GitStatusTool as &dyn Tool, "C:git status"),
        (&GitDiffTool as &dyn Tool, "C:git diff"),
        (&GitCommitTool as &dyn Tool, "C:git commit"),
        (&GitUndoTool as &dyn Tool, "C:git undo"),
        (&GitRedoTool as &dyn Tool, "C:git redo"),
    ];
    for (tool, want) in cases {
        let ts = subj(tool, json!({"message": "m"}), &e);
        assert_eq!(view(&ts), vec![want.to_string()], "{}", tool.name());
    }
}

#[test]
fn git_checkout_and_branch_append_their_name() {
    let e = env();
    let ts = subj(
        &GitCheckoutTool,
        json!({"branch": "dev", "name": "dev"}),
        &e,
    );
    assert_eq!(view(&ts), ["C:git checkout dev"]);
    let ts = subj(&GitBranchTool, json!({"name": "feature-x"}), &e);
    assert_eq!(view(&ts), ["C:git branch feature-x"]);
}

#[test]
fn git_subject_is_matched_by_a_command_glob_rule() {
    // The command subject must be shaped so `git checkout*` style rules apply.
    let e = env();
    let ts = subj(
        &GitCheckoutTool,
        json!({"branch": "main", "name": "main"}),
        &e,
    );
    match &ts.subjects[0] {
        Subject::Command(c) => assert!(c.starts_with("git checkout")),
        other => panic!("expected Command, got {other:?}"),
    }
}

// -- Tool::subjects: tools with no subjects ----------------------------------

#[test]
fn tools_without_subjects_return_empty_default() {
    let e = env();
    let none: Vec<(&dyn Tool, Value)> = vec![
        (&ScreenCaptureTool as &dyn Tool, json!({})),
        (
            &ComputerControlTool as &dyn Tool,
            json!({"action": "click"}),
        ),
    ];
    for (tool, input) in none {
        let ts = subj(tool, input, &e);
        assert!(ts.subjects.is_empty(), "{}: {:?}", tool.name(), ts.subjects);
        assert!(!ts.unparsed, "{}", tool.name());
    }
    let search = WebSearchTool::from_env();
    let ts = subj(&search, json!({"query": "rust"}), &e);
    assert!(ts.subjects.is_empty() && !ts.unparsed);
}

#[test]
fn tool_subjects_default_is_empty_and_parsed() {
    let ts = ToolSubjects::default();
    assert!(ts.subjects.is_empty());
    assert!(!ts.unparsed);
}

#[test]
fn path_subjects_from_tools_are_canonical_through_symlinks() {
    let e = env();
    let real = e.wd().join("real");
    std::fs::create_dir(&real).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real, e.wd().join("link")).unwrap();
        let ts = subj(&ReadFileTool, json!({"path": "link/f.txt"}), &e);
        assert_eq!(view(&ts), vec![r(&real.join("f.txt"))]);
        let ts = subj(
            &WriteFileTool,
            json!({"path": "link/f.txt", "content": ""}),
            &e,
        );
        assert_eq!(view(&ts), vec![w(&real.join("f.txt"))]);
    }
}

#[allow(dead_code)]
fn _unused(_: PathBuf) {}
