#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 section 17 (round-4 review conditions): 17.1 when a `for` binding stops being trusted,
//! 17.2 an unknown directory stays unknown. Cases are the reviewer's `r4.txt` and `r4b.txt`,
//! driven through the real `ShellTool` and `decide` under built-ins, with a temp home that
//! holds `.ssh/id_x`.
//!
//! "Never Allow" cases assert only that the decision is not Allow (it may be the matching
//! rule, a Deny as the literal form would be, or `builtin:unparsed-command`).

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

fn env() -> Env {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let wd_p = std::fs::canonicalize(wd.path()).unwrap();
    let home_p = std::fs::canonicalize(home.path()).unwrap();
    std::fs::create_dir_all(home_p.join(".ssh")).unwrap();
    std::fs::write(home_p.join(".ssh/id_x"), "k").unwrap();
    std::fs::create_dir_all(wd_p.join("sub")).unwrap();
    std::fs::write(wd_p.join("sub/file"), "x").unwrap();
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

fn assert_not_allow(cmds: &[&str]) {
    let e = env();
    let p = policy(&e);
    for c in cmds {
        assert_ne!(g(&p, c).0, "allow", "{c}");
    }
}

fn assert_allow(cmds: &[&str]) {
    let e = env();
    let p = policy(&e);
    for c in cmds {
        assert_eq!(g(&p, c), ("allow", None), "{c}");
    }
}

fn assert_rule(cases: &[(&str, &str)]) {
    let e = env();
    let p = policy(&e);
    for (c, id) in cases {
        assert_eq!(g(&p, c), ("ask", Some((*id).to_owned())), "{c}");
    }
}

// -- 17.1 an inner loop rebinds the same name -----------------------------------------------------

#[test]
fn an_inner_loop_that_rebinds_the_same_name_invalidates_the_outer_binding() {
    assert_not_allow(&[
        "for f in a; do for f in ~/.ssh/id_x; do :; done; cat \"$f\"; done",
        "for f in ~/.ssh/id_x; do for f in a; do :; done; cat \"$f\"; done",
        "for f in a; do for f in ~/.bashrc; do :; done; cp evil \"$f\"; done",
    ]);
}

#[test]
fn nested_loops_with_different_names_keep_their_bindings() {
    assert_allow(&["for f in a; do for g in b; do cat \"$f/$g\"; done; cat \"$f\"; done"]);
    assert_rule(&[(
        "for f in ~/.ssh/id_x; do for g in b; do cat \"$f\"; done; done",
        "builtin:ssh",
    )]);
}

// -- 17.1 eval / source / . in the body -------------------------------------------------------------

#[test]
fn eval_source_or_dot_in_the_body_invalidates_every_binding() {
    assert_not_allow(&[
        "for f in a; do eval f=~/.ssh/id_x; cat \"$f\"; done",
        "for f in a; do eval 'f=~/.ssh/id_x'; cat \"$f\"; done",
        "for f in a; do source ./setf.sh; cat \"$f\"; done",
        "for f in a; do . ./setf.sh; cat \"$f\"; done",
        "for f in a; do eval f=~/.bashrc; cp evil \"$f\"; done",
    ]);
}

// -- 17.1 namerefs and array elements -----------------------------------------------------------------

#[test]
fn namerefs_invalidate_every_binding() {
    assert_not_allow(&[
        "for f in a; do declare -n r=f; r=~/.ssh/id_x; cat \"$f\"; done",
        "for f in a; do local -n r=f; r=~/.ssh/id_x; cat \"$f\"; done",
        "for f in a; do typeset -n r=f; r=~/.ssh/id_x; cat \"$f\"; done",
    ]);
}

#[test]
fn array_element_assignment_to_a_bound_name_invalidates_it() {
    assert_not_allow(&[
        "for f in a; do f[0]=~/.ssh/id_x; cat \"$f\"; done",
        "for f in a; do f[0]=.git/hooks/pre-commit; cp evil \"$f\"; done",
    ]);
}

// -- 17.1 function bodies see no bindings -----------------------------------------------------------

#[test]
fn function_bodies_never_see_loop_bindings() {
    assert_not_allow(&[
        "for f in a; do g() { cat \"$f\"; }; done; f=~/.ssh/id_x; g",
        "for f in a; do g() { cat \"$f\"; }; done; f=~/.ssh/id_x g",
        "for f in a; do g() { cp evil \"$f\"; }; done; f=~/.bashrc; g",
    ]);
}

// -- other reassignments the reviewer probed ---------------------------------------------------------------

#[test]
fn other_reassignments_still_invalidate() {
    assert_not_allow(&[
        "for f in a; do while read -r f; do cat \"$f\"; done < list.txt; done",
        "for f in a; do read -r x f <<< \"y $HOME/.ssh/id_x\"; cat \"$f\"; done",
        "for f in a; do let f=1; cat \"$f\"; done",
        "for f in a; do unset f; cat \"$f\"; done",
        "for f in a; do cat \"$f\" & f=~/.ssh/id_x; cat \"$f\"; done",
        "for f in ~/.ssh/id_x; { :; }; while true; do cat \"$f\"; break; done",
        "for f in a; { :; }; f=~/.ssh/id_x; while true; do cat \"$f\"; break; done",
    ]);
}

// -- guards: constructs that keep working as they do today (reviewer's "fine" list) ----------------

#[test]
fn while_until_and_case_in_the_body_keep_the_binding() {
    assert_rule(&[
        (
            "for f in ~/.ssh/id_x; do while true; do cat \"$f\"; break; done; done",
            "builtin:ssh",
        ),
        (
            "for f in ~/.ssh/id_x; do until false; do cat \"$f\"; break; done; done",
            "builtin:ssh",
        ),
    ]);
    assert_allow(&["for f in a; do case \"$f\" in a) cat \"$f\";; esac; done"]);
}

#[test]
fn the_word_done_in_quotes_comments_patterns_or_substitutions_does_not_end_the_loop() {
    assert_rule(&[
        (
            "for f in ~/.ssh/id_x; do case x in done) :;; esac; cat \"$f\"; done",
            "builtin:ssh",
        ),
        (
            "for f in ~/.ssh/id_x; do echo \"done\"; cat \"$f\"; done",
            "builtin:ssh",
        ),
        (
            "for f in ~/.ssh/id_x; do echo done; cat \"$f\"; done",
            "builtin:ssh",
        ),
        (
            "for f in ~/.ssh/id_x; do cat <<EOF\ndone\nEOF\ncat \"$f\"; done",
            "builtin:ssh",
        ),
        (
            "for f in ~/.ssh/id_x; do x=$(echo done); cat \"$f\"; done",
            "builtin:ssh",
        ),
    ]);
}

#[test]
fn c_style_select_and_glob_loops_are_unchanged() {
    assert_rule(&[(
        "select f in ~/.ssh/id_x; do cat \"$f\"; break; done",
        "builtin:ssh",
    )]);
    assert_allow(&["for f in *.py; do python3 \"$f\"; done"]);
    assert_not_allow(&[
        "for ((i=0;i<1;i++)); do cat \"$i\"; done",
        "for f; do cat \"$f\"; done",
    ]);
}

// -- 17.2 an unknown directory stays unknown --------------------------------------------------------------------

#[test]
fn a_cd_that_may_not_run_does_not_clear_an_unknown_directory() {
    assert_not_allow(&[
        "H=~; cd \"$H\" || cd /tmp; cat .ssh/id_x",
        "H=~; cd \"$H\"; false && cd /; cat .ssh/id_x",
        "H=~; cd \"$H\"; if false; then cd /; fi; cat .ssh/id_x",
    ]);
}

#[test]
fn a_later_cd_of_any_kind_does_not_clear_an_unknown_directory() {
    assert_not_allow(&[
        "H=~; cd \"$H\"; (cd /); cat .ssh/id_x",
        "H=~; cd \"$H\" && cd .ssh && cat id_x",
        "H=~; cd \"$H\"; cd ~/proj; cat ../.ssh/id_x",
        "H=~; cd \"$H\"; cd /; cat .ssh/id_x",
        "H=~; pushd \"$H\"; popd; cat notes.txt",
    ]);
}

#[test]
fn an_unknown_directory_covers_newlines_subshells_redirects_and_cd_spellings() {
    assert_not_allow(&[
        "H=~; cd \"$H\"\ncat .ssh/id_x",
        "H=~; (cd \"$H\"; cat .ssh/id_x)",
        "H=~; cd \"$H\"; cat < .ssh/id_x",
        "H=~; cd \"$H\"; cp evil .bashrc",
        "H=~; builtin cd \"$H\"; cat .ssh/id_x",
        "H=~; command cd \"$H\"; cat .ssh/id_x",
        "H=~; \\cd \"$H\"; cat .ssh/id_x",
    ]);
}

#[test]
fn a_cd_inside_a_subshell_never_changes_the_outer_directory() {
    assert_allow(&[
        "H=~; (cd \"$H\"); cat notes.txt",
        "x=$(cd \"$HOME\"; pwd); cat notes.txt",
    ]);
}

#[test]
fn absolute_paths_after_an_unknown_directory_are_unaffected() {
    assert_allow(&["H=~; cd \"$H\"; cat /etc/hosts"]);
}

#[test]
fn eval_strings_merge_their_directory_state_into_the_outer_line() {
    assert_not_allow(&[
        "H=~; eval 'cd \"$H\"'; cat .ssh/id_x",
        "eval \"cd $HOME/..\"; cat home/.ssh/id_x",
    ]);
}

#[test]
fn a_literal_cd_inside_eval_sets_the_outer_base_like_a_normal_cd() {
    let e = env();
    let p = policy(&e);
    let c = format!("eval 'cd {}'; cat id_x", e.home.join(".ssh").display());
    assert_eq!(g(&p, &c), ("ask", Some("builtin:ssh".to_owned())), "{c}");
}

// -- guards for 17.2 -------------------------------------------------------------------------------------------------

#[test]
fn ordinary_commands_after_an_unknown_directory_stay_allowed() {
    assert_allow(&[
        "H=~; cd \"$H\" && cargo test",
        "cd \"$(git rev-parse --show-toplevel)\" && cargo test",
    ]);
}

#[test]
fn a_relative_read_after_a_non_literal_cd_still_asks() {
    assert_not_allow(&["cd \"$(git rev-parse --show-toplevel)\" && cat README.md"]);
}

#[test]
fn a_plain_literal_cd_still_resolves_normally() {
    let e = env();
    let p = policy(&e);
    assert_eq!(g(&p, "cd sub && cat file"), ("allow", None));
    assert_eq!(
        g(&p, "cd sub && cat ../.env"),
        ("ask", Some("builtin:env-file".to_owned()))
    );
    assert_eq!(
        g(&p, "cd ~ && cat .ssh/id_x"),
        ("ask", Some("builtin:ssh".to_owned()))
    );
}

// -- 17.4 a line that defines a function never trusts `for` bindings -----------------------------------

#[test]
fn a_function_defined_before_the_loop_and_called_inside_it_is_not_trusted() {
    assert_not_allow(&[
        "g() { f=~/.ssh/id_x; }; for f in a; do g; cat \"$f\"; done",
        "function g { f=~/.ssh/id_x; }; for f in a; do g; cat \"$f\"; done",
        "g() { f=~/.bashrc; }; for f in a; do g; cp evil \"$f\"; done",
        "g() { read -r f < list.txt; }; for f in a; do g; cat \"$f\"; done",
        "function g() { read -r f < list.txt; }; for f in a; do g; cat \"$f\"; done",
        "g () { f=~/.ssh/id_x; }; for f in a; do g; cat \"$f\"; done",
    ]);
}

#[test]
fn the_function_keyword_form_defined_in_the_loop_is_not_trusted() {
    assert_not_allow(&[
        "for f in a; do function g { cp evil \"$f\"; }; done; f=~/.bashrc; g",
        "for f in a; do function g() { cp evil \"$f\"; }; done; f=~/.bashrc; g",
    ]);
}

#[test]
fn a_function_with_a_subshell_body_is_not_trusted() {
    assert_not_allow(&["for f in a; do g() ( cp evil \"$f\" ); done; f=~/.bashrc; g"]);
}

#[test]
fn any_loop_variable_reference_on_a_function_defining_line_is_variable_derived() {
    assert_not_allow(&[
        "g() { echo hi; }; for f in a; do cat \"$f\"; done",
        "for f in a; do cat \"$f\"; done; g() { echo hi; }",
    ]);
}

#[test]
fn sourcing_a_heredoc_inside_the_loop_is_not_trusted() {
    assert_not_allow(&[
        "for f in a; do source /dev/stdin <<'EOF'\nf=~/.ssh/id_x\nEOF\ncat \"$f\"; done",
    ]);
}

#[test]
fn a_line_that_defines_a_function_but_uses_no_loop_variable_is_unaffected() {
    assert_allow(&[
        "g() { echo hi; }; g",
        "function g { echo hi; }; g",
        "g () { echo hi; }; g",
        "g() ( echo hi ); g",
    ]);
    // and a loop on a line with no function keeps its binding
    assert_rule(&[("for f in ~/.ssh/id_x; do cat \"$f\"; done", "builtin:ssh")]);
}
