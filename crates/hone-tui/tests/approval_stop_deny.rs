#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W31 of `docs/plans/worker-tools-spec.md` (amends W21), the part that needs a
//! new App API: `App::take_pending_deny_paused() -> bool`. Fails to compile
//! until it exists.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_proto::{AgentEvent, StepAction};
use hone_tui::app::App;
use tokio::sync::mpsc;

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn app() -> (App, mpsc::Receiver<StepAction>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new();
    app.set_step_tx(tx);
    (app, rx)
}

fn paused_event() -> AgentEvent {
    AgentEvent::ToolCallPaused {
        id: "c1".to_owned(),
        name: "shell".to_owned(),
        args: serde_json::json!({}),
        worker: None,
    }
}

fn ms(base: Instant, n: u64) -> Instant {
    base + Duration::from_millis(n)
}

#[test]
fn test_w21_x_after_guard_should_close_menu_and_request_stop_without_answering_itself() {
    // W31: the App no longer sends `Deny` itself. It closes the menu and sets
    // two requests the TUI loop drains in order: cancel (abort the turn) first,
    // then `take_pending_deny_paused()` (answer the paused call with Deny, so a
    // denied worker cannot start another call before the abort).
    let (mut app, mut rx) = app();
    let t0 = Instant::now();
    app.handle_agent_event_at(paused_event(), t0);
    assert!(app.debugger_open);

    app.handle_key_at(key(KeyCode::Char('x')), ms(t0, 500));

    assert!(
        rx.try_recv().is_err(),
        "the loop answers after aborting, not the App"
    );
    assert!(!app.debugger_open, "the menu closes");
    assert!(app.take_pending_cancel(), "the turn is cancelled");
    assert!(!app.take_pending_cancel(), "the flag is taken once");
    assert!(
        app.take_pending_deny_paused(),
        "the paused call is to be denied"
    );
    assert!(!app.take_pending_deny_paused(), "the request is taken once");
}
