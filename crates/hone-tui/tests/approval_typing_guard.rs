#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R4 (docs/plans/tui-approval-fix-spec.md section 6): typing must not be able to answer the
//! approval menu.
//!
//! Time seam assumed (public on `hone_tui::app::App`):
//! - `handle_key_at(&mut self, key: KeyEvent, now: std::time::Instant)`: same routing as
//!   `handle_key`, plus the guard. A menu key counts only if `now` is >= 400 ms after the menu
//!   opened AND >= 400 ms after the previous keystroke. Otherwise the key goes to the normal
//!   input handler. Esc always means Skip.
//! - `handle_agent_event_at(&mut self, event: AgentEvent, now: Instant)`: same as
//!   `handle_agent_event`, recording `now` as the menu-open time for `ToolCallPaused`.
//!
//! `handle_key_debugger` itself stays unguarded (see `approval_esc.rs`).

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_proto::{AgentEvent, StepAction};
use hone_tui::app::App;
use tokio::sync::mpsc;

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn paused_event() -> AgentEvent {
    AgentEvent::ToolCallPaused {
        id: "c1".to_owned(),
        name: "write_file".to_owned(),
        args: serde_json::json!({}),
    }
}

fn ms(base: Instant, n: u64) -> Instant {
    base + Duration::from_millis(n)
}

fn app() -> (App, mpsc::Receiver<StepAction>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new();
    app.set_step_tx(tx);
    (app, rx)
}

#[test]
fn test_r4_typing_across_menu_opening_should_not_answer_and_text_is_kept() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    for (i, c) in "add a test".chars().enumerate() {
        let now = ms(t0, i as u64 * 50);
        if i == 3 {
            // The menu pops up in the middle of the sentence.
            app.handle_agent_event_at(paused_event(), now);
        }
        app.handle_key_at(key(KeyCode::Char(c)), now);
    }

    assert!(rx.try_recv().is_err(), "typing must never answer the menu");
    assert_eq!(app.input().buffer, "add a test");
}

#[test]
fn test_r4_y_after_500ms_pause_should_proceed() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);
    app.handle_key_at(key(KeyCode::Char('x')), ms(t0, 10)); // earlier typing

    app.handle_key_at(key(KeyCode::Char('y')), ms(t0, 510));

    assert!(matches!(rx.try_recv(), Ok(StepAction::Proceed)));
}

#[test]
fn test_r4_y_soon_after_menu_opened_should_be_typed_not_answered() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Char('y')), ms(t0, 100));

    assert!(rx.try_recv().is_err());
    assert_eq!(app.input().buffer, "y");
}

#[test]
fn test_r4_y_soon_after_previous_keystroke_should_be_typed_not_answered() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);
    app.handle_key_at(key(KeyCode::Char('k')), ms(t0, 600));

    // Menu is old enough, but the previous keystroke was only 100 ms ago.
    app.handle_key_at(key(KeyCode::Char('y')), ms(t0, 700));

    assert!(rx.try_recv().is_err());
    assert_eq!(app.input().buffer, "ky");
}

#[test]
fn test_r4_esc_should_always_skip_even_immediately() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);
    app.handle_key_at(key(KeyCode::Char('h')), ms(t0, 20));

    app.handle_key_at(key(KeyCode::Esc), ms(t0, 30));

    assert!(matches!(rx.try_recv(), Ok(StepAction::Skip)));
}

#[test]
fn test_r4_space_should_not_approve_even_after_a_pause() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Char(' ')), ms(t0, 900));

    assert!(
        !matches!(rx.try_recv(), Ok(StepAction::Proceed)),
        "only y/Enter approve"
    );
}

#[test]
fn test_r4_enter_after_a_pause_should_proceed() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Enter), ms(t0, 900));

    assert!(matches!(rx.try_recv(), Ok(StepAction::Proceed)));
}

// -- Round 3 (R14) ----------------------------------------------------------------------------
//
// Extra seam assumed: `App::handle_paste_at(&mut self, text: &str, now: Instant)`;
// `handle_paste(text)` delegates with `Instant::now()` and, like a key, updates `last_key_at`.

#[test]
fn test_r14a_paste_then_immediate_y_should_not_approve() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_paste_at("some pasted text", ms(t0, 600));
    app.handle_key_at(key(KeyCode::Char('y')), ms(t0, 620));

    assert!(
        rx.try_recv().is_err(),
        "a paste counts as a keystroke for the guard"
    );
}

#[test]
fn test_r14a_paste_then_immediate_enter_should_not_approve() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_paste_at("line one\n", ms(t0, 600));
    app.handle_key_at(key(KeyCode::Enter), ms(t0, 610));

    assert!(!matches!(rx.try_recv(), Ok(StepAction::Proceed)));
}

#[test]
fn test_r14b_enter_with_non_empty_input_should_not_approve() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.type_text("fix it");
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Enter), ms(t0, 500));

    assert!(
        !matches!(rx.try_recv(), Ok(StepAction::Proceed)),
        "with text in the input box Enter belongs to the input"
    );
}

#[test]
fn test_r14b_y_with_non_empty_input_after_pause_should_approve() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.type_text("fix it");
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Char('y')), ms(t0, 500));

    assert!(matches!(rx.try_recv(), Ok(StepAction::Proceed)));
}

#[test]
fn test_r14c_cancel_turn_should_close_an_open_approval_menu() {
    let (mut app, _rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);
    assert!(app.debugger_open, "precondition: menu open");

    app.cancel_turn();

    assert!(!app.debugger_open, "a stale menu must not outlive the turn");
}

// -- R14b: with text in the input box only `y` approves (not `a` either) -----------------------

#[test]
fn test_r14b_a_with_non_empty_input_after_pause_should_be_typed_not_answered() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.type_text("fix it");
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Char('a')), ms(t0, 500));

    assert!(
        rx.try_recv().is_err(),
        "neither ApproveAlways nor Proceed may be sent while the input has text"
    );
    assert_eq!(app.input().buffer, "fix ita", "the `a` must reach the input");
}

#[test]
fn test_r14b_a_with_empty_input_after_pause_should_approve_always() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Char('a')), ms(t0, 500));

    assert!(matches!(rx.try_recv(), Ok(StepAction::ApproveAlways(_))));
}
