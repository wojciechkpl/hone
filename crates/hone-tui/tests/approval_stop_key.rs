#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W21 of `docs/plans/worker-tools-spec.md`: `x` in the approval menu denies the
//! paused call, closes the menu and cancels the turn (like Esc with no menu
//! open: `take_pending_cancel()` is true). Like the other letter keys it counts
//! only after the 400 ms menu guard; the footer hints list it.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_proto::{AgentEvent, StepAction};
use hone_tui::{app::App, render::root};
use ratatui::{Terminal, backend::TestBackend};
use tokio::sync::mpsc;

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn ms(base: Instant, n: u64) -> Instant {
    base + Duration::from_millis(n)
}

fn paused_event() -> AgentEvent {
    AgentEvent::ToolCallPaused {
        id: "c1".to_owned(),
        name: "shell".to_owned(),
        args: serde_json::json!({}),
        worker: None,
    }
}

fn app() -> (App, mpsc::Receiver<StepAction>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new();
    app.set_step_tx(tx);
    (app, rx)
}

#[test]
fn test_w31_x_with_text_in_the_input_box_should_do_nothing() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.type_text("fix the");
    app.handle_agent_event_at(paused_event(), ms(t0, 0));

    app.handle_key_at(key(KeyCode::Char('x')), ms(t0, 900));

    assert!(rx.try_recv().is_err(), "no answer");
    assert!(app.debugger_open, "menu stays open");
    assert!(!app.take_pending_cancel(), "no stop requested");
    assert_eq!(app.input().buffer, "fix thex", "the x is typed");
}

#[test]
fn test_w21_x_soon_after_menu_opened_should_do_nothing() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Char('x')), ms(t0, 100));

    assert!(rx.try_recv().is_err(), "no answer before the guard");
    assert!(app.debugger_open, "menu stays open");
    assert!(!app.take_pending_cancel(), "no cancel before the guard");
}

#[test]
fn test_w21_x_soon_after_previous_keystroke_should_do_nothing() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);
    app.handle_key_at(key(KeyCode::Char('k')), ms(t0, 600));

    app.handle_key_at(key(KeyCode::Char('x')), ms(t0, 700));

    assert!(rx.try_recv().is_err());
    assert!(app.debugger_open);
    assert!(!app.take_pending_cancel());
}

#[test]
fn test_w21_n_should_deny_but_not_cancel_the_turn() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);

    app.handle_key_at(key(KeyCode::Char('n')), ms(t0, 500));

    assert!(matches!(rx.try_recv(), Ok(StepAction::Deny(_))));
    assert!(!app.take_pending_cancel(), "only x stops the turn");
}

#[test]
fn test_w21_x_when_no_menu_is_open_should_just_be_typed() {
    let (mut app, mut rx) = app();
    let t0 = Instant::now();

    app.handle_key_at(key(KeyCode::Char('x')), ms(t0, 1000));

    assert!(rx.try_recv().is_err());
    assert!(!app.take_pending_cancel());
    assert_eq!(app.input().buffer, "x");
}

#[test]
fn test_w21_menu_hints_should_list_x() {
    let (mut app, _rx) = app();
    app.skip_splash();
    app.handle_agent_event_at(paused_event(), Instant::now());
    let (w, h) = (140u16, 40u16);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("terminal");
    terminal.draw(|f| root::render(&app, f)).expect("draw");
    let buf = terminal.backend().buffer().clone();
    let mut rows = Vec::new();
    for row in 0..h {
        let line: String = (0..w)
            .map(|col| {
                buf.cell((col, row))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ')
            })
            .collect();
        rows.push(line);
    }
    let hint = rows
        .iter()
        .find(|l| l.contains("proceed"))
        .unwrap_or_else(|| panic!("menu footer not rendered:\n{}", rows.join("\n")));
    assert!(
        hint.split_whitespace().any(|w| w == "x"),
        "the footer must list the x key: {hint:?}"
    );
}
