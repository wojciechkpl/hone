#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W45 of `docs/plans/worker-tools-spec.md`: aborting the post-turn task really
//! stops a running fix turn. `TestVerificationLoop::run_with_handle` dispatches
//! fix turns to the agent actor; the caller's `events` sender is still open
//! (the UI holds the receiver), so dropping the loop future does not stop the
//! actor's turn. The fix: the loop gives each fix turn an event channel it owns
//! (forwarding to the caller's), so dropping the loop closes it and the actor
//! cancels the turn, including its pending tool calls. No public API change.

mod worker_tools_support;

use std::time::Duration;

use hone_core::{AgentHandle, TestVerificationLoop};
use hone_proto::{AgentEvent, ToolApprovalPolicy};
use serde_json::json;
use tokio::sync::mpsc;

use worker_tools_support::*;

#[tokio::test]
async fn test_w45_dropping_the_test_loop_should_cancel_a_fix_turn_waiting_for_approval() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    // The fix turn's model asks to run `shell`; nobody ever answers the prompt.
    let (_step_tx, step_rx) = step_channel();
    let mut config = lead_config(ScriptedProvider::new(&[]), session_tools(&log), dir.path());
    config.provider = CallsOnce::new(vec![tool_call("f1", "shell", json!({}))]);
    config.approval_policy = ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    };
    config.step_rx = Some(step_rx);
    let handle = AgentHandle::spawn(config);

    // The test command fails, so one fix turn follows.
    let verification = TestVerificationLoop::new("exit 1", 2, dir.path().to_path_buf());
    // The caller (the TUI loop) keeps its receiver open throughout.
    let (events_tx, mut events_rx) = mpsc::channel::<AgentEvent>(256);
    let h = handle.clone();
    let task = tokio::spawn(async move { verification.run_with_handle(&h, events_tx).await });

    // Wait until the fix turn is paused at the approval prompt.
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = events_rx.recv().await {
            if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                return;
            }
        }
        panic!("events closed before the fix turn paused");
    })
    .await
    .expect("the fix turn should pause for approval");

    // Esc: the post-turn task is aborted.
    task.abort();
    let _ = task.await;

    // The actor must be released promptly: a plain command gets answered.
    let free = tokio::time::timeout(Duration::from_secs(5), handle.get_messages()).await;
    assert!(
        free.is_ok(),
        "the actor is still stuck in the fix turn after the loop was dropped"
    );
    assert!(invoked(&log).is_empty(), "the pending tool call never ran");
}
