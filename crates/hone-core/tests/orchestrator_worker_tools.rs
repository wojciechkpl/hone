#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Contract tests W1, W2, W4-W8 of `docs/plans/worker-tools-spec.md`:
//! orchestrator workers get the session's tools, filtered by their agent
//! definition, under the session's approval policy / mode / policy / sandbox /
//! hooks / audit log.

mod worker_tools_support;

use std::sync::Arc;
use std::time::Duration;

use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, PolicyContext, PolicyRule};
use hone_core::egress_log::EgressLog;
use hone_core::hooks::{HookRegistry, HookSpec};
use hone_core::{AgentDef, AgentMode, AgentRegistry, Orchestrator, PolicyConfig, WorkerStatus};
use hone_proto::{AgentEvent, StepAction, ToolApprovalPolicy};
use hone_sandbox::SandboxPolicy;
use serde_json::{Value, json};

use worker_tools_support::*;

fn ask_risky() -> ToolApprovalPolicy {
    ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    }
}

fn custom_agent(name: &str, tools: &[&str]) -> AgentDef {
    let list = tools.join(", ");
    AgentDef::from_markdown(&format!(
        "---\nname: {name}\ndescription: custom {name}\ntools: [{list}]\n---\n\nYou are {name}.\n"
    ))
    .expect("agent def parses")
}

// ---------------------------------------------------------------------------
// W1: workers get tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w1_rust_expert_worker_when_session_has_tools_should_send_exactly_its_six_tools() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W1-RUST"]);
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W1-RUST", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;

    assert!(matches!(out.workers[0].status, WorkerStatus::Completed));
    assert_eq!(
        provider.worker_tools("W1-RUST"),
        sorted(&[
            "read_file",
            "edit_file",
            "write_file",
            "shell",
            "grep",
            "glob"
        ]),
        "web_fetch / git_commit must not leak to rust-expert"
    );
}

#[tokio::test]
async fn test_w1_code_reviewer_worker_when_session_has_tools_should_send_only_read_grep_glob_shell()
{
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W1-REVIEW"]);
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W1-REVIEW", "code-reviewer")]),
        None,
        proceed_all(),
    )
    .await;

    assert_eq!(
        provider.worker_tools("W1-REVIEW"),
        sorted(&["read_file", "grep", "glob", "shell"]),
        "a reviewer must not get edit_file / write_file"
    );
}

#[tokio::test]
async fn test_w1_worker_when_agent_tools_list_is_empty_should_get_the_whole_session_registry() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W1-EMPTY"]);
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    let mut registry = AgentRegistry::new();
    registry.register(custom_agent("open-agent", &[]));

    let orch = Orchestrator::new(config, registry, 4);
    run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W1-EMPTY", "open-agent")]),
        None,
        proceed_all(),
    )
    .await;

    assert_eq!(
        provider.worker_tools("W1-EMPTY"),
        sorted(&SESSION_TOOL_NAMES)
    );
}

#[tokio::test]
async fn test_w1_worker_when_agent_name_is_unknown_should_get_the_whole_session_registry() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W1-UNKNOWN"]);
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, AgentRegistry::new(), 4);
    run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W1-UNKNOWN", "no-such-agent")]),
        None,
        proceed_all(),
    )
    .await;

    assert_eq!(
        provider.worker_tools("W1-UNKNOWN"),
        sorted(&SESSION_TOOL_NAMES)
    );
}

#[tokio::test]
async fn test_w1_worker_should_actually_execute_its_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W1-EXEC"]);
    provider.script(
        "W1-EXEC",
        vec![
            tool_call("e1", "read_file", json!({ "path": "a.rs" })),
            tool_call("e2", "grep", json!({ "pattern": "x" })),
        ],
    );
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W1-EXEC", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;

    let mut ran = invoked(&log);
    ran.sort();
    assert_eq!(ran, vec!["grep".to_owned(), "read_file".to_owned()]);
    assert_eq!(call_end(&out.events, "e1").map(|(_, err)| err), Some(false));
    assert_eq!(out.workers[0].response, "done-W1-EXEC");
}

#[tokio::test]
async fn test_w1_worker_when_model_calls_a_tool_outside_its_list_should_not_run_it() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W1-OUT"]);
    provider.script(
        "W1-OUT",
        vec![
            tool_call("o1", "write_file", json!({ "path": "x" })),
            tool_call("o2", "read_file", json!({ "path": "x" })),
        ],
    );
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W1-OUT", "code-reviewer")]),
        None,
        proceed_all(),
    )
    .await;

    // The reviewer's own tool ran; the write it is not allowed did not.
    assert_eq!(invoked(&log), vec!["read_file".to_owned()]);
}

// ---------------------------------------------------------------------------
// W2: the lead keeps no tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w2_lead_when_decomposing_and_merging_should_send_no_tool_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W2-WORKER"]);
    *provider.decompose_json.lock().unwrap() = r#"{"goal":"g","subtasks":[{"id":"1","description":"W2-WORKER","agent":"rust-expert","context":"","depends_on":[]}]}"#.to_owned();
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let mut orch = Orchestrator::new(config, builtin_registry(), 4);
    let (tx, _rx) = tokio::sync::mpsc::channel(256);
    orch.run("do it", tx).await.expect("run");

    let decompose = provider.requests_of(|k| *k == Kind::Decompose);
    let merge = provider.requests_of(|k| *k == Kind::Merge);
    assert_eq!(decompose.len(), 1);
    assert_eq!(merge.len(), 1);
    assert!(decompose[0].tools.is_empty(), "{:?}", decompose[0].tools);
    assert!(merge[0].tools.is_empty(), "{:?}", merge[0].tools);
    // ... while the worker in the same run did get tools (W1), so the test
    // fails today rather than passing vacuously.
    assert_eq!(
        provider.worker_tools("W2-WORKER"),
        sorted(&[
            "read_file",
            "edit_file",
            "write_file",
            "shell",
            "grep",
            "glob"
        ])
    );
}

// ---------------------------------------------------------------------------
// W4: approvals follow the session
// ---------------------------------------------------------------------------

fn shell_script(provider: &ScriptedProvider, marker: &str, id: &str) {
    provider.script(marker, vec![tool_call(id, "shell", json!({ "cmd": id }))]);
}

#[tokio::test]
async fn test_w4_worker_when_ask_risky_with_channel_and_proceed_should_pause_then_run() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W4-PROCEED"]);
    shell_script(&provider, "W4-PROCEED", "p1");
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W4-PROCEED", "rust-expert")]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    assert_eq!(paused_ids(&out.events), vec!["p1".to_owned()]);
    assert_eq!(invoked(&log), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w4_worker_when_ask_risky_with_channel_and_deny_should_not_run_and_carry_on() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W4-DENY"]);
    shell_script(&provider, "W4-DENY", "d1");
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W4-DENY", "rust-expert")]),
        Some(step_tx),
        Box::new(|_| StepAction::Deny("not now".to_owned())),
    )
    .await;

    assert_eq!(paused_ids(&out.events), vec!["d1".to_owned()]);
    assert!(invoked(&log).is_empty(), "denied call must not run");
    let (result, is_error) = call_end(&out.events, "d1").expect("ToolCallEnd");
    assert!(is_error);
    assert_eq!(result["error"], "denied by user");
    assert!(matches!(out.workers[0].status, WorkerStatus::Completed));
    assert_eq!(out.workers[0].response, "done-W4-DENY");
}

#[tokio::test]
async fn test_w4_worker_when_ask_risky_with_channel_and_skip_should_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W4-SKIP"]);
    shell_script(&provider, "W4-SKIP", "s1");
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W4-SKIP", "rust-expert")]),
        Some(step_tx),
        Box::new(|_| StepAction::Skip),
    )
    .await;

    assert_eq!(paused_ids(&out.events), vec!["s1".to_owned()]);
    assert!(invoked(&log).is_empty());
}

#[tokio::test]
async fn test_w4_worker_when_ask_risky_without_channel_should_deny_and_carry_on() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W4-NOCHAN"]);
    shell_script(&provider, "W4-NOCHAN", "n1");
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = None;

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W4-NOCHAN", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;

    assert!(
        paused_ids(&out.events).is_empty(),
        "no menu nobody can answer"
    );
    assert!(invoked(&log).is_empty());
    let (result, is_error) = call_end(&out.events, "n1").expect("ToolCallEnd");
    assert!(is_error);
    assert_eq!(result["error"], "denied: no approval channel");
    assert!(matches!(out.workers[0].status, WorkerStatus::Completed));
}

#[tokio::test]
async fn test_w4_worker_when_policy_is_auto_should_run_without_pause() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W4-AUTO"]);
    shell_script(&provider, "W4-AUTO", "a1");
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ToolApprovalPolicy::Auto;
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W4-AUTO", "rust-expert")]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    assert!(paused_ids(&out.events).is_empty());
    assert_eq!(invoked(&log), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w4_worker_when_tool_is_always_allowed_should_run_without_pause() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W4-ALWAYS"]);
    shell_script(&provider, "W4-ALWAYS", "al1");
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ToolApprovalPolicy::AskRisky {
        always_allowed: vec!["shell".to_owned()],
    };
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W4-ALWAYS", "rust-expert")]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    assert!(paused_ids(&out.events).is_empty());
    assert_eq!(invoked(&log), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w4_worker_when_tool_is_read_only_under_ask_risky_should_run_without_pause() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W4-RO"]);
    provider.script("W4-RO", vec![tool_call("r1", "read_file", json!({}))]);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W4-RO", "rust-expert")]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    assert!(paused_ids(&out.events).is_empty());
    assert_eq!(invoked(&log), vec!["read_file".to_owned()]);
}

// ---------------------------------------------------------------------------
// W5: mode follows the session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w5_worker_when_session_is_auto_approve_should_run_risky_call_without_pause() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W5-AUTOAPPROVE"]);
    shell_script(&provider, "W5-AUTOAPPROVE", "aa1");
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);
    config.mode = AgentMode::AutoApprove;

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W5-AUTOAPPROVE", "rust-expert")]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    assert!(paused_ids(&out.events).is_empty());
    assert_eq!(invoked(&log), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w5_worker_when_session_is_auto_approve_should_still_obey_policy_deny() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W5-AA-POLICY"]);
    shell_script(&provider, "W5-AA-POLICY", "ap1");
    let mut config = lead_config(
        Arc::clone(&provider),
        session_tools_with_shell_subject(&log, Some("git push")),
        dir.path(),
    );
    config.approval_policy = ask_risky();
    config.mode = AgentMode::AutoApprove;
    config.policy = Some(deny_policy(dir.path(), "git push*", "no-push"));

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W5-AA-POLICY", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;

    assert!(invoked(&log).is_empty());
    let (result, is_error) = call_end(&out.events, "ap1").expect("ToolCallEnd");
    assert!(is_error);
    assert_eq!(result["error"], "denied by policy");
}

#[tokio::test]
async fn test_w5_worker_when_session_is_plan_should_get_only_read_only_tools_and_write_no_plan() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W5-PLAN"]);
    // The model tries a write and a shell call anyway, then a read.
    provider.script(
        "W5-PLAN",
        vec![
            tool_call("w1", "write_file", json!({ "path": "x" })),
            tool_call("w2", "shell", json!({ "cmd": "rm" })),
            tool_call("w3", "read_file", json!({ "path": "x" })),
        ],
    );
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.mode = AgentMode::Plan;

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W5-PLAN", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;

    // rust-expert's list narrowed to the read-only tools among them.
    assert_eq!(
        provider.worker_tools("W5-PLAN"),
        sorted(&["read_file", "grep", "glob"])
    );
    assert_eq!(invoked(&log), vec!["read_file".to_owned()]);
    // The worker runs in Build: no plan file, no PlanSaved.
    assert!(
        !out.events
            .iter()
            .any(|e| matches!(e, AgentEvent::PlanSaved { .. })),
        "a worker must not save a plan"
    );
    assert!(
        !dir.path().join(".hone/plans").exists(),
        "a worker must not write a plan file"
    );
}

#[tokio::test]
async fn test_w5_worker_when_session_is_build_should_keep_write_tools() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W5-BUILD"]);
    provider.script("W5-BUILD", vec![tool_call("b1", "write_file", json!({}))]);
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.mode = AgentMode::Build;

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W5-BUILD", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;

    assert_eq!(invoked(&log), vec!["write_file".to_owned()]);
}

// ---------------------------------------------------------------------------
// W6: policy, sandbox, hooks and audit log follow the session
// ---------------------------------------------------------------------------

fn deny_policy(workdir: &std::path::Path, pattern: &str, id: &str) -> PolicyConfig {
    let context = PolicyContext::for_platform(workdir.to_path_buf(), None);
    let rules = vec![PolicyRule::command(pattern, Decision::Deny).with_id(id)];
    let compiled = CompiledPolicy::compile(&context, &[], &rules, &[]).expect("policy compiles");
    PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: vec![],
        builtins: false,
    }
}

#[tokio::test]
async fn test_w6_worker_when_policy_denies_the_call_should_block_it() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W6-POLICY"]);
    shell_script(&provider, "W6-POLICY", "pl1");
    let mut config = lead_config(
        Arc::clone(&provider),
        session_tools_with_shell_subject(&log, Some("git push")),
        dir.path(),
    );
    config.policy = Some(deny_policy(dir.path(), "git push*", "no-push"));

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W6-POLICY", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;

    assert!(invoked(&log).is_empty());
    let (result, is_error) = call_end(&out.events, "pl1").expect("ToolCallEnd");
    assert!(is_error);
    assert_eq!(result["error"], "denied by policy");
    assert_eq!(result["rule"], "no-push");
}

#[tokio::test]
async fn test_w6_worker_when_session_has_pre_tool_use_hook_should_run_it_for_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let marker = dir.path().join("hook-ran");
    let provider = ScriptedProvider::new(&["W6-HOOK"]);
    shell_script(&provider, "W6-HOOK", "h1");
    let mut hooks = HookRegistry::new();
    hooks.register(HookSpec {
        name: "deny-shell".to_owned(),
        event: "PreToolUse".to_owned(),
        command: format!(
            r#"cat > /dev/null && touch '{}' && echo '{{"deny":true,"message":"hook says no","mutated_input":null}}'"#,
            marker.display()
        ),
        timeout_ms: 5000,
        tool_name_pattern: Some("shell".to_owned()),
    });
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.hooks = Some(Arc::new(hooks));

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W6-HOOK", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;

    assert!(
        marker.exists(),
        "the PreToolUse hook must run for the worker"
    );
    assert!(invoked(&log).is_empty(), "the hook denied the call");
}

#[tokio::test]
async fn test_w6_worker_when_session_has_audit_log_should_record_the_workers_calls() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let log_path = dir.path().join("audit.jsonl");
    let egress = Arc::new(tokio::sync::Mutex::new(EgressLog::new(log_path.clone())));
    let provider = ScriptedProvider::new(&["W6-AUDIT"]);
    provider.script(
        "W6-AUDIT",
        vec![tool_call("au1", "read_file", json!({ "path": "f" }))],
    );
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.egress_log = Some(Arc::clone(&egress));

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W6-AUDIT", "rust-expert")]),
        None,
        proceed_all(),
    )
    .await;
    egress.lock().await.flush().unwrap();

    let entries: Vec<Value> = std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(
        entries.iter().any(|e| e["tool"] == "read_file"),
        "the worker's read_file call must be in the audit log: {entries:?}"
    );
}

#[tokio::test]
async fn test_w6_worker_should_use_session_sandbox_policy_not_its_agent_definitions_profile() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W6-SANDBOX"]);
    shell_script(&provider, "W6-SANDBOX", "sb1");
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.sandbox_policy = Arc::new(SandboxPolicy {
        allow_env: vec!["SESSION_SANDBOX_MARKER".to_owned()],
        ..SandboxPolicy::permissive()
    });
    // An agent definition carrying its own, different sandbox profile.
    let mut boxed = custom_agent("boxed", &["shell"]);
    boxed.sandbox = Some(hone_core::agent_def::SandboxProfile {
        mode: hone_sandbox::SandboxMode::Manual,
        allow_read: vec![],
        allow_write: vec![],
    });
    let mut registry = AgentRegistry::new();
    registry.register(boxed);

    let orch = Orchestrator::new(config, registry, 4);
    run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W6-SANDBOX", "boxed")]),
        None,
        proceed_all(),
    )
    .await;

    let calls = log.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].sandbox_allow_env,
        vec!["SESSION_SANDBOX_MARKER".to_owned()],
        "the tool must see the session's sandbox policy"
    );
}

// ---------------------------------------------------------------------------
// W7: parallel workers ask one at a time
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_w7_parallel_workers_when_both_need_approval_should_never_have_two_prompts_out() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W7-A", "W7-B"]);
    provider.script(
        "W7-A",
        vec![tool_call("wa1", "shell", json!({ "who": "a" }))],
    );
    provider.script(
        "W7-B",
        vec![tool_call("wb1", "shell", json!({ "who": "b" }))],
    );
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![
            subtask("1", "W7-A", "rust-expert"),
            subtask("2", "W7-B", "rust-expert"),
        ]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    let mut ids = paused_ids(&out.events);
    ids.sort();
    assert_eq!(
        ids,
        vec!["wa1".to_owned(), "wb1".to_owned()],
        "both workers ask"
    );
    assert_eq!(
        out.overlapping_pauses, 0,
        "a second ToolCallPaused arrived before the first was answered: {:?}",
        out.events
    );
}

#[tokio::test(start_paused = true)]
async fn test_w7_parallel_workers_when_one_is_denied_should_not_deny_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W7-A", "W7-B"]);
    provider.script(
        "W7-A",
        vec![tool_call("wa1", "shell", json!({ "who": "a" }))],
    );
    provider.script(
        "W7-B",
        vec![tool_call("wb1", "shell", json!({ "who": "b" }))],
    );
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![
            subtask("1", "W7-A", "rust-expert"),
            subtask("2", "W7-B", "rust-expert"),
        ]),
        Some(step_tx),
        Box::new(|id| {
            if id == "wa1" {
                StepAction::Deny("a is denied".to_owned())
            } else {
                StepAction::Proceed
            }
        }),
    )
    .await;

    let calls = log.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "only B's call runs: {calls:?}");
    assert_eq!(calls[0].input["who"], "b");
    assert_eq!(call_end(&out.events, "wa1").map(|(_, e)| e), Some(true));
    assert_eq!(call_end(&out.events, "wb1").map(|(_, e)| e), Some(false));
}

// ---------------------------------------------------------------------------
// W8: optional worker timeout (guards for the unchanged behaviour; the new
// `without_worker_timeout` API is tested in orchestrator_worker_timeout.rs)
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_w8_default_worker_timeout_should_stay_300_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W8-SLOW"]);
    *provider.worker_delay.lock().unwrap() = Some(Duration::from_secs(301));
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui_limit(
        orch,
        plan(vec![subtask("1", "W8-SLOW", "rust-expert")]),
        None,
        proceed_all(),
        Duration::from_secs(86_400),
    )
    .await;

    assert!(
        matches!(out.workers[0].status, WorkerStatus::TimedOut(_)),
        "{:?}",
        out.workers[0].status
    );
}

#[tokio::test(start_paused = true)]
async fn test_w8_default_worker_timeout_should_not_fire_before_300_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W8-JUST"]);
    *provider.worker_delay.lock().unwrap() = Some(Duration::from_secs(299));
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui_limit(
        orch,
        plan(vec![subtask("1", "W8-JUST", "rust-expert")]),
        None,
        proceed_all(),
        Duration::from_secs(86_400),
    )
    .await;

    assert!(
        matches!(out.workers[0].status, WorkerStatus::Completed),
        "{:?}",
        out.workers[0].status
    );
}

#[tokio::test(start_paused = true)]
async fn test_w8_with_worker_timeout_should_still_apply_the_given_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W8-SHORT"]);
    *provider.worker_delay.lock().unwrap() = Some(Duration::from_secs(30));
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4).with_worker_timeout(10);
    let out = run_plan_with_ui_limit(
        orch,
        plan(vec![subtask("1", "W8-SHORT", "rust-expert")]),
        None,
        proceed_all(),
        Duration::from_secs(86_400),
    )
    .await;

    assert!(matches!(out.workers[0].status, WorkerStatus::TimedOut(_)));
}
