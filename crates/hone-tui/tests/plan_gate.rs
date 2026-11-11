#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Rows G1-G5 and G8-G10 of `docs/plans/plan-gate-spec.md`: the `/plan`
//! overlay acts on its keys at once, Edit keeps the prompt, Run stages the
//! plan shown, planning is a background turn, and the input cursor is a byte
//! offset.
//!
//! API pinned here (new): `App::take_pending_plan_run() -> Option<PlanPreviewState>`,
//! `PlanPreviewState::plan: hone_core::TaskPlan`, `App::plan_failed(&str)`.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_core::{SubTask, TaskPlan};
use hone_proto::AgentEvent;
use hone_tui::app::{App, AppState, PlanPreviewState};
use hone_tui::message::DisplayRole;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn rich_plan() -> TaskPlan {
    TaskPlan {
        goal: "refactor auth".to_owned(),
        subtasks: vec![
            SubTask {
                id: "1".into(),
                description: "read the code".into(),
                agent: "debugger".into(),
                context: "ctx one".into(),
                depends_on: vec![],
                acceptance_tests: vec!["it compiles".into()],
                budget_usd: Some(0.25),
                scope: vec!["src/auth.rs".into()],
                requires_capabilities: vec!["fs".into()],
            },
            SubTask {
                id: "2".into(),
                description: "change it".into(),
                agent: "tdd-workflow".into(),
                context: "ctx two".into(),
                depends_on: vec!["1".into()],
                acceptance_tests: vec!["tests pass".into(), "no warnings".into()],
                budget_usd: None,
                scope: vec!["src/".into(), "tests/".into()],
                requires_capabilities: vec![],
            },
        ],
    }
}

fn preview(prompt: &str) -> PlanPreviewState {
    PlanPreviewState::from_decompose(prompt, rich_plan(), 0.01)
}

fn user_texts(app: &App) -> Vec<String> {
    app.messages()
        .iter()
        .filter(|m| matches!(m.role, DisplayRole::User))
        .map(|m| m.content.clone())
        .collect()
}

fn last_error(app: &App) -> Option<String> {
    app.messages()
        .iter()
        .rev()
        .find(|m| matches!(m.role, DisplayRole::Error))
        .map(|m| m.content.clone())
}

fn waiting_app() -> App {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("some running turn");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.state(), AppState::WaitingForResponse));
    app
}

/// A time safely past the G13 guard (400 ms) for an overlay opened just before.
fn settled() -> Instant {
    Instant::now() + Duration::from_millis(600)
}

fn submit(app: &mut App, text: &str) {
    app.set_input_buffer(text);
    app.submit_input();
}

// ---------------------------------------------------------------- G5

#[test]
fn test_g5_preview_keeps_the_full_plan_with_every_subtask_field() {
    let state = preview("refactor auth");
    let expected = rich_plan();

    assert_eq!(state.plan.goal, expected.goal);
    assert_eq!(state.plan.subtasks, expected.subtasks);
    // Existing fields unchanged.
    assert_eq!(state.prompt, "refactor auth");
    assert_eq!(state.subtasks.len(), 2);
}

// ---------------------------------------------------------------- G1 / G4

#[test]
fn test_g4_run_key_stages_the_plan_and_starts_a_turn() {
    let mut app = App::new();
    app.set_plan_preview(preview("refactor auth"));

    app.handle_key_plan_preview(key(KeyCode::Char('r')));

    assert!(!app.plan_preview_open);
    assert!(app.plan_preview().is_none(), "preview is moved out");
    assert_eq!(user_texts(&app), vec!["refactor auth".to_owned()]);
    assert!(matches!(app.state(), AppState::WaitingForResponse));
    let staged = app.take_pending_plan_run().expect("staged run");
    assert_eq!(staged.prompt, "refactor auth");
    assert_eq!(staged.plan.subtasks, rich_plan().subtasks);
    assert!(app.take_pending_plan_run().is_none(), "drained once");
}

#[test]
fn test_g4_enter_runs_like_r() {
    let mut app = App::new();
    app.set_plan_preview(preview("refactor auth"));

    app.handle_key_plan_preview(key(KeyCode::Enter));

    assert!(matches!(app.state(), AppState::WaitingForResponse));
    assert_eq!(user_texts(&app), vec!["refactor auth".to_owned()]);
    assert!(app.take_pending_plan_run().is_some());
}

#[test]
fn test_g4_run_starts_the_turn_with_the_default_agent() {
    let mut app = App::new();
    app.set_plan_preview(preview("refactor auth"));
    app.handle_key_plan_preview(key(KeyCode::Char('r')));

    app.handle_agent_event(AgentEvent::ToolCallStart {
        id: "t1".into(),
        name: "read".into(),
        args: serde_json::Value::Null,
    });

    let ledger = app.current_turn().expect("ledger");
    assert_eq!(ledger.active_agent.as_deref(), Some("default"));
}

#[test]
fn test_g4_run_clears_per_turn_fallback_notices() {
    let mut app = App::new();
    let fb = || AgentEvent::ModelFallback {
        requested: "a:x".into(),
        fallback: "b:y".into(),
        reason: "down".into(),
    };
    app.handle_agent_event(fb());
    let count = |app: &App| {
        app.messages()
            .iter()
            .filter(|m| m.content.contains("switched to fallback"))
            .count()
    };
    assert_eq!(count(&app), 1);

    app.set_plan_preview(preview("refactor auth"));
    app.handle_key_plan_preview(key(KeyCode::Char('r')));
    app.handle_agent_event(fb());

    assert_eq!(
        count(&app),
        2,
        "the same pair is shown again in the new turn"
    );
}

#[test]
fn test_g4_run_sets_turn_start() {
    let mut app = App::new();
    app.set_plan_preview(preview("refactor auth"));
    app.handle_key_plan_preview(key(KeyCode::Char('r')));

    std::thread::sleep(std::time::Duration::from_millis(1100));
    app.tick();

    assert!(
        app.status().turn_elapsed_secs >= 1,
        "turn_start must be set"
    );
}

#[test]
fn test_g1_handle_key_with_overlay_open_runs_at_once_and_ignores_typed_text() {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("half-typed");
    app.set_plan_preview(preview("refactor auth"));

    app.handle_key_at(key(KeyCode::Enter), settled());

    assert!(
        app.take_pending_plan_run().is_some(),
        "run staged by the key itself"
    );
    assert_eq!(
        user_texts(&app),
        vec!["refactor auth".to_owned()],
        "the typed text must not be submitted"
    );
    assert_eq!(app.input().buffer, "half-typed", "input bar untouched");
}

#[test]
fn test_g1_handle_key_r_with_overlay_open_runs_at_once() {
    let mut app = App::new();
    app.skip_splash();
    app.set_plan_preview(preview("refactor auth"));

    app.handle_key_at(key(KeyCode::Char('r')), settled());

    assert!(app.take_pending_plan_run().is_some());
    assert!(matches!(app.state(), AppState::WaitingForResponse));
    assert_eq!(app.input().buffer, "", "'r' is not typed into the input");
}

// ---------------------------------------------------------------- G2 / G10

#[test]
fn test_g2_edit_puts_the_prompt_in_the_input_bar_replacing_the_text() {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("old draft");
    let before = app.messages().len();
    app.set_plan_preview(preview("refactor auth"));

    app.handle_key_plan_preview(key(KeyCode::Char('e')));

    assert!(!app.plan_preview_open);
    assert!(app.plan_preview().is_none());
    assert_eq!(app.input().buffer, "refactor auth");
    assert_eq!(app.input().cursor_position, "refactor auth".len());
    assert_eq!(app.messages().len(), before, "no message pushed");
    assert!(matches!(app.state(), AppState::Idle));
    assert!(app.take_pending_plan_run().is_none());
}

#[test]
fn test_g2_edit_via_handle_key_with_overlay_open() {
    let mut app = App::new();
    app.skip_splash();
    app.set_plan_preview(preview("refactor auth"));

    app.handle_key_at(key(KeyCode::Char('e')), settled());

    assert_eq!(app.input().buffer, "refactor auth");
    assert!(!app.plan_preview_open);
}

#[test]
fn test_g2_slash_plan_prompt_is_trimmed_and_staged_without_the_command() {
    let mut app = App::new();
    app.skip_splash();

    submit(&mut app, "/plan   refactor auth  ");

    assert_eq!(
        app.take_pending_plan_prompt().as_deref(),
        Some("refactor auth")
    );
}

#[test]
fn test_g10_set_input_buffer_cursor_is_a_byte_offset() {
    let mut app = App::new();
    let text = "zażółć gęślą jaźń";
    assert_ne!(text.len(), text.chars().count());

    app.set_input_buffer(text);

    assert_eq!(app.input().cursor_position, text.len());
}

#[test]
fn test_g10_typing_after_edit_with_non_ascii_prompt_appends_at_end() {
    let mut app = App::new();
    app.skip_splash();
    let prompt = "zażółć gęślą jaźń";
    app.set_plan_preview(preview(prompt));
    app.handle_key_plan_preview(key(KeyCode::Char('e')));

    app.handle_key(key(KeyCode::Char('x')));

    assert_eq!(app.input().buffer, format!("{prompt}x"));
}

// ---------------------------------------------------------------- G3

#[test]
fn test_g3_cancel_keys_close_the_overlay_and_change_nothing_else() {
    for code in [KeyCode::Char('c'), KeyCode::Esc] {
        let mut app = App::new();
        app.skip_splash();
        app.type_text("typed");
        let before = app.messages().len();
        app.set_plan_preview(preview("refactor auth"));

        app.handle_key_plan_preview(key(code));

        assert!(!app.plan_preview_open, "{code:?}");
        assert!(app.plan_preview().is_none(), "{code:?}");
        assert_eq!(app.input().buffer, "typed", "{code:?}");
        assert_eq!(app.messages().len(), before, "{code:?}");
        assert!(matches!(app.state(), AppState::Idle), "{code:?}");
        assert!(app.take_pending_plan_run().is_none(), "{code:?}");
    }
}

#[test]
fn test_g3_other_keys_are_noops_and_stage_nothing() {
    let mut app = App::new();
    app.set_plan_preview(preview("refactor auth"));

    for code in [KeyCode::Char('x'), KeyCode::Tab, KeyCode::Down] {
        app.handle_key_plan_preview(key(code));
    }

    assert!(app.plan_preview_open);
    assert!(app.plan_preview().is_some());
    assert!(app.take_pending_plan_run().is_none());
    assert!(matches!(app.state(), AppState::Idle));
}

// ---------------------------------------------------------------- G8

#[test]
fn test_g8_plan_command_when_idle_starts_background_planning() {
    let mut app = App::new();
    app.skip_splash();

    submit(&mut app, "/plan refactor auth");

    assert!(
        app.messages()
            .iter()
            .any(|m| m.content == "Planning: refactor auth")
    );
    assert!(matches!(app.state(), AppState::WaitingForResponse));
    assert_eq!(
        app.take_pending_plan_prompt().as_deref(),
        Some("refactor auth")
    );
    assert!(app.take_pending_plan_prompt().is_none());
    std::thread::sleep(std::time::Duration::from_millis(1100));
    app.tick();
    assert!(
        app.status().turn_elapsed_secs >= 1,
        "turn_start must be set"
    );
}

#[test]
fn test_g8_plan_command_when_in_error_state_also_plans() {
    let mut app = App::new();
    app.skip_splash();
    app.handle_agent_event(AgentEvent::Error {
        message: "boom".into(),
    });
    assert!(matches!(app.state(), AppState::Error(_)));

    submit(&mut app, "/plan refactor auth");

    assert!(matches!(app.state(), AppState::WaitingForResponse));
    assert_eq!(
        app.take_pending_plan_prompt().as_deref(),
        Some("refactor auth")
    );
}

#[test]
fn test_g8_set_plan_preview_after_planning_opens_overlay_and_goes_idle() {
    let mut app = App::new();
    app.skip_splash();
    submit(&mut app, "/plan refactor auth");
    let _ = app.take_pending_plan_prompt();

    app.set_plan_preview(preview("refactor auth"));

    assert!(app.plan_preview_open);
    assert!(matches!(app.state(), AppState::Idle));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    app.tick();
    assert_eq!(app.status().turn_elapsed_secs, 0, "turn_start cleared");
}

#[test]
fn test_g8_plan_failed_pushes_the_error_and_goes_idle() {
    let mut app = App::new();
    app.skip_splash();
    submit(&mut app, "/plan refactor auth");
    let _ = app.take_pending_plan_prompt();

    app.plan_failed("model unreachable");

    assert_eq!(
        last_error(&app).as_deref(),
        Some("/plan decompose failed: model unreachable")
    );
    assert!(matches!(app.state(), AppState::Idle));
    assert!(!app.plan_preview_open);
}

#[test]
fn test_g8_cancel_turn_during_planning_returns_to_idle() {
    let mut app = App::new();
    app.skip_splash();
    submit(&mut app, "/plan refactor auth");
    let _ = app.take_pending_plan_prompt();

    app.cancel_turn();

    assert!(matches!(app.state(), AppState::Idle));
    assert!(!app.plan_preview_open);
}

// ---------------------------------------------------------------- G9

#[test]
fn test_g9_plan_while_a_turn_runs_is_refused() {
    let mut app = waiting_app();
    let _ = app.take_pending_plan_prompt();

    submit(&mut app, "/plan refactor auth");

    assert_eq!(
        last_error(&app).as_deref(),
        Some("/plan: wait for the running turn to finish (Esc stops it)")
    );
    assert!(app.take_pending_plan_prompt().is_none(), "nothing staged");
    assert!(matches!(app.state(), AppState::WaitingForResponse));
    assert!(
        !app.messages()
            .iter()
            .any(|m| m.content.starts_with("Planning:")),
        "no Planning message"
    );
}

#[test]
fn test_g9_plan_while_planning_is_refused() {
    let mut app = App::new();
    app.skip_splash();
    submit(&mut app, "/plan first");
    let _ = app.take_pending_plan_prompt();

    submit(&mut app, "/plan second");

    assert_eq!(
        last_error(&app).as_deref(),
        Some("/plan: wait for the running turn to finish (Esc stops it)")
    );
    assert!(app.take_pending_plan_prompt().is_none());
}

#[test]
fn test_g9_empty_plan_keeps_its_usage_error() {
    let mut app = App::new();
    app.skip_splash();

    submit(&mut app, "/plan");

    assert_eq!(
        last_error(&app).as_deref(),
        Some("usage: /plan <describe what you want done>")
    );
    assert!(app.take_pending_plan_prompt().is_none());
    assert!(matches!(app.state(), AppState::Idle));
}
