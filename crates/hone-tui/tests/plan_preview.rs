//! Slice 2 RED-phase tests for the `:plan <prompt>` TUI command.
//!
//! Contract:
//! - `Command::Plan(String)` variant parses from `/plan <prompt>`.
//! - `PlanPreviewState` holds the decomposed plan + cost estimate and is
//!   constructed via `PlanPreviewState::from_decompose`.
//! - `App` gains `plan_preview`, `plan_preview_open`, and
//!   `pending_plan_action` fields with documented setters / readers.
//! - `App::handle_key_plan_preview` drives the overlay state machine.
//!
//! These tests are INTENTIONALLY failing until the rust-expert adds the
//! required symbols in the green phase.  The implementer MUST NOT modify
//! this file.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_core::{DEFAULT_WORKER_COST_USD, SubTask, TaskPlan};
use hone_tui::{
    app::{App, PlanAction, PlanPreviewState},
    commands::Command,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Build a `TaskPlan` with the given goal and `n` synthetic subtasks.
fn make_task_plan(goal: &str, n: usize) -> TaskPlan {
    let subtasks = (0..n)
        .map(|i| SubTask {
            id: format!("t{i}"),
            description: format!("subtask {i}"),
            agent: format!("agent-{i}"),
            context: String::new(),
            depends_on: vec![],
            acceptance_tests: vec![],
            budget_usd: None,
            scope: vec![],
            requires_capabilities: vec![],
        })
        .collect();
    TaskPlan {
        goal: goal.to_string(),
        subtasks,
    }
}

/// Build a minimal `PlanPreviewState` for use in overlay key-handler tests.
fn make_preview_state() -> PlanPreviewState {
    PlanPreviewState::from_decompose("refactor auth", make_task_plan("refactor auth", 1), 0.01)
}

// ---------------------------------------------------------------------------
// 1. Command parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_plan_command_with_prompt() {
    let cmd = Command::parse("/plan refactor x").expect("parse returned None");
    assert_eq!(cmd, Command::Plan("refactor x".to_string()));
}

#[test]
fn parse_plan_command_empty() {
    let cmd = Command::parse("/plan").expect("parse returned None");
    assert_eq!(cmd, Command::Plan(String::new()));
}

// ---------------------------------------------------------------------------
// 2. Completions
// ---------------------------------------------------------------------------

#[test]
fn plan_in_command_completions() {
    let completions = Command::completions("p");
    assert!(
        completions.contains(&"plan"),
        "expected 'plan' in completions for prefix 'p', got: {completions:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. PlanPreviewState constructor + cost formula
// ---------------------------------------------------------------------------

#[test]
fn plan_preview_state_constructor_uses_default_worker_cost() {
    let plan = make_task_plan("do the thing", 2);
    let lead_cost = 0.012_f64;
    let state = PlanPreviewState::from_decompose("do the thing", plan, lead_cost);

    // Goal is forwarded.
    assert_eq!(state.goal, "do the thing");
    // Two subtasks round-tripped.
    assert_eq!(state.subtasks.len(), 2);
    // Cost formula: lead + N * DEFAULT_WORKER_COST_USD.
    let expected = lead_cost + 2.0 * DEFAULT_WORKER_COST_USD;
    let diff = (state.estimated_total_usd - expected).abs();
    assert!(
        diff < 1e-9,
        "estimated_total_usd={} expected={expected}",
        state.estimated_total_usd
    );
    // lead_cost_usd stored verbatim.
    let lead_diff = (state.lead_cost_usd - lead_cost).abs();
    assert!(lead_diff < 1e-9);
    // Prompt stored.
    assert_eq!(state.prompt, "do the thing");
}

#[test]
fn plan_preview_state_subtask_fields_populated() {
    let plan = make_task_plan("goal", 2);
    let state = PlanPreviewState::from_decompose("goal", plan, 0.0);
    let first = &state.subtasks[0];
    assert_eq!(first.id, "t0");
    assert_eq!(first.agent, "agent-0");
    assert_eq!(first.description, "subtask 0");
}

// ---------------------------------------------------------------------------
// 4. App overlay open/read
// ---------------------------------------------------------------------------

#[test]
fn set_plan_preview_opens_overlay() {
    let mut app = App::new();
    assert!(!app.plan_preview_open, "should start closed");
    assert!(app.plan_preview().is_none(), "should start empty");

    app.set_plan_preview(make_preview_state());

    assert!(app.plan_preview_open, "overlay must be open after set_plan_preview");
    assert!(app.plan_preview().is_some(), "state must be stored");
}

// ---------------------------------------------------------------------------
// 5-12. Key-handler state machine
// ---------------------------------------------------------------------------

#[test]
fn handle_key_plan_preview_r_runs() {
    let mut app = App::new();
    app.set_plan_preview(make_preview_state());

    app.handle_key_plan_preview(make_key(KeyCode::Char('r')));

    assert!(!app.plan_preview_open, "overlay must be dismissed");
    assert_eq!(
        app.take_pending_plan_action(),
        Some(PlanAction::Run),
        "pending action must be Run"
    );
    // plan_preview preserved so the run loop can read the prompt.
    assert!(app.plan_preview().is_some(), "plan_preview must stay Some for Run");
}

#[test]
fn handle_key_plan_preview_e_edits_and_clears_state() {
    let mut app = App::new();
    app.set_plan_preview(make_preview_state());

    app.handle_key_plan_preview(make_key(KeyCode::Char('e')));

    assert!(!app.plan_preview_open, "overlay must be dismissed");
    assert_eq!(
        app.take_pending_plan_action(),
        Some(PlanAction::Edit),
        "pending action must be Edit"
    );
    assert!(
        app.plan_preview().is_none(),
        "plan_preview must be cleared on Edit (user will re-trigger)"
    );
}

#[test]
fn handle_key_plan_preview_esc_cancels() {
    let mut app = App::new();
    app.set_plan_preview(make_preview_state());

    app.handle_key_plan_preview(make_key(KeyCode::Esc));

    assert!(!app.plan_preview_open, "overlay must be dismissed");
    assert_eq!(
        app.take_pending_plan_action(),
        Some(PlanAction::Cancel),
        "pending action must be Cancel"
    );
    assert!(app.plan_preview().is_none(), "plan_preview must be cleared on Cancel");
}

#[test]
fn handle_key_plan_preview_c_also_cancels() {
    let mut app = App::new();
    app.set_plan_preview(make_preview_state());

    app.handle_key_plan_preview(make_key(KeyCode::Char('c')));

    assert!(!app.plan_preview_open, "overlay must be dismissed");
    assert_eq!(
        app.take_pending_plan_action(),
        Some(PlanAction::Cancel),
        "pending action must be Cancel for 'c'"
    );
    assert!(app.plan_preview().is_none(), "plan_preview must be cleared on 'c'");
}

#[test]
fn handle_key_plan_preview_unknown_key_is_noop() {
    let mut app = App::new();
    app.set_plan_preview(make_preview_state());

    app.handle_key_plan_preview(make_key(KeyCode::Char('x')));

    assert!(app.plan_preview_open, "overlay must remain open on unknown key");
    assert_eq!(
        app.take_pending_plan_action(),
        None,
        "no pending action on unknown key"
    );
}

#[test]
fn take_pending_plan_action_drains_once() {
    let mut app = App::new();
    app.set_plan_preview(make_preview_state());

    app.handle_key_plan_preview(make_key(KeyCode::Char('r')));

    // First call returns Some.
    assert_eq!(app.take_pending_plan_action(), Some(PlanAction::Run));
    // Second call must drain to None.
    assert_eq!(
        app.take_pending_plan_action(),
        None,
        "take_pending_plan_action must drain the field"
    );
}

#[test]
fn enter_key_treated_as_run() {
    let mut app = App::new();
    app.set_plan_preview(make_preview_state());

    app.handle_key_plan_preview(make_key(KeyCode::Enter));

    assert!(!app.plan_preview_open, "Enter must dismiss overlay");
    assert_eq!(
        app.take_pending_plan_action(),
        Some(PlanAction::Run),
        "Enter must produce Run action"
    );
    assert!(app.plan_preview().is_some(), "plan_preview preserved for Enter/Run path");
}
