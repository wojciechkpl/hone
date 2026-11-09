#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 section 16 (round-3 review amendments), through the real `ShellTool` and `decide`
//! under built-ins: 16.1 `for` bindings, 16.2 `cd` to a non-literal target, 16.3 `$HOME`
//! is known, 16.4 heredoc parameter expansion, plus the `fp_var.txt` regression guard.
//!
//! Cases come from the round-3 reviewer's reproducers (`forb.txt`, `forb2.txt`, `cdvar.txt`,
//! `hd3.txt`, `hd4.txt`, `fp_var.txt`). The home directory is a temp dir passed explicitly.
//! Every case must ask or deny exactly as its literal form does; "ask" without an id means
//! the decision is Ask and the id is not pinned (it may be the matching rule or
//! `builtin:unparsed-command`).

use std::collections::HashSet;
use std::path::PathBuf;

use hone_proto::ToolCallRequest;
use hone_tools::policy_gate::{GateDecision, PolicyConfig, decide, resolve_permission_policy};
use hone_tools::{ShellTool, Tool};
use serde_json::json;

struct Env {
    _wd: tempfile::TempDir,
    _home: tempfile::TempDir,
    wd: PathBuf,
    home: PathBuf,
}

/// Workdir plus a home that contains `.ssh/id_x` (so globs over it match something).
fn env() -> Env {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let wd_p = std::fs::canonicalize(wd.path()).unwrap();
    let home_p = std::fs::canonicalize(home.path()).unwrap();
    std::fs::create_dir_all(home_p.join(".ssh")).unwrap();
    std::fs::write(home_p.join(".ssh/id_x"), "k").unwrap();
    Env {
        _wd: wd,
        _home: home,
        wd: wd_p,
        home: home_p,
    }
}

fn policy(e: &Env) -> PolicyConfig {
    resolve_permission_policy(&e.wd, Some(e.home.clone()), true)
        .unwrap()
        .unwrap()
}

fn g(p: &PolicyConfig, c: &str) -> (&'static str, Option<String>) {
    let call = ToolCallRequest {
        id: "1".into(),
        name: "shell".into(),
        arguments: json!({ "command": c }),
    };
    match decide(p, &HashSet::new(), Some(&ShellTool as &dyn Tool), &call) {
        GateDecision::Allow => ("allow", None),
        GateDecision::Deny { rule_id } => ("deny", Some(rule_id)),
        GateDecision::Ask { rule_id } => ("ask", Some(rule_id)),
    }
}

fn some(s: &str) -> Option<String> {
    Some(s.to_owned())
}

fn kind(p: &PolicyConfig, c: &str) -> &'static str {
    g(p, c).0
}

fn assert_ask(p: &PolicyConfig, cmds: &[&str]) {
    for c in cmds {
        assert_eq!(kind(p, c), "ask", "{c}");
    }
}

fn assert_exact(p: &PolicyConfig, cases: &[(&str, (&'static str, Option<&str>))]) {
    for (c, (k, id)) in cases {
        assert_eq!(g(p, c), (*k, id.map(str::to_owned)), "{c}");
    }
}

fn assert_not_allow(p: &PolicyConfig, cmds: &[&str]) {
    for c in cmds {
        assert_ne!(kind(p, c), "allow", "{c}");
    }
}

// -- 16.1 `for` bindings: substitute with the real op ---------------------------------------------

#[test]
fn for_values_are_substituted_with_the_commands_real_op_read() {
    let e = env();
    let p = policy(&e);
    assert_exact(
        &p,
        &[
            (
                "for h in ~; do cat \"$h/.ssh/id_x\"; done",
                ("ask", Some("builtin:ssh")),
            ),
            (
                "for x in .ssh; do cat ~/\"$x\"/id_x; done",
                ("ask", Some("builtin:ssh")),
            ),
            (
                "for f in ~/.ssh/id_x; do cat \"$f\"; done",
                ("ask", Some("builtin:ssh")),
            ),
            (
                "for f in .; do cat \"$f/.env\"; done",
                ("ask", Some("builtin:env-file")),
            ),
            (
                "for d in /; do cat \"${d}etc/hosts\"; done",
                ("allow", None),
            ),
            ("for x in a; do cat \"$x$x\"; done", ("allow", None)),
            ("for f in a; do cat \"$f\"; done", ("allow", None)),
        ],
    );
}

#[test]
fn for_values_are_substituted_with_the_commands_real_op_write() {
    let e = env();
    let p = policy(&e);
    assert_exact(
        &p,
        &[
            (
                "for f in ~/.bashrc; do cp evil \"$f\"; done",
                ("ask", Some("builtin:shell-startup")),
            ),
            (
                "for f in ~/.bashrc; do echo x >> \"$f\"; done",
                ("ask", Some("builtin:shell-startup")),
            ),
            (
                "for f in ~; do echo x >> \"$f/.bashrc\"; done",
                ("ask", Some("builtin:shell-startup")),
            ),
            (
                "for f in ~; do cp evil \"$f/.zshrc\"; done",
                ("ask", Some("builtin:shell-startup")),
            ),
            (
                "for f in .git/hooks/pre-commit; do cp evil \"$f\"; done",
                ("ask", Some("builtin:git-hooks")),
            ),
            (
                "for f in .hone/mcp.json; do tee \"$f\" < evil.json; done",
                ("ask", Some("builtin:hone-config")),
            ),
            (
                "for f in .git/config; do sed -i '' 's/a/b/' \"$f\"; done",
                ("ask", Some("builtin:git-hooks")),
            ),
        ],
    );
}

#[test]
fn a_literal_glob_in_the_for_list_is_expanded() {
    let e = env();
    let p = policy(&e);
    assert_exact(
        &p,
        &[
            (
                "for f in ~/.ssh/*; do cat \"$f\"; done",
                ("ask", Some("builtin:ssh")),
            ),
            (
                "for f in a b ~/.ssh/id_x; do cat \"$f\"; done",
                ("ask", Some("builtin:ssh")),
            ),
            ("for f in *.py; do python3 \"$f\"; done", ("allow", None)),
            ("for f in *; do cat \"$f\"; done", ("allow", None)),
        ],
    );
}

#[test]
fn nested_loops_substitute_the_combinations() {
    let e = env();
    let p = policy(&e);
    assert_exact(
        &p,
        &[(
            "for d in ~; do for x in .ssh; do cat \"$d/$x/id_x\"; done; done",
            ("ask", Some("builtin:ssh")),
        )],
    );
}

#[test]
fn the_for_list_words_are_not_path_subjects_of_their_own() {
    let e = env();
    let p = policy(&e);
    // the values are only used through substitution: echo reads nothing
    assert_exact(
        &p,
        &[
            (
                "for f in ~/.ssh/id_x; do echo \"$f\"; done",
                ("allow", None),
            ),
            ("for f in ~/.bashrc; do echo \"$f\"; done", ("allow", None)),
        ],
    );
}

#[test]
fn more_than_32_combinations_count_as_variable_derived() {
    let e = env();
    let p = policy(&e);
    let words = |n: usize| {
        (1..=n)
            .map(|i| format!("a{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    // single loop: 33 values (> 32) -> ask; 32 -> allow
    let over = format!("for f in {}; do cat \"$f\"; done", words(33));
    let at = format!("for f in {}; do cat \"$f\"; done", words(32));
    assert_eq!(kind(&p, &over), "ask", "33 combinations");
    assert_eq!(g(&p, &at), ("allow", None), "32 combinations");
    // nested loops: 6 x 6 = 36 combinations for the word "$a$b" -> ask; 5 x 6 = 30 -> allow
    let nested = |n: usize| {
        format!(
            "for a in {}; do for b in {}; do cat \"$a$b\"; done; done",
            words(n),
            words(6)
        )
    };
    assert_eq!(kind(&p, &nested(6)), "ask", "36 combinations");
    assert_eq!(g(&p, &nested(5)), ("allow", None), "30 combinations");
}

#[test]
fn a_binding_is_valid_only_inside_its_loop_body() {
    let e = env();
    let p = policy(&e);
    assert_ask(
        &p,
        &[
            "for f in x; do cat \"$f\"; done; cat \"$f\"",
            "for f in a; do :; done; f=~/.ssh/id_x; cat \"$f\"",
        ],
    );
}

#[test]
fn reassignment_makes_the_variable_derived_again() {
    let e = env();
    let p = policy(&e);
    for reassign in [
        "f=~/.ssh/id_x",
        "f+=x",
        "read -r f <<< ~/.ssh/id_x",
        "read f",
        "printf -v f '%s' ~/.ssh/id_x",
        "mapfile f",
        "readarray f",
        "declare f=b",
        "local f=b",
        "typeset f=b",
        "export f=b",
        "getopts ab f",
        "((f=1))",
        "unset f",
    ] {
        let c = format!("for f in a; do {reassign}; cat \"$f\"; done");
        assert_eq!(kind(&p, &c), "ask", "{c}");
    }
    assert_ask(
        &p,
        &["for f in ~/.ss?/id_x; do f=/etc/hosts; cat \"$f\"; done"],
    );
}

#[test]
fn variables_derived_from_a_loop_variable_and_non_literal_lists_ask() {
    let e = env();
    let p = policy(&e);
    assert_ask(
        &p,
        &[
            "for f in notes.txt; do g=$f; cat \"$g\"; done",
            "for f in $(ls ~/.ssh); do cat ~/.ssh/\"$f\"; done",
            "for f in \"$@\"; do cat \"$f\"; done",
        ],
    );
}

// -- 16.2 `cd` to a target that isn't literal --------------------------------------------------------

#[test]
fn relative_paths_after_a_non_literal_cd_count_as_variable_derived() {
    let e = env();
    let p = policy(&e);
    assert_ask(
        &p,
        &[
            "H=~; cd \"$H\" && cat .ssh/id_x",
            "cd \"$(dirname ~/.ssh/id_x)\" && cat id_x",
            "cd \"$(printf '%s' ~)\"; base64 .ssh/id_x",
            "D=..; cd $D/home && cat .ssh/id_x",
            "cd - && cat .ssh/id_x",
            "cd \"$OLDPWD\" && cat .ssh/id_x",
            "for d in ~; do cd \"$d\"; cat .ssh/id_x; done",
            "pushd \"$(echo x)\" >/dev/null; cat .ssh/id_x",
            "cd \"$X\" && echo x > out.txt",
        ],
    );
}

#[test]
fn a_literal_or_known_cd_target_is_tracked() {
    let e = env();
    let p = policy(&e);
    assert_exact(
        &p,
        &[
            ("cd ~ && cat .ssh/id_x", ("ask", Some("builtin:ssh"))),
            ("cd $HOME && cat .ssh/id_x", ("ask", Some("builtin:ssh"))),
            (
                "cd \"$HOME\" && cat .ssh/id_x",
                ("ask", Some("builtin:ssh")),
            ),
            (
                "pushd \"$HOME\" >/dev/null; cat .ssh/id_x",
                ("ask", Some("builtin:ssh")),
            ),
        ],
    );
}

#[test]
fn absolute_paths_after_a_non_literal_cd_are_unaffected() {
    let e = env();
    let p = policy(&e);
    let c = format!("cd \"$X\" && cat {}/notes.txt", e.wd.display());
    assert_eq!(g(&p, &c), ("allow", None));
}

#[test]
fn ordinary_commands_after_a_non_literal_cd_stay_allowed() {
    let e = env();
    let p = policy(&e);
    assert_exact(
        &p,
        &[
            (
                "cd \"$(git rev-parse --show-toplevel)\" && cargo test",
                ("allow", None),
            ),
            ("cd \"$DIR\" && cargo build", ("allow", None)),
            ("cd \"$DIR\" && echo done", ("allow", None)),
        ],
    );
}

// -- 16.3 $HOME is known ------------------------------------------------------------------------------------

#[test]
fn home_references_are_resolved_not_variable_derived() {
    let e = env();
    let p = policy(&e);
    assert_exact(
        &p,
        &[
            ("cat \"$HOME/notes.txt\"", ("allow", None)),
            ("cat \"${HOME}/notes.txt\"", ("allow", None)),
            ("cat ~/notes.txt", ("allow", None)),
            (". \"$HOME/.cargo/env\"", ("allow", None)),
            ("source \"$HOME/.nvm/nvm.sh\"", ("allow", None)),
        ],
    );
}

#[test]
fn a_home_credential_path_via_home_asks_under_its_own_rule() {
    let e = env();
    let p = policy(&e);
    assert_exact(
        &p,
        &[
            ("cat \"$HOME/.ssh/id_x\"", ("ask", Some("builtin:ssh"))),
            ("cat \"${HOME}/.ssh/id_x\"", ("ask", Some("builtin:ssh"))),
            ("cat $HOME/.netrc", ("ask", Some("builtin:git-credentials"))),
            (
                "echo x >> \"$HOME/.bashrc\"",
                ("ask", Some("builtin:shell-startup")),
            ),
            (
                "cp evil \"$HOME/.zshrc\"",
                ("ask", Some("builtin:shell-startup")),
            ),
        ],
    );
}

#[test]
fn other_unknown_variables_still_ask() {
    let e = env();
    let p = policy(&e);
    assert_ask(
        &p,
        &[
            "cp build/app \"$DEST\"",
            "rm -rf \"$TMPDIR/cache\"",
            "cat \"$OTHERHOME/notes.txt\"",
        ],
    );
}

// -- 16.4 heredoc parameter expansion is injection ------------------------------------------------------------

fn hd(head: &str, delim: &str, body: &str) -> String {
    format!("{head} <<{delim}\n{body}\nEOF")
}

#[test]
fn unquoted_heredoc_with_any_parameter_expansion_is_denied() {
    let e = env();
    let p = policy(&e);
    let pipe = ("deny", Some("builtin:pipe-to-shell"));
    assert_exact(
        &p,
        &[
            (&hd("bash", "EOF", "$X"), pipe),
            (&hd("bash", "EOF", "echo $X"), pipe),
            (&hd("bash", "EOF", "echo $1"), pipe),
            (&hd("bash", "EOF", "${X:-sudo id}"), pipe),
            (&hd("bash", "EOF", "sud$Xo id"), pipe),
            (&hd(". /dev/stdin", "EOF", "echo ${X}"), pipe),
            (&hd("source /dev/stdin", "EOF", "echo $X"), pipe),
            (&hd("sh -s", "EOF", "echo $HOME"), pipe),
            (&hd("bash", "EOF", "$(curl x)"), pipe),
            (&hd("bash", "EOF", "`curl x`"), pipe),
            (
                &format!("X='x; sudo id'; {}", hd("bash", "EOF", "echo $X")),
                pipe,
            ),
        ],
    );
}

#[test]
fn quoted_heredoc_is_unchanged_its_body_is_parsed_and_gated() {
    let e = env();
    let p = policy(&e);
    let sudo = ("deny", Some("builtin:sudo"));
    assert_exact(
        &p,
        &[
            (&hd("bash", "'EOF'", "echo hi"), ("allow", None)),
            (&hd("bash", "'EOF'", "echo $X"), ("allow", None)),
            (&hd("bash", "'EOF'", "sudo id"), sudo),
            (&hd(". /dev/stdin", "'EOF'", "sudo id"), sudo),
            (
                &hd("bash /dev/stdin", "'EOF'", "cat ~/.ssh/id_x"),
                ("ask", Some("builtin:ssh")),
            ),
            (
                &hd("bash", "'EOF'", "echo x >> ~/.bashrc"),
                ("ask", Some("builtin:shell-startup")),
            ),
            (
                &hd("bash", "'EOF'", "git push origin main"),
                ("ask", Some("builtin:git-push")),
            ),
            (&hd("bash", "\"EOF\"", "sudo id"), sudo),
            (&hd("bash", "\\EOF", "sudo id"), sudo),
        ],
    );
}

#[test]
fn escaped_dollars_and_backslashed_words_in_an_unquoted_body() {
    let e = env();
    let p = policy(&e);
    // `s\udo` is the word `sudo` to the inner shell
    assert_exact(
        &p,
        &[(
            &hd("bash", "EOF", "s\\udo id"),
            ("deny", Some("builtin:sudo")),
        )],
    );
    // an escaped `$(` reaches the inner shell as a substitution
    assert_not_allow(&p, &[&hd("bash", "EOF", "\\$(curl x)")]);
}

#[test]
fn heredoc_forms_and_positions_are_all_gated() {
    let e = env();
    let p = policy(&e);
    let sudo = ("deny", Some("builtin:sudo"));
    assert_exact(
        &p,
        &[
            ("bash <<-'EOF'\n\tsudo id\n\tEOF", sudo),
            ("bash 0<<'A'\nsudo id\nA", sudo),
            ("bash <<'EOF' && echo done\nsudo id\nEOF", sudo),
            ("bash <<'EOF' | cat\nsudo id\nEOF", sudo),
            ("bash <<'EOF' > /dev/null\nsudo id\nEOF", sudo),
            ("cat <<A; bash <<B\nx\nA\nsudo id\nB", sudo),
            ("bash <<A; cat <<B\nsudo id\nA\nx\nB", sudo),
            ("bash <<'EOF'\ncat <<'EOF'\nEOF\nsudo id\nEOF", sudo),
            ("bash <<'EOF'\nEOF\nsudo id", sudo),
            ("bash <<'EOF'\nbash <<'INNER'\nsudo id\nINNER\nEOF", sudo),
            (
                "git commit -m \"$(cat <<'EOF'\nmsg\nEOF\n)\"; sudo id",
                sudo,
            ),
            (
                "printf '%s\\n' 'sudo id' | bash",
                ("deny", Some("builtin:pipe-to-shell")),
            ),
            ("cat <<'X'\nsudo id\nX", ("allow", None)),
        ],
    );
    assert_not_allow(
        &p,
        &[
            "bash <<'A' <<'B'\necho hi\nA\nsudo id\nB",
            "bash <<< 'echo hi' <<'B'\nsudo id\nB",
            "bash 3<<'A'\nsudo id\nA",
            "{ bash; } <<'EOF'\nsudo id\nEOF",
            "(bash) <<'EOF'\nsudo id\nEOF",
            "bash <<'EOF'\n$'\\x73udo' id\nEOF",
            "bash <<'EOF'\neval \"$(printf 'sud%s id' o)\"\nEOF",
            "bash -c \"$(cat <<'EOF'\nsudo id\nEOF\n)\"",
            "eval \"$(cat <<'EOF'\nsudo id\nEOF\n)\"",
            "X='x; sudo id'; bash <<< \"echo $X\"",
        ],
    );
    assert_exact(
        &p,
        &[("bash /dev/stdin", ("ask", Some("builtin:unparsed-command")))],
    );
}

// -- regression guard: the fp_var.txt commands that are Allow stay Allow -----------------------------------

const FP_ALLOW: &[&str] = &[
    r####"git commit -m "$(cat <<'EOF'
fix: thing

Body
EOF
)""####,
    r####"git commit -m "fix: $MSG""####,
    r####"for f in *.py; do python3 "$f"; done"####,
    r####"for f in src/*.rs; do wc -l "$f"; done"####,
    r####"awk '{print $1}' data.txt"####,
    r####"awk "{print \$1}" data.txt"####,
    r####"sed "s/$OLD/$NEW/g" file.txt"####,
    r####"grep -rn "$PATTERN" src"####,
    r####"pytest -k "$K" tests"####,
    r####"cargo test $FILTER"####,
    r####"curl -H "Authorization: Bearer $TOKEN" https://api.example.com"####,
    r####"docker run -e FOO=$BAR alpine"####,
    r####"kill $(pgrep node)"####,
    r####"kill $PID"####,
    r####"sleep $N"####,
    r####"seq 1 $N"####,
    r####"mkdir -p "$OUT""####,
    r####"cd "$(git rev-parse --show-toplevel)""####,
    r####". "$HOME/.cargo/env""####,
    r####"source "$HOME/.nvm/nvm.sh""####,
    r####"ls $HOME"####,
    r####"cat "$HOME/notes.txt""####,
    r####"[[ -f "$f" ]] && echo yes"####,
    r####"test -f "$f""####,
    r####"echo "$PATH""####,
    r####"printf '%s\n' "$x""####,
    r####"wait $!"####,
    r####"exit $?"####,
    r####"python3 -c "print($X)""####,
    r####"node -e "console.log(process.env.X)""####,
    r####"git checkout "$BRANCH""####,
    r####"git log --since="$DATE""####,
    r####"npm run build -- --env=$ENV"####,
    r####"VERSION=$(cat VERSION) && echo $VERSION"####,
    r####"x=$(date +%s); echo $x"####,
    r####"go test ./... -run "$T""####,
    r####"make -j$(nproc)"####,
    r####"cmake -S . -B build -DCMAKE_BUILD_TYPE=$TYPE"####,
    r####"jq ".items[] | .name" data.json"####,
    r####"jq --arg n "$NAME" '.x = $n' data.json"####,
    r####"find . -name '*.tmp' -exec rm {} \;"####,
    r####"find . -name "$GLOB""####,
    r####"xargs -I{} echo {}"####,
    r####"tar czf out-$(date +%F).tgz src"####,
    r####"git diff HEAD~1 -- "$FILE""####,
    r####"while read -r line; do echo "$line"; done < input.txt"####,
    r####"time cargo build"####,
    r####"env | grep HONE"####,
    r####"diff <(sort a.txt) <(sort b.txt)"####,
    r####"ruby -e 'puts 1'"####,
    r####"bash -c 'echo $0' x"####,
    r####"sh -c 'for i in 1 2; do echo $i; done'"####,
];

#[test]
fn everyday_fp_var_commands_stay_allowed() {
    let e = env();
    let p = policy(&e);
    for c in FP_ALLOW {
        assert_eq!(g(&p, c), ("allow", None), "{c}");
    }
}
