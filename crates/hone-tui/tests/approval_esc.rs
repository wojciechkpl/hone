//! Esc on an approval prompt must never run the paused call (P2.2 spec 6):
//! it sends `StepAction::Skip`, not `Proceed`. The other keys keep their
//! mapping.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_proto::StepAction;
use hone_tui::app::App;
use tokio::sync::mpsc;

fn press(app: &mut App, code: KeyCode) {
    app.handle_key_debugger(KeyEvent::new(code, KeyModifiers::NONE));
}

fn app_with_channel() -> (App, mpsc::Receiver<StepAction>) {
    let (tx, rx) = mpsc::channel(4);
    let mut app = App::new();
    app.set_step_tx(tx);
    app.debugger_open = true;
    (app, rx)
}

#[test]
fn test_esc_on_approval_prompt_should_send_skip_and_close() {
    let (mut app, mut rx) = app_with_channel();
    press(&mut app, KeyCode::Esc);
    assert!(
        matches!(rx.try_recv(), Ok(StepAction::Skip)),
        "Esc must dismiss the prompt without running the call"
    );
    assert!(!app.debugger_open);
}

#[test]
fn test_esc_on_approval_prompt_should_not_send_proceed() {
    let (mut app, mut rx) = app_with_channel();
    press(&mut app, KeyCode::Esc);
    let got = rx.try_recv().expect("an action must be sent");
    assert!(!matches!(got, StepAction::Proceed));
}

#[test]
fn test_enter_on_approval_prompt_should_still_send_proceed() {
    let (mut app, mut rx) = app_with_channel();
    press(&mut app, KeyCode::Enter);
    assert!(matches!(rx.try_recv(), Ok(StepAction::Proceed)));
}
