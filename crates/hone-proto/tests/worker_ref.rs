#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W33 of `docs/plans/worker-tools-spec.md`: approval events name the worker.
//! Assumed: `hone_proto::WorkerRef { task_id: String, agent: String }` (derives
//! Debug, Clone, PartialEq, Serialize, Deserialize), and a `worker:
//! Option<WorkerRef>` field on `AgentEvent::ToolCallPaused` and
//! `AgentEvent::ApprovalRequired`, `#[serde(default, skip_serializing_if =
//! "Option::is_none")]`.

use hone_proto::{AgentEvent, WorkerRef};
use serde_json::json;

fn worker() -> WorkerRef {
    WorkerRef {
        task_id: "2".to_owned(),
        agent: "rust-expert".to_owned(),
    }
}

fn paused(worker: Option<WorkerRef>) -> AgentEvent {
    AgentEvent::ToolCallPaused {
        id: "c1".to_owned(),
        name: "shell".to_owned(),
        args: json!({ "command": "ls" }),
        worker,
    }
}

fn approval(worker: Option<WorkerRef>) -> AgentEvent {
    AgentEvent::ApprovalRequired {
        action: "shell".to_owned(),
        description: "policy rule x requires approval".to_owned(),
        worker,
    }
}

#[test]
fn test_w33_paused_without_worker_should_serialize_without_a_worker_key() {
    let v = serde_json::to_value(paused(None)).unwrap();
    assert_eq!(
        v,
        json!({ "type": "tool_call_paused", "id": "c1", "name": "shell",
                "args": { "command": "ls" } })
    );
}

#[test]
fn test_w33_approval_required_without_worker_should_serialize_without_a_worker_key() {
    let v = serde_json::to_value(approval(None)).unwrap();
    assert_eq!(
        v,
        json!({ "type": "approval_required", "action": "shell",
                "description": "policy rule x requires approval" })
    );
}

#[test]
fn test_w33_old_json_without_worker_should_deserialize_to_none() {
    let old_paused = json!({ "type": "tool_call_paused", "id": "c1", "name": "shell",
                             "args": { "command": "ls" } });
    let old_approval = json!({ "type": "approval_required", "action": "shell",
                               "description": "d" });

    match serde_json::from_value::<AgentEvent>(old_paused).unwrap() {
        AgentEvent::ToolCallPaused { worker, .. } => assert_eq!(worker, None),
        other => panic!("{other:?}"),
    }
    match serde_json::from_value::<AgentEvent>(old_approval).unwrap() {
        AgentEvent::ApprovalRequired { worker, .. } => assert_eq!(worker, None),
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_w33_events_with_a_worker_should_round_trip_and_carry_the_worker_object() {
    for event in [paused(Some(worker())), approval(Some(worker()))] {
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(
            v["worker"],
            json!({ "task_id": "2", "agent": "rust-expert" })
        );
        let back: AgentEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back, event);
    }
}
