#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Review round 2 of `docs/plans/plan-gate-spec.md`: rows G19 (no message left
//! streaming), G20 (`BudgetProgress` leaves the session cost alone) and G21
//! (a refused `/plan` leaves no "Planning:" line).

use hone_proto::{AgentEvent, TokenUsage};
use hone_tui::app::{App, AppState};
use hone_tui::message::DisplayRole;

fn tok(t: &str) -> AgentEvent {
    AgentEvent::Token { text: t.into() }
}

fn done(cost: f64) -> AgentEvent {
    AgentEvent::Done {
        usage: TokenUsage::default(),
        cost_usd: cost,
        plan: None,
    }
}

fn started(id: &str) -> AgentEvent {
    AgentEvent::WorkerStarted {
        task_id: id.into(),
        agent: "debugger".into(),
        description: format!("do {id}"),
    }
}

fn completed(id: &str) -> AgentEvent {
    AgentEvent::WorkerCompleted {
        task_id: id.into(),
        agent: "debugger".into(),
        status: "completed".into(),
        cost_usd: 0.1,
    }
}

fn streaming(app: &App) -> Vec<String> {
    app.messages()
        .iter()
        .filter(|m| m.is_streaming)
        .map(|m| m.content.clone())
        .collect()
}

fn busy_app() -> App {
    let mut app = App::new();
    app.skip_splash();
    app.set_input_buffer("do the thing");
    app.submit_input();
    assert!(matches!(app.state(), AppState::WaitingForResponse));
    app
}

// ---------------------------------------------------------------- G19

#[test]
fn test_g19_orchestrated_event_sequence_leaves_no_message_streaming() {
    let mut app = busy_app();

    app.handle_agent_event(tok("{decompose json}"));
    app.handle_agent_event(AgentEvent::PlanCreated {
        goal: "g".into(),
        subtask_count: 2,
    });
    assert!(
        streaming(&app).is_empty(),
        "after PlanCreated: {:?}",
        streaming(&app)
    );

    app.handle_agent_event(started("a"));
    app.handle_agent_event(tok("worker a answer"));
    app.handle_agent_event(completed("a"));
    assert!(
        streaming(&app).is_empty(),
        "worker a's message must stop streaming at WorkerCompleted: {:?}",
        streaming(&app)
    );

    app.handle_agent_event(started("b"));
    app.handle_agent_event(tok("worker b answer"));
    app.handle_agent_event(completed("b"));
    assert!(
        streaming(&app).is_empty(),
        "after worker b: {:?}",
        streaming(&app)
    );

    app.handle_agent_event(tok("merged answer"));
    app.handle_agent_event(tok("\n\n--- Orchestration complete ---"));
    app.handle_agent_event(done(0.5));

    assert!(
        streaming(&app).is_empty(),
        "after Done: {:?}",
        streaming(&app)
    );
}

#[test]
fn test_g19_next_worker_started_ends_the_previous_workers_stream() {
    let mut app = busy_app();
    app.handle_agent_event(started("a"));
    app.handle_agent_event(tok("worker a answer"));

    // No WorkerCompleted in between: WorkerStarted itself must end the stream.
    app.handle_agent_event(started("b"));

    assert!(streaming(&app).is_empty(), "{:?}", streaming(&app));
}

#[test]
fn test_g19_error_leaves_no_message_streaming() {
    let mut app = busy_app();
    app.handle_agent_event(tok("first"));
    app.handle_agent_event(AgentEvent::PlanCreated {
        goal: "g".into(),
        subtask_count: 1,
    });
    app.handle_agent_event(tok("worker answer"));

    app.handle_agent_event(AgentEvent::Error {
        message: "boom".into(),
    });

    assert!(streaming(&app).is_empty(), "{:?}", streaming(&app));
}

#[test]
fn test_g19_cancel_turn_leaves_no_message_streaming() {
    let mut app = busy_app();
    app.handle_agent_event(tok("first"));
    app.handle_agent_event(AgentEvent::PlanCreated {
        goal: "g".into(),
        subtask_count: 1,
    });
    app.handle_agent_event(tok("worker answer"));

    app.cancel_turn();

    assert!(streaming(&app).is_empty(), "{:?}", streaming(&app));
}

// ---------------------------------------------------------------- G20

#[test]
fn test_g20_budget_progress_does_not_touch_the_session_cost() {
    let mut app = busy_app();
    app.handle_agent_event(done(1.0));
    assert!((app.status().session_cost_usd - 1.0).abs() < 1e-9);

    app.handle_agent_event(AgentEvent::BudgetProgress {
        spent_usd: 0.3,
        budget_usd: Some(5.0),
        workers_completed: 1,
        workers_total: 2,
    });

    assert!(
        (app.status().session_cost_usd - 1.0).abs() < 1e-9,
        "BudgetProgress alone must not change the session cost, got {}",
        app.status().session_cost_usd
    );
}

#[test]
fn test_g20_session_cost_after_an_orchestration_is_prior_plus_done() {
    let mut app = busy_app();
    app.handle_agent_event(done(1.0));
    app.set_input_buffer("orchestrate");
    app.submit_input();

    app.handle_agent_event(AgentEvent::BudgetProgress {
        spent_usd: 0.3,
        budget_usd: None,
        workers_completed: 1,
        workers_total: 1,
    });
    app.handle_agent_event(done(0.5));

    assert!(
        (app.status().session_cost_usd - 1.5).abs() < 1e-9,
        "got {}",
        app.status().session_cost_usd
    );
}

// ---------------------------------------------------------------- G21

#[test]
fn test_g21_refused_plan_leaves_only_the_refusal_not_the_planning_line() {
    let mut app = App::new();
    app.skip_splash();
    app.set_input_buffer("/plan refactor auth");
    app.submit_input();
    assert!(
        app.messages()
            .iter()
            .any(|m| m.content == "Planning: refactor auth")
    );

    app.refuse_plan();

    assert!(
        !app.messages()
            .iter()
            .any(|m| m.content.starts_with("Planning:")),
        "no Planning line may remain"
    );
    let last = app.messages().last().unwrap();
    assert!(matches!(last.role, DisplayRole::Error));
    assert_eq!(
        last.content,
        "/plan: the previous turn is still finishing; try again when it's done"
    );
}
