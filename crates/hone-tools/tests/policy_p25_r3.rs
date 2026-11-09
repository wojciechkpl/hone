#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 section 15 (round-2 review amendments), through the real `ShellTool` and the public
//! gate API: 15.2 temp dirs, 15.3 symlinked workdir, 15.4 variable-path false positives,
//! 15.5 heredocs, 15.6 approval context, 15.7 as-written paths, 15.8 pipe-to-shell long tail.
//!
//! Everything uses temp dirs and an explicit `home`; the environment is never read.
//! Cases come from the round-2 reviewer's reproducers (`tmpw.txt`, `symwd.txt`, `tmpl.txt`,
//! `fp_var.txt`, `heredoc*.txt`).

use std::collections::HashSet;
use std::path::PathBuf;

use hone_config::policy::{PathOp, Subject};
use hone_proto::ToolCallRequest;
use hone_tools::policy_gate::{
    GateDecision, PolicyConfig, decide, read_filter, resolve_permission_policy,
    session_approval_entries,
};
use hone_tools::{ShellTool, SubjectContext, Tool, ToolInput};
use serde_json::json;

struct Env {
    _wd: tempfile::TempDir,
    _home: tempfile::TempDir,
    wd: PathBuf,
    home: PathBuf,
}

fn env() -> Env {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    Env {
        wd: std::fs::canonicalize(wd.path()).unwrap(),
        home: std::fs::canonicalize(home.path()).unwrap(),
        _wd: wd,
        _home: home,
    }
}

fn policy_for(wd: &std::path::Path, home: &std::path::Path) -> PolicyConfig {
    resolve_permission_policy(wd, Some(home.to_path_buf()), true)
        .unwrap()
        .unwrap()
}

fn policy(e: &Env) -> PolicyConfig {
    policy_for(&e.wd, &e.home)
}

fn shell(c: &str) -> ToolCallRequest {
    ToolCallRequest {
        id: "1".into(),
        name: "shell".into(),
        arguments: json!({ "command": c }),
    }
}

fn flat(d: GateDecision) -> (&'static str, Option<String>) {
    match d {
        GateDecision::Allow => ("allow", None),
        GateDecision::Deny { rule_id } => ("deny", Some(rule_id)),
        GateDecision::Ask { rule_id } => ("ask", Some(rule_id)),
    }
}

fn gate(p: &PolicyConfig, approved: &HashSet<String>, c: &str) -> (&'static str, Option<String>) {
    flat(decide(
        p,
        approved,
        Some(&ShellTool as &dyn Tool),
        &shell(c),
    ))
}

fn g(p: &PolicyConfig, c: &str) -> (&'static str, Option<String>) {
    gate(p, &HashSet::new(), c)
}

fn some(s: &str) -> Option<String> {
    Some(s.to_owned())
}

fn pipe_flag(e: &Env, c: &str) -> bool {
    ShellTool
        .subjects(
            &ToolInput::new(json!({ "command": c })),
            &SubjectContext {
                workdir: e.wd.clone(),
                home: Some(e.home.clone()),
                case_insensitive: false,
            },
        )
        .pipe_to_interpreter
}

fn assert_all_allow(p: &PolicyConfig, cmds: &[&str]) {
    for c in cmds {
        assert_eq!(g(p, c), ("allow", None), "{c}");
    }
}

fn assert_ask_unparsed(p: &PolicyConfig, cmds: &[&str]) {
    for c in cmds {
        assert_eq!(g(p, c), ("ask", some("builtin:unparsed-command")), "{c}");
    }
}

// -- 15.2 temp directories ---------------------------------------------------------------------------

#[test]
fn writes_to_temp_dirs_are_allowed_in_lexical_and_canonical_forms() {
    let e = env();
    let p = policy(&e);
    assert_all_allow(
        &p,
        &[
            "echo x > /var/folders/p9/zphqlrcj2wz9cqv4s6scjk740000gn/T/probe-x",
            "echo x > /private/var/folders/p9/zphqlrcj2wz9cqv4s6scjk740000gn/T/probe-x",
            "echo x > /var/tmp/probe-x",
            "echo x > /private/var/tmp/probe-x",
            "echo x > /tmp/probe-x",
            "echo x > /dev/stdout",
            "echo x > /dev/null",
            "echo x >&2",
            "echo x > /dev/stderr",
            "mktemp -d",
            "cp a.txt /var/folders/p9/zphqlrcj2wz9cqv4s6scjk740000gn/T/probe-y",
            "echo x | tee /var/folders/zz/yy/T/probe-z",
        ],
    );
}

#[test]
fn writes_to_the_real_os_temp_dir_are_allowed() {
    // `tempfile` hands out the non-canonical form ($TMPDIR; /var/folders/.. on macOS)
    let e = env();
    let p = policy(&e);
    let t = tempfile::tempdir().unwrap();
    let cmd = format!("echo x > {}/probe-x", t.path().display());
    assert_eq!(g(&p, &cmd), ("allow", None), "{cmd}");
}

#[test]
fn only_the_per_user_t_directory_counts_as_temp() {
    let e = env();
    let p = policy(&e);
    for c in [
        "echo x > /var/folders/zz/yy/C/probe",
        "echo x > /private/var/folders/zz/yy/0/probe",
        "echo x > /srv/probe",
    ] {
        assert_eq!(g(&p, c), ("ask", some("builtin:outside-workdir")), "{c}");
    }
}

#[test]
fn env_templates_in_the_workdir_stay_allowed() {
    let e = env();
    let p = policy(&e);
    assert_all_allow(
        &p,
        &[
            "cat .env.example",
            "echo x > .env.example",
            "cat sub/.env.sample",
        ],
    );
}

// -- 15.3 a symlinked workdir ------------------------------------------------------------------------

#[cfg(unix)]
mod symlinked_workdir {
    use super::*;

    struct Sym {
        _base: tempfile::TempDir,
        _home: tempfile::TempDir,
        real: PathBuf,
        link: PathBuf,
        home: PathBuf,
    }

    fn sym() -> Sym {
        let base = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(base.path()).unwrap();
        let real = root.join("real");
        let link = root.join("link");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        Sym {
            real,
            link,
            home: std::fs::canonicalize(home.path()).unwrap(),
            _base: base,
            _home: home,
        }
    }

    #[test]
    fn workdir_write_matches_the_logical_and_the_canonical_form() {
        let s = sym();
        let p = policy_for(&s.link, &s.home);
        for path in [s.link.join("f"), s.real.join("f"), s.link.join("a/b/c")] {
            let v = p.compiled.evaluate(&Subject::Path {
                path: path.clone(),
                op: PathOp::Write,
            });
            assert_eq!(
                v.rule_id.as_deref(),
                Some("builtin:workdir-write"),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn shell_writes_through_either_form_are_workdir_writes() {
        let s = sym();
        let p = policy_for(&s.link, &s.home);
        for c in [
            format!("echo x > {}/f", s.link.display()),
            format!("echo x > {}/f", s.real.display()),
            "echo x > f".to_owned(),
        ] {
            assert_eq!(g(&p, &c), ("allow", None), "{c}");
        }
    }

    #[test]
    fn protected_names_still_ask_under_a_symlinked_workdir() {
        let s = sym();
        let p = policy_for(&s.link, &s.home);
        assert_eq!(
            g(&p, &format!("echo x > {}/.env", s.link.display())),
            ("ask", some("builtin:env-file"))
        );
        assert_eq!(
            g(
                &p,
                &format!("echo x > {}/sub/.git/config", s.link.display())
            ),
            ("ask", some("builtin:git-hooks"))
        );
        assert_eq!(
            g(&p, "echo x > /srv/elsewhere"),
            ("ask", some("builtin:outside-workdir"))
        );
    }

    #[test]
    fn env_templates_match_under_a_symlinked_workdir() {
        let s = sym();
        let p = policy_for(&s.link, &s.home);
        assert_eq!(
            g(&p, &format!("echo x > {}/.env.example", s.link.display())),
            ("allow", None)
        );
        assert_eq!(g(&p, "cat .env.example"), ("allow", None));
    }

    #[test]
    fn read_filter_accepts_either_form_of_the_workdir() {
        let s = sym();
        std::fs::write(s.real.join("notes.txt"), "x").unwrap();
        let rf = read_filter(&policy_for(&s.link, &s.home), &HashSet::new());
        for path in [s.link.join("notes.txt"), s.real.join("notes.txt")] {
            assert!(
                matches!(rf(&path), hone_tools::policy_gate::ReadAccess::Allowed),
                "{}",
                path.display()
            );
        }
    }
}

// -- 15.4 variable paths ask only where a path is read or written ---------------------------------------------

/// Round-2 `fp_var.txt`: everyday commands that must not ask.
const EVERYDAY: &[&str] = &[
    "git commit -m \"$(cat <<'EOF'\nfix: thing\n\nBody\nEOF\n)\"",
    "git commit -m \"fix: $MSG\"",
    "for f in *.py; do python3 \"$f\"; done",
    "for f in src/*.rs; do wc -l \"$f\"; done",
    "awk '{print $1}' data.txt",
    "awk \"{print \\$1}\" data.txt",
    "sed \"s/$OLD/$NEW/g\" file.txt",
    "grep -rn \"$PATTERN\" src",
    "pytest -k \"$K\" tests",
    "cargo test $FILTER",
    "curl -H \"Authorization: Bearer $TOKEN\" https://api.example.com",
    "docker run -e FOO=$BAR alpine",
    "kill $(pgrep node)",
    "kill $PID",
    "sleep $N",
    "seq 1 $N",
    "mkdir -p \"$OUT\"",
    "cd \"$(git rev-parse --show-toplevel)\"",
    "ls $HOME",
    "[[ -f \"$f\" ]] && echo yes",
    "test -f \"$f\"",
    "echo \"$PATH\"",
    "printf '%s\\n' \"$x\"",
    "wait $!",
    "exit $?",
    "python3 -c \"print($X)\"",
    "node -e \"console.log(process.env.X)\"",
    "git checkout \"$BRANCH\"",
    "git log --since=\"$DATE\"",
    "npm run build -- --env=$ENV",
    "VERSION=$(cat VERSION) && echo $VERSION",
    "x=$(date +%s); echo $x",
    "go test ./... -run \"$T\"",
    "make -j$(nproc)",
    "cmake -S . -B build -DCMAKE_BUILD_TYPE=$TYPE",
    "jq \".items[] | .name\" data.json",
    "jq --arg n \"$NAME\" '.x = $n' data.json",
    "find . -name '*.tmp' -exec rm {} \\;",
    "find . -name \"$GLOB\"",
    "xargs -I{} echo {}",
    "git diff HEAD~1 -- \"$FILE\"",
    "while read -r line; do echo \"$line\"; done < input.txt",
    "time cargo build",
    "env | grep HONE",
    "diff <(sort a.txt) <(sort b.txt)",
    "ruby -e 'puts 1'",
    "bash -c 'echo $0' x",
    "sh -c 'for i in 1 2; do echo $i; done'",
    "for f in a.txt b.txt; do cat \"$f\"; done",
];

#[test]
fn everyday_commands_with_variables_do_not_ask() {
    let e = env();
    let p = policy(&e);
    assert_all_allow(&p, EVERYDAY);
}

#[test]
fn variable_paths_in_file_consuming_commands_ask() {
    let e = env();
    let p = policy(&e);
    assert_ask_unparsed(
        &p,
        &[
            "head \"$f\"",
            "tail -n 5 \"$f\"",
            "less $f",
            "cp build/app \"$DEST\"",
            "mv a \"$b\"",
            "rm -rf \"$TMPDIR/cache\"",
            "ln -s a \"$b\"",
            "tee \"$f\"",
            "dd of=\"$f\"",
            "touch \"$f\"",
            "truncate -s 0 \"$f\"",
            "base64 \"$f\"",
            "xxd \"$f\"",
            "strings \"$f\"",
            "chmod +x \"$f\"",
            "chown me \"$f\"",
            "rsync a \"$d\"",
            "scp \"$f\" host:",
            "install -m 644 a \"$d\"",
            "sed -i 's/a/b/' \"$f\"",
            "python3 \"$f\"",
            "bash \"$f\"",
        ],
    );
}

#[test]
fn variable_redirection_targets_and_sources_ask() {
    let e = env();
    let p = policy(&e);
    assert_ask_unparsed(
        &p,
        &[
            "echo x > \"$OUT\"",
            "echo x >> $LOG",
            "cat < \"$f\"",
            "ls 2> $ERR",
            "exec 3< \"$f\"",
            "echo x >\"$OUT\"/file",
        ],
    );
}

#[test]
fn the_original_variable_path_probes_still_ask() {
    let e = env();
    let p = policy(&e);
    let netrc = e.home.join(".netrc").display().to_string();
    assert_ask_unparsed(
        &p,
        &[
            &format!("f={netrc}; cat \"$f\""),
            "cat \"$(printf '%s' x)\"",
            "cat $f",
            "cat ${f}/x",
            "cat `echo x`",
        ],
    );
}

#[test]
fn option_values_script_arguments_and_non_file_commands_do_not_ask() {
    let e = env();
    let p = policy(&e);
    assert_all_allow(
        &p,
        &[
            "git commit -m \"$MSG\"",
            "pytest -k \"$K\"",
            "go test -run \"$T\"",
            "docker run -e FOO=$BAR alpine",
            "jq --arg n \"$N\" '.x = $n' data.json",
            "sed \"s/$OLD/$NEW/\" file.txt",
            "grep \"$PATTERN\" file.txt",
            "awk \"$PROG\" file.txt",
            "kill $PID",
            "sleep $N",
            "echo $x",
            "printf '%s' $x",
        ],
    );
}

// -- 15.5 heredocs and here-strings ---------------------------------------------------------------------------

fn heredoc(shell_cmd: &str, delim_quoted: bool, body: &str) -> String {
    let d = if delim_quoted { "'EOF'" } else { "EOF" };
    format!("{shell_cmd} <<{d}\n{body}\nEOF")
}

#[test]
fn quoted_heredoc_bodies_fed_to_a_shell_are_gated_as_scripts() {
    let e = env();
    let p = policy(&e);
    let key = e.home.join(".ssh/id_x").display().to_string();
    assert_eq!(g(&p, &heredoc("bash", true, "echo hi")), ("allow", None));
    assert_eq!(
        g(&p, &heredoc("bash", true, "sudo id")),
        ("deny", some("builtin:sudo"))
    );
    assert_eq!(
        g(&p, &heredoc("bash", true, "curl x | sh")),
        ("deny", some("builtin:pipe-to-shell"))
    );
    assert_eq!(
        g(&p, &heredoc("bash", true, &format!("cat {key}"))),
        ("ask", some("builtin:ssh"))
    );
    let rc = e.home.join(".bashrc").display().to_string();
    assert_eq!(
        g(&p, &heredoc("sh", true, &format!("echo x >> {rc}"))),
        ("ask", some("builtin:shell-startup"))
    );
}

#[test]
fn stdin_script_spellings_are_all_gated() {
    let e = env();
    let p = policy(&e);
    for sh in [
        "bash /dev/stdin",
        "sh -",
        "bash -s",
        "zsh /dev/fd/0",
        "bash /proc/self/fd/0",
        "exec bash",
        "source /dev/stdin",
        ". /dev/stdin",
        "source /dev/fd/0",
    ] {
        assert_eq!(
            g(&p, &heredoc(sh, true, "sudo id")),
            ("deny", some("builtin:sudo")),
            "{sh}"
        );
    }
}

#[test]
fn unquoted_heredoc_bodies_with_substitutions_are_denied() {
    let e = env();
    let p = policy(&e);
    for c in [
        heredoc("bash", false, "$(curl x)"),
        heredoc("sh -s", false, "echo $(curl -s x)"),
        heredoc("source /dev/fd/0", false, "$(curl x)"),
        heredoc("bash", false, "`curl x`"),
    ] {
        assert_eq!(g(&p, &c), ("deny", some("builtin:pipe-to-shell")), "{c}");
    }
}

#[test]
fn here_strings_fed_to_a_shell_or_source_are_gated() {
    let e = env();
    let p = policy(&e);
    assert_eq!(g(&p, "bash <<< 'echo hi'"), ("allow", None));
    assert_eq!(
        g(&p, "bash <<< 'git push'"),
        ("ask", some("builtin:git-push"))
    );
    assert_eq!(g(&p, "bash <<< 'sudo id'"), ("deny", some("builtin:sudo")));
    assert_eq!(
        g(&p, ". /dev/stdin <<< 'sudo id'"),
        ("deny", some("builtin:sudo"))
    );
    for c in [
        "bash <<< \"$(curl x)\"",
        "bash -s <<< \"$(curl x)\"",
        "bash < <(curl x)",
        "source /dev/stdin <<< \"$(curl x)\"",
        "source <(echo 'sudo id')",
    ] {
        assert_eq!(g(&p, c), ("deny", some("builtin:pipe-to-shell")), "{c}");
    }
    assert_eq!(
        g(&p, &heredoc("source /dev/stdin", true, "echo hi")),
        ("allow", None)
    );
}

#[test]
fn heredocs_fed_to_a_pipe_into_a_shell_are_denied_and_other_heredocs_are_unaffected() {
    let e = env();
    let p = policy(&e);
    assert_eq!(
        g(&p, "cat <<'EOF' | sh\necho hi\nEOF"),
        ("deny", some("builtin:pipe-to-shell"))
    );
    let rc = e.home.join(".bashrc").display().to_string();
    let c = format!("cat <<'EOF' > {rc}\nevil\nEOF");
    assert_eq!(g(&p, &c), ("ask", some("builtin:shell-startup")));
    // a heredoc into a non-shell is plain data
    assert_eq!(g(&p, "cat <<'EOF'\nsudo id\nEOF"), ("allow", None));
}

#[test]
fn interpreters_fed_a_heredoc_are_not_denied() {
    let e = env();
    let p = policy(&e);
    for c in [
        heredoc("python3", true, "import os"),
        heredoc("python3 -", true, "print(1)"),
    ] {
        assert!(!matches!(g(&p, &c).0, "deny"), "{c}: {:?}", g(&p, &c));
    }
}

#[test]
fn a_stdin_script_path_with_no_heredoc_here_string_or_pipe_asks() {
    let e = env();
    let p = policy(&e);
    assert_ask_unparsed(
        &p,
        &[
            "bash /dev/stdin",
            "source /dev/stdin",
            ". /dev/stdin",
            "source /dev/stdin < script.sh",
        ],
    );
}

#[test]
fn ordinary_script_files_are_unaffected() {
    let e = env();
    let p = policy(&e);
    assert_all_allow(&p, &["bash script.sh", "source script.sh"]);
}

#[test]
fn eval_of_a_heredoc_is_not_allowed() {
    let e = env();
    let p = policy(&e);
    let c = "eval \"$(cat <<'EOF'\nsudo id\nEOF\n)\"";
    assert_ne!(g(&p, c).0, "allow", "{c}");
}

// -- 15.6 approval context, completed ------------------------------------------------------------------------------

const PUSH: &str = "git push origin main";

fn remember(p: &PolicyConfig, c: &str) -> HashSet<String> {
    session_approval_entries(
        p,
        Some(&ShellTool as &dyn Tool),
        &shell(c),
        "builtin:git-push",
    )
    .into_iter()
    .collect()
}

#[test]
fn exported_variables_and_subshell_assignments_are_part_of_the_approval_context() {
    let e = env();
    let p = policy(&e);
    let approved = remember(&p, PUSH);
    for variant in [
        "export GIT_SSH_COMMAND='sh -c id'; git push origin main",
        "export X=1 && git push origin main",
        "declare -x X=1; git push origin main",
        "typeset -x X=1; git push origin main",
        "set -a; X=1; git push origin main",
        "(GIT_DIR=/tmp/o/.git; git push origin main)",
        "(X=1; git push origin main)",
    ] {
        assert_eq!(gate(&p, &approved, variant).0, "ask", "{variant}");
    }
}

#[test]
fn an_approved_exported_variant_covers_only_itself() {
    let e = env();
    let p = policy(&e);
    for variant in [
        "export X=1; git push origin main",
        "(X=1; git push origin main)",
    ] {
        let approved = remember(&p, variant);
        assert_eq!(gate(&p, &approved, variant), ("allow", None), "{variant}");
        assert_eq!(
            gate(&p, &approved, PUSH).0,
            "ask",
            "plain push rides on `{variant}`"
        );
    }
}

#[test]
fn nested_command_strings_and_source_lines_never_match_a_remembered_approval() {
    let e = env();
    let p = policy(&e);
    for variant in [
        "sh -c 'git push origin main'",
        "bash -c 'git push origin main'",
        "eval git push origin main",
        "eval \"git push origin main\"",
        "source x.sh; git push origin main",
        ". x.sh && git push origin main",
    ] {
        // approving the plain line does not cover them...
        let plain = remember(&p, PUSH);
        assert_eq!(gate(&p, &plain, variant).0, "ask", "{variant}");
        // ...and approving the line itself does not either: it asks again
        let own = remember(&p, variant);
        assert_eq!(gate(&p, &own, variant).0, "ask", "{variant} (own approval)");
    }
}

// -- 15.7 as-written paths everywhere ------------------------------------------------------------------------------------

#[cfg(unix)]
mod as_written {
    use super::*;

    #[test]
    fn a_glob_over_a_symlinked_rc_file_asks() {
        let e = env();
        std::fs::write(e.wd.join("rc.txt"), "").unwrap();
        std::os::unix::fs::symlink(e.wd.join("rc.txt"), e.home.join(".zshrc")).unwrap();
        let p = policy(&e);
        assert_eq!(
            g(&p, "echo x >> ~/.zsh[r]c"),
            ("ask", some("builtin:shell-startup"))
        );
    }

    #[test]
    fn dd_output_to_a_symlinked_rc_file_asks() {
        let e = env();
        std::fs::write(e.wd.join("rc.txt"), "").unwrap();
        std::os::unix::fs::symlink(e.wd.join("rc.txt"), e.home.join(".zshrc")).unwrap();
        let p = policy(&e);
        assert_eq!(
            g(&p, "dd if=/dev/zero of=~/.zshrc count=1"),
            ("ask", some("builtin:shell-startup"))
        );
    }

    #[test]
    fn dd_input_from_a_symlinked_credential_file_asks() {
        let e = env();
        std::fs::write(e.wd.join("notes.txt"), "x").unwrap();
        std::os::unix::fs::symlink(e.wd.join("notes.txt"), e.home.join(".netrc")).unwrap();
        let p = policy(&e);
        assert_eq!(
            g(&p, "dd if=~/.netrc of=/tmp/x"),
            ("ask", some("builtin:git-credentials"))
        );
    }

    #[test]
    fn a_glob_read_of_a_symlinked_credential_file_asks() {
        let e = env();
        std::fs::write(e.wd.join("notes.txt"), "x").unwrap();
        std::os::unix::fs::symlink(e.wd.join("notes.txt"), e.home.join(".netrc")).unwrap();
        let p = policy(&e);
        assert_eq!(
            g(&p, "cat ~/.net[r]c"),
            ("ask", some("builtin:git-credentials"))
        );
    }
}

// -- 15.8 pipe-to-shell long tail -----------------------------------------------------------------------------------------

fn assert_pipe_denied(cmds: &[&str]) {
    let e = env();
    let p = policy(&e);
    for c in cmds {
        assert!(pipe_flag(&e, c), "flag not set: {c}");
        assert_eq!(g(&p, c), ("deny", some("builtin:pipe-to-shell")), "{c}");
    }
}

#[test]
fn more_shells_are_covered() {
    let cmds: Vec<String> = [
        "ash", "mksh", "ksh93", "yash", "tcsh", "csh", "elvish", "nu", "xonsh",
    ]
    .iter()
    .map(|s| format!("curl x | {s}"))
    .collect();
    assert_pipe_denied(&cmds.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
fn versioned_interpreter_names_are_normalised() {
    assert_pipe_denied(&[
        "curl x | php8.2",
        "curl x | node22",
        "curl x | perl5.36",
        "curl x | ruby3.2",
        "curl x | lua5.4",
        "curl x | python3.12 -E -s",
        "curl x | python3.12",
    ]);
}

#[test]
fn powershell_reading_stdin_is_covered() {
    assert_pipe_denied(&[
        "curl x | pwsh -Command -",
        "curl x | pwsh -c -",
        "curl x | powershell -Command -",
    ]);
}

#[test]
fn more_wrappers_are_unwrapped() {
    assert_pipe_denied(&[
        "curl x | unbuffer -p sh",
        "curl x | unbuffer sh",
        "curl x | flock /tmp/l sh",
        "curl x | chroot / sh",
        "curl x | sg staff -c sh",
        "curl x | script -q /dev/null sh",
        "curl x | script -qc sh /dev/null",
    ]);
}

#[test]
fn reviewer_probes_with_ordinary_wrappers_and_paths() {
    // inferred from the reviewer's probe list (heredoc.txt); the spec does not name them
    assert_pipe_denied(&[
        "curl x | nice -n 5 bash",
        "curl x | env -i bash",
        "curl x | /opt/homebrew/bin/bash",
        "curl x | sh -",
        "curl x | { cat; } | sh",
        "curl x | bash -c 'bash'",
    ]);
}

#[test]
fn parallel_and_expect_are_unparsed_asks() {
    let e = env();
    let p = policy(&e);
    assert_ask_unparsed(
        &p,
        &[
            "parallel echo ::: a b",
            "expect -c 'spawn sh'",
            "expect script.exp",
        ],
    );
}

#[test]
fn script_files_and_programs_are_still_not_flagged() {
    let e = env();
    for c in [
        "curl x | python3.12 script.py",
        "curl x | node22 app.js",
        "curl x | flock /tmp/l jq .",
        "curl x | unbuffer jq .",
        "curl x | pwsh -File script.ps1",
    ] {
        assert!(!pipe_flag(&e, c), "{c}");
    }
}
