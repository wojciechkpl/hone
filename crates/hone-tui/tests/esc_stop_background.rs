#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W39 of `docs/plans/worker-tools-spec.md`: with the TUI idle, an empty input
//! box and the previous turn's task (post-turn test run, fix turns) still
//! running, Esc asks the loop to abort that task and says so in a system line.
//! The loop side (aborting the task handle) is not testable from here.
//!
//! Assumed App API (new):
//! - `App::set_background_task_running(&mut self, running: bool)`: the loop
//!   tells the app whether such a task is running.
//! - `App::take_pending_stop_background(&mut self) -> bool`: true once after
//!   an Esc that requests stopping it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_tui::app::App;
use hone_tui::message::DisplayRole;

fn esc(app: &mut App) {
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
}

fn idle_app() -> App {
    let mut app = App::new();
    app.skip_splash();
    app
}

fn system_lines(app: &App) -> usize {
    app.messages()
        .iter()
        .filter(|m| matches!(m.role, DisplayRole::System))
        .count()
}

#[test]
fn test_w39_esc_when_idle_empty_box_and_background_task_should_request_stop_and_say_so() {
    let mut app = idle_app();
    app.set_background_task_running(true);
    let before = system_lines(&app);

    esc(&mut app);

    assert!(
        app.take_pending_stop_background(),
        "the loop is told to abort"
    );
    assert!(
        !app.take_pending_stop_background(),
        "the request is taken once"
    );
    assert_eq!(system_lines(&app), before + 1, "a system line says so");
}

#[test]
fn test_w39_esc_with_text_in_the_input_box_should_only_clear_it() {
    let mut app = idle_app();
    app.set_background_task_running(true);
    app.type_text("half typed");
    let before = system_lines(&app);

    esc(&mut app);

    assert_eq!(app.input().buffer, "", "Esc clears the input, as now");
    assert!(
        !app.take_pending_stop_background(),
        "the task is left alone"
    );
    assert_eq!(system_lines(&app), before);
}

#[test]
fn test_w39_esc_without_a_background_task_should_behave_as_before() {
    let mut app = idle_app();
    app.set_background_task_running(false);
    let before = system_lines(&app);

    esc(&mut app);

    assert!(!app.take_pending_stop_background());
    assert_eq!(system_lines(&app), before, "no line, nothing to stop");
}

#[test]
fn test_w39_esc_after_the_background_task_ended_should_not_request_a_stop() {
    let mut app = idle_app();
    app.set_background_task_running(true);
    app.set_background_task_running(false);

    esc(&mut app);

    assert!(!app.take_pending_stop_background());
}
