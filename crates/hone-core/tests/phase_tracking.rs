#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! K2 and K3 of `docs/plans/context-phase-events-spec.md`: which tool calls the
//! phase tracker treats as writes and as test runs.
//!
//! API required (new; the file fails to compile until it exists), defined in
//! `context.rs` and re-exported at the `hone_core` crate root (the `context`
//! module itself is private):
//!
//! ```ignore
//! #[derive(Debug, Clone, Copy, PartialEq, Eq)]
//! pub enum ToolPhaseKind { Read, Write, Test }
//!
//! /// K2/K3. `Write`: `write_file`, `edit_file`, `multi_edit`, `apply_patch`
//! /// (and the alias `apply_diff`). `Test`: the legacy name `run_tests`, or a
//! /// `shell` call whose `command` string runs a test runner in any segment
//! /// (see the spec for separators, wrappers and runners). Everything else,
//! /// including MCP tools, unknown names, a `shell` call without a string
//! /// `command`, and any `shell` call that is not a test run, is `Read`.
//! pub fn classify_tool_call(name: &str, args: &serde_json::Value) -> ToolPhaseKind;
//!
//! impl ContextManager {
//!     /// Like `notify_tool_call(name)` but classified with the call's arguments.
//!     pub fn notify_tool_use(&mut self, name: &str, args: &serde_json::Value);
//! }
//! ```
//!
//! `ContextManager::notify_tool_call(name)` keeps working: it classifies by name
//! only (`shell` without arguments is a read; `run_tests` is a test; the write
//! names are writes).

use hone_core::{ContextConfig, ContextManager, TaskPhase, ToolPhaseKind, classify_tool_call};
use serde_json::{Value, json};

fn shell(cmd: &str) -> Value {
    json!({ "command": cmd })
}

fn kind_of_shell(cmd: &str) -> ToolPhaseKind {
    classify_tool_call("shell", &shell(cmd))
}

// ---- K2: the write tools -------------------------------------------------------

#[test]
fn test_k2_real_write_tools_should_classify_as_writes() {
    for name in [
        "write_file",
        "edit_file",
        "multi_edit",
        "apply_patch",
        "apply_diff",
    ] {
        assert_eq!(
            classify_tool_call(name, &json!({})),
            ToolPhaseKind::Write,
            "{name}"
        );
    }
}

#[test]
fn test_k2_every_other_tool_should_classify_as_a_read() {
    for name in [
        "read_file",
        "grep",
        "glob",
        "git_status",
        "git_diff",
        "list_dir",
        "web_fetch",
        "mcp__github__edit_file",
        "mcp__fs__write_file",
        "some_unknown_tool",
        "",
        "Write_File",
    ] {
        assert_eq!(
            classify_tool_call(name, &json!({})),
            ToolPhaseKind::Read,
            "{name:?}"
        );
    }
}

// ---- K3: shell test runs ---------------------------------------------------------

#[test]
fn test_k3_each_test_runner_should_classify_as_a_test() {
    for cmd in [
        "cargo test",
        "cargo test -p foo --lib",
        "cargo nextest run",
        "pytest",
        "pytest -x tests/",
        "py.test tests/",
        "python -m pytest",
        "python3 -m pytest -q",
        "python -m unittest",
        "python3 -m unittest discover",
        "npm test",
        "npm run test",
        "npm run test:unit",
        "pnpm test",
        "pnpm run test-ci",
        "yarn test",
        "bun test",
        "deno test",
        "jest",
        "vitest run",
        "go test ./...",
        "mvn test",
        "gradle test",
        "./gradlew test",
        "make test",
        "flutter test",
        "dart test",
        "ctest",
        "rspec spec/",
        "phpunit",
        "tox",
        "nox",
        "mix test",
        "swift test",
        "dotnet test",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd}");
    }
}

#[test]
fn test_k3_leading_assignments_and_wrappers_should_be_ignored() {
    for cmd in [
        "RUST_LOG=debug cargo nextest run",
        "A=1 B=2 cargo test",
        "env FOO=1 cargo test",
        "env cargo test",
        "time pytest",
        "time cargo test",
        "uv run pytest -x",
        "poetry run pytest",
        "pdm run pytest",
        "npx jest",
        "bunx vitest",
        "pnpm exec vitest",
        "bundle exec rspec",
        "time env FOO=1 uv run pytest",
        "FOO=bar poetry run python -m pytest",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd}");
    }
}

#[test]
fn test_k3_any_segment_may_run_the_tests() {
    for cmd in [
        "cd crates/x && cargo test -p foo",
        "cargo build && cargo test",
        "make build; cargo test",
        "false || pytest",
        "cat log | pytest",
        "echo start\ncargo test",
        "cd app\nnpm test\necho done",
        "  cargo test  ",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd:?}");
    }
}

#[test]
fn test_k3_commands_that_only_mention_a_runner_should_not_count() {
    for cmd in [
        r#"echo "cargo test""#,
        "grep -r pytest .",
        "cargo build",
        "cargo check",
        "cargo clippy",
        "cat tests/test.py",
        "ls tests",
        "git status",
        "npm install",
        "npm run build",
        "python script.py",
        "python -m pip install pytest",
        "pip install pytest",
        "make build",
        "go build ./...",
        "echo pytest",
        "",
        "   ",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Read, "{cmd:?}");
    }
}

#[test]
fn test_k3_legacy_run_tests_name_should_stay_a_test() {
    assert_eq!(
        classify_tool_call("run_tests", &json!({})),
        ToolPhaseKind::Test
    );
}

#[test]
fn test_k3_shell_without_a_usable_command_should_be_a_read() {
    for args in [
        json!({}),
        json!({ "command": 5 }),
        json!({ "command": null }),
        json!(null),
        json!("cargo test"),
    ] {
        assert_eq!(
            classify_tool_call("shell", &args),
            ToolPhaseKind::Read,
            "{args}"
        );
    }
}

#[test]
fn test_k3_only_the_shell_tool_runs_commands() {
    assert_eq!(
        classify_tool_call("read_file", &shell("cargo test")),
        ToolPhaseKind::Read,
        "a `command` argument on another tool does not make it a test run"
    );
}

// ---- the tracker itself ----------------------------------------------------------

fn manager() -> ContextManager {
    ContextManager::with_config(ContextConfig::default())
}

#[test]
fn test_k3_notify_tool_use_should_move_to_verification_on_a_shell_test_run() {
    let mut ctx = manager();
    ctx.notify_tool_use("read_file", &json!({ "path": "a.rs" }));
    assert_eq!(ctx.current_phase(), TaskPhase::Exploration);
    ctx.notify_tool_use("shell", &shell("cd x && cargo test"));
    assert_eq!(ctx.current_phase(), TaskPhase::Verification);
}

#[test]
fn test_k3_notify_tool_use_should_ignore_a_shell_call_that_is_not_a_test_run() {
    let mut ctx = manager();
    ctx.notify_tool_use("shell", &shell("cargo build"));
    ctx.notify_tool_use("shell", &shell(r#"echo "cargo test""#));
    assert_eq!(ctx.current_phase(), TaskPhase::Exploration);
}

#[test]
fn test_k2_notify_tool_use_should_move_to_editing_on_apply_patch() {
    let mut ctx = manager();
    ctx.notify_tool_use("apply_patch", &json!({ "patch": "--- a\n+++ b\n" }));
    assert_eq!(ctx.current_phase(), TaskPhase::Editing);
}

#[test]
fn test_k4_notify_tool_call_by_name_should_keep_working() {
    let mut ctx = manager();
    ctx.notify_tool_call("apply_patch");
    assert_eq!(ctx.current_phase(), TaskPhase::Editing);

    let mut ctx = manager();
    ctx.notify_tool_call("run_tests");
    assert_eq!(ctx.current_phase(), TaskPhase::Verification);

    let mut ctx = manager();
    ctx.notify_tool_call("shell"); // no arguments: a read
    assert_eq!(ctx.current_phase(), TaskPhase::Exploration);
}

#[test]
fn test_k4_three_reads_after_verification_should_return_to_exploration() {
    let mut ctx = manager();
    ctx.notify_tool_use("shell", &shell("pytest"));
    assert_eq!(ctx.current_phase(), TaskPhase::Verification);
    for _ in 0..3 {
        ctx.notify_tool_use("read_file", &json!({}));
    }
    assert_eq!(ctx.current_phase(), TaskPhase::Exploration);
}

// ---- K3a: the matcher on real commands (review round 1) ---------------------------------

#[test]
fn test_k3a_separators_inside_quotes_should_not_split_a_command() {
    for cmd in [
        r#"git commit -am "fix test; cargo test passes""#,
        "echo 'step 1; pytest -q'",
        r#"echo "a && cargo test""#,
        "echo 'a | pytest'",
        "git commit -m 'run cargo test || fix'",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Read, "{cmd:?}");
    }
    // a real separator next to quoted text still splits
    for cmd in [
        r#"git commit -am "fix; done" && cargo test"#,
        r#"cd "my dir" && cargo test"#,
        "echo 'a;b'; pytest -q",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd:?}");
    }
}

#[test]
fn test_k3a_shell_dash_c_should_be_matched_on_its_quoted_script() {
    for cmd in [
        r#"bash -c "cargo test -p foo""#,
        "sh -c 'cd x && pytest'",
        r#"zsh -c "npm test""#,
        "bash -c 'FOO=1 cargo nextest run'",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd:?}");
    }
    for cmd in [
        r#"bash -c "echo cargo test""#,
        "sh -c 'cargo build'",
        r#"zsh -c "ls tests""#,
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Read, "{cmd:?}");
    }
}

#[test]
fn test_k3a_timeout_nice_and_stdbuf_wrappers_should_be_skipped() {
    for cmd in [
        "timeout 600 cargo test",
        "timeout 10m pytest -x",
        "nice cargo test",
        "nice -n 10 pytest",
        "stdbuf -oL cargo test",
        "stdbuf -o0 -e0 pytest",
        "time timeout 60 nice -n 5 cargo test",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd:?}");
    }
}

#[test]
fn test_k3a_runner_launcher_options_should_be_skipped() {
    for cmd in [
        "npx --yes jest",
        "npx -y jest --ci",
        "bunx --bun vitest",
        "uv run --with pytest pytest",
        "uv run --frozen pytest -q",
        "uv run --with pytest python -m pytest",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd:?}");
    }
}

#[test]
fn test_k3a_path_qualified_runners_should_match_by_basename() {
    for cmd in [
        ".venv/bin/pytest -q",
        "./node_modules/.bin/jest",
        "/usr/local/bin/cargo test",
        "python3.12 -m pytest",
        "python3.11 -m unittest discover",
        "./.venv/bin/python -m pytest",
        "/usr/bin/env pytest",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd:?}");
    }
    for cmd in [
        ".venv/bin/pip install pytest",
        "./scripts/build.sh",
        "python3.12 script.py",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Read, "{cmd:?}");
    }
}

#[test]
fn test_k3a_script_runners_should_match_test_scripts() {
    for cmd in [
        "npm t",
        "npm test -- --watch",
        "npm run test:unit",
        "yarn test --ci",
        "yarn run test:e2e",
        "pnpm test",
        "pnpm run test:unit",
        "bun run test:unit",
        "bun run test",
        "cargo t",
        "cargo t -p foo",
        "just test",
        "bazel test //...",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Test, "{cmd:?}");
    }
    for cmd in [
        "npm run build",
        "yarn build",
        "yarn run lint",
        "pnpm run build",
        "bun run build",
        "just build",
        "bazel build //...",
        "cargo b",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Read, "{cmd:?}");
    }
}

#[test]
fn test_k3a_a_runner_that_only_builds_or_lists_tests_should_be_a_read() {
    for cmd in [
        "cargo test --no-run",
        "cargo test --no-run -p foo",
        "cargo test -- --list",
        "cargo test -p foo -- --list --format terse",
        "pytest --collect-only",
        "pytest --collect-only -q tests/",
        "pytest --co",
        "cd app && cargo test --no-run",
    ] {
        assert_eq!(kind_of_shell(cmd), ToolPhaseKind::Read, "{cmd:?}");
    }
    // a later segment that runs the tests wins over a listing one
    assert_eq!(
        kind_of_shell("cargo test --no-run && cargo test"),
        ToolPhaseKind::Test
    );
    assert_eq!(
        kind_of_shell("pytest --collect-only; pytest -x"),
        ToolPhaseKind::Test
    );
}
