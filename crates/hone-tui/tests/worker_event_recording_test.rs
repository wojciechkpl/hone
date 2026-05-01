//! Integration test: WorkerStarted / WorkerCompleted events must be recorded
//! into the current TurnLedger, not merely pushed as plain System messages.
//!
//! This test file is intentionally RED — it will fail to compile until the
//! following items are added to the production code:
//!
//!   In `turn_ledger.rs`:
//!     - `WorkerStatus` enum (Running, Succeeded, Failed, TimedOut)
//!     - `WorkerInvocation` struct with pub fields
//!     - `worker_invocations: Vec<WorkerInvocation>` field on `TurnLedger`
//!     - `record_worker_started` / `record_worker_completed` methods
//!
//!   In `app.rs`:
//!     - `App::handle_agent_event(WorkerStarted)` must call
//!       `self.current_turn.get_or_insert_with(...).record_worker_started(...)`
//!     - `App::handle_agent_event(WorkerCompleted)` must call
//!       `self.current_turn.get_or_insert_with(...).record_worker_completed(...)`
//!     - `pub fn current_turn(&self) -> Option<&TurnLedger>` accessor so
//!       external test code can inspect the ledger without making the field pub.

use hone_proto::AgentEvent;
use hone_tui::{
    app::App,
    turn_ledger::{TurnLedger, WorkerInvocation, WorkerStatus},
};

// ---------------------------------------------------------------------------
// Test 11
// ---------------------------------------------------------------------------

/// Verify that AgentEvent::WorkerStarted and AgentEvent::WorkerCompleted are
/// recorded into `App::current_turn()` (the in-progress TurnLedger) rather
/// than just pushed into the conversation message log.
///
/// Regression: currently both event arms in `app.rs:2166-2187` only call
/// `self.messages.push(...)` and never touch the ledger, so
/// `current_turn().worker_invocations` stays empty.
#[test]
fn app_records_worker_events_into_current_turn_ledger() {
    let mut app = App::new();

    // Drive a WorkerStarted event.
    app.handle_agent_event(AgentEvent::WorkerStarted {
        task_id: "t1".to_string(),
        agent: "rust-expert".to_string(),
        description: "do thing".to_string(),
    });

    // The event must have created or updated current_turn with a Running entry.
    // `current_turn()` is a public accessor the implementer must add.
    {
        let ledger: &TurnLedger = app
            .current_turn()
            .expect("handle_agent_event(WorkerStarted) must initialise current_turn");

        assert_eq!(
            ledger.worker_invocations.len(),
            1,
            "WorkerStarted must add exactly one entry to worker_invocations"
        );

        let w: &WorkerInvocation = &ledger.worker_invocations[0];
        assert_eq!(w.task_id, "t1");
        assert_eq!(w.agent, "rust-expert");
        assert_eq!(w.description, "do thing");
        assert_eq!(
            w.status,
            WorkerStatus::Running,
            "entry must start as Running"
        );
        assert!(w.cost_usd.is_none());
    }

    // Drive a WorkerCompleted event for the same task.
    app.handle_agent_event(AgentEvent::WorkerCompleted {
        task_id: "t1".to_string(),
        agent: "rust-expert".to_string(),
        status: "completed".to_string(),
        cost_usd: 0.05,
    });

    // The same entry must be updated in-place; vec length stays at 1.
    {
        let ledger: &TurnLedger = app
            .current_turn()
            .expect("current_turn must still be Some after WorkerCompleted");

        assert_eq!(
            ledger.worker_invocations.len(),
            1,
            "WorkerCompleted must update the existing entry, not add a second one"
        );

        let w: &WorkerInvocation = &ledger.worker_invocations[0];
        assert_eq!(
            w.status,
            WorkerStatus::Succeeded,
            "status must be Succeeded after 'completed'"
        );
        assert_eq!(
            w.cost_usd,
            Some(0.05),
            "cost_usd must be recorded from the WorkerCompleted event"
        );
    }
}
