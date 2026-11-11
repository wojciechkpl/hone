#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Review round 1 of `docs/plans/plan-gate-spec.md`: rows G13 (key guard),
//! G15 (`refuse_plan`), G17 (history) and G18 (Esc while planning).

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_core::{SubTask, TaskPlan};
use hone_tui::app::{App, AppState, PlanPreviewState};
use hone_tui::message::DisplayRole;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn preview() -> PlanPreviewState {
    let plan = TaskPlan {
        goal: "refactor auth".into(),
        subtasks: vec![SubTask {
            id: "1".into(),
            description: "d".into(),
            agent: "debugger".into(),
            context: String::new(),
            depends_on: vec![],
            acceptance_tests: vec![],
            budget_usd: None,
            scope: vec![],
            requires_capabilities: vec![],
        }],
    };
    PlanPreviewState::from_decompose("refactor auth", plan, 0.01)
}

/// App with the overlay open; returns the app and a clock reading taken right
/// after opening (the overlay's stamp is no later than this).
fn opened() -> (App, Instant) {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("typed");
    app.set_plan_preview(preview());
    (app, Instant::now())
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn assert_dropped(app: &mut App, why: &str) {
    assert!(app.plan_preview_open, "overlay must stay open: {why}");
    assert!(app.plan_preview().is_some(), "{why}");
    assert_eq!(app.input().buffer, "typed", "input unchanged: {why}");
    assert!(
        app.take_pending_plan_run().is_none(),
        "nothing staged: {why}"
    );
    assert!(matches!(app.state(), AppState::Idle), "{why}");
}

// ---------------------------------------------------------------- G13

#[test]
fn test_g13_plan_keys_right_after_the_overlay_opens_are_dropped() {
    for code in [
        KeyCode::Char('r'),
        KeyCode::Char('e'),
        KeyCode::Char('c'),
        KeyCode::Enter,
    ] {
        let (mut app, t0) = opened();
        app.handle_key_at(key(code), t0 + ms(100));
        assert_dropped(&mut app, &format!("{code:?} at +100ms"));
    }
}

#[test]
fn test_g13_plan_keys_act_once_the_overlay_has_settled() {
    let (mut app, t0) = opened();
    app.handle_key_at(key(KeyCode::Char('r')), t0 + ms(500));
    assert!(app.take_pending_plan_run().is_some());
    assert!(!app.plan_preview_open);

    let (mut app, t0) = opened();
    app.handle_key_at(key(KeyCode::Char('e')), t0 + ms(500));
    assert_eq!(app.input().buffer, "refactor auth");
    assert!(!app.plan_preview_open);

    let (mut app, t0) = opened();
    app.handle_key_at(key(KeyCode::Char('c')), t0 + ms(500));
    assert!(!app.plan_preview_open);
    assert!(app.plan_preview().is_none());

    let (mut app, t0) = opened();
    app.handle_key_at(key(KeyCode::Enter), t0 + ms(500));
    assert!(app.take_pending_plan_run().is_some());
}

#[test]
fn test_g13_plan_key_right_after_another_keystroke_is_dropped() {
    let (mut app, t0) = opened();
    // A stray key (a no-op for the overlay) at +500ms, then 'r' 100ms later.
    app.handle_key_at(key(KeyCode::Char('x')), t0 + ms(500));
    app.handle_key_at(key(KeyCode::Char('r')), t0 + ms(600));
    assert_dropped(&mut app, "r 100ms after the previous keystroke");

    // 'r' 500ms after that keystroke acts.
    app.handle_key_at(key(KeyCode::Char('r')), t0 + ms(1100));
    assert!(app.take_pending_plan_run().is_some());
}

#[test]
fn test_g13_esc_cancels_at_once() {
    let (mut app, t0) = opened();

    app.handle_key_at(key(KeyCode::Esc), t0 + ms(10));

    assert!(!app.plan_preview_open);
    assert!(app.plan_preview().is_none());
    assert!(app.take_pending_plan_run().is_none());
}

#[test]
fn test_g13_plan_keys_with_modifiers_are_noops() {
    for modifier in [
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SHIFT,
    ] {
        for code in [
            KeyCode::Char('r'),
            KeyCode::Char('e'),
            KeyCode::Char('c'),
            KeyCode::Enter,
        ] {
            let (mut app, _) = opened();
            app.handle_key_plan_preview(KeyEvent::new(code, modifier));
            assert_dropped(&mut app, &format!("{modifier:?}+{code:?}"));
        }
    }
}

#[test]
fn test_g13_paste_while_the_overlay_is_open_is_dropped() {
    let (mut app, t0) = opened();

    app.handle_paste("pasted text\nwith newline");
    assert_dropped(&mut app, "handle_paste");

    app.handle_paste_at("more", t0 + ms(700));
    assert_dropped(&mut app, "handle_paste_at");
}

// ---------------------------------------------------------------- G15

#[test]
fn test_g15_refuse_plan_after_accepting_plan_resets_to_idle_and_stages_nothing() {
    let mut app = App::new();
    app.skip_splash();
    app.set_input_buffer("/plan refactor auth");
    app.submit_input();
    assert!(matches!(app.state(), AppState::WaitingForResponse));

    app.refuse_plan();

    let err = app
        .messages()
        .iter()
        .rev()
        .find(|m| matches!(m.role, DisplayRole::Error))
        .map(|m| m.content.clone());
    assert_eq!(
        err.as_deref(),
        Some("/plan: the previous turn is still finishing; try again when it's done")
    );
    assert!(matches!(app.state(), AppState::Idle));
    assert!(app.take_pending_plan_prompt().is_none());
    assert!(app.take_pending_plan_run().is_none());
    std::thread::sleep(ms(1100));
    app.tick();
    assert_eq!(app.status().turn_elapsed_secs, 0, "turn_start cleared");
}

// ---------------------------------------------------------------- G17

#[test]
fn test_g17_run_adds_the_prompt_to_up_arrow_history() {
    let mut app = App::new();
    app.skip_splash();
    app.set_plan_preview(preview());
    app.handle_key_plan_preview(key(KeyCode::Char('r')));
    // The run ends (Idle) so the input bar is usable as after any turn.
    app.cancel_turn();
    assert_eq!(app.input().buffer, "");

    app.handle_key(key(KeyCode::Up));

    assert_eq!(app.input().buffer, "refactor auth");
}

// ---------------------------------------------------------------- G18

#[test]
fn test_g18_esc_via_handle_key_during_planning_requests_a_cancel() {
    let mut app = App::new();
    app.skip_splash();
    app.set_input_buffer("/plan refactor auth");
    app.submit_input();
    let _ = app.take_pending_plan_prompt();
    assert!(matches!(app.state(), AppState::WaitingForResponse));

    app.handle_key(key(KeyCode::Esc));

    assert!(
        app.take_pending_cancel(),
        "Esc while planning must request a cancel"
    );
}
