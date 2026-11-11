#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W34 of `docs/plans/worker-tools-spec.md`: the approval menu names the worker
//! whose call is paused. Assumed: `hone_proto::WorkerRef`, and the `worker`
//! field on `AgentEvent::ToolCallPaused`. Fails to compile until they exist.

use hone_proto::{AgentEvent, WorkerRef};
use hone_tui::{app::App, render::root};
use ratatui::{Terminal, backend::TestBackend};

const LONG: &str = "refactor the parser module so that every error carries a span and a hint \
                    for the user, then update all call sites and the docs ENDMARK";

fn started(task: &str, agent: &str, description: &str) -> AgentEvent {
    AgentEvent::WorkerStarted {
        task_id: task.to_owned(),
        agent: agent.to_owned(),
        description: description.to_owned(),
    }
}

fn paused(worker: Option<(&str, &str)>) -> AgentEvent {
    AgentEvent::ToolCallPaused {
        id: "c1".to_owned(),
        name: "shell".to_owned(),
        args: serde_json::json!({ "command": "cargo test" }),
        worker: worker.map(|(task, agent)| WorkerRef {
            task_id: task.to_owned(),
            agent: agent.to_owned(),
        }),
    }
}

fn rows(app: &App, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("terminal");
    terminal.draw(|f| root::render(app, f)).expect("draw");
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|row| {
            (0..w)
                .map(|col| {
                    buf.cell((col, row))
                        .map(|c| c.symbol().chars().next().unwrap_or(' '))
                        .unwrap_or(' ')
                })
                .collect()
        })
        .collect()
}

/// Only the menu box: the conversation behind it also shows `WorkerStarted`
/// lines ("[t42] rust-expert: ..."), which must not count. Rows from the
/// title row to the footer row, cut to the box's columns.
fn menu_rows(all: &[String]) -> Vec<String> {
    let title = all
        .iter()
        .position(|r| r.contains("Agent Debugger"))
        .unwrap_or_else(|| panic!("no menu:\n{}", all.join("\n")));
    let footer = all
        .iter()
        .position(|r| r.contains("proceed"))
        .unwrap_or_else(|| panic!("no menu footer:\n{}", all.join("\n")));
    let chars: Vec<char> = all[title].chars().collect();
    let left = chars
        .iter()
        .position(|c| *c == '\u{256d}')
        .expect("box corner");
    let right = chars
        .iter()
        .rposition(|c| *c == '\u{256e}')
        .expect("box corner");
    all[title..=footer]
        .iter()
        .map(|r| r.chars().skip(left).take(right - left + 1).collect())
        .collect()
}

fn app_with_menu(events: Vec<AgentEvent>) -> App {
    let mut app = App::new();
    app.skip_splash();
    for e in events {
        app.handle_agent_event(e);
    }
    assert!(app.debugger_open);
    app
}

fn row_with<'a>(rows: &'a [String], needle: &str) -> Option<&'a String> {
    rows.iter().find(|r| r.contains(needle))
}

#[test]
fn test_w34_menu_should_show_agent_and_subtask_id_and_the_workers_description() {
    let app = app_with_menu(vec![
        started("t42", "rust-expert", "refactor the parser module"),
        paused(Some(("t42", "rust-expert"))),
    ]);

    let rows = menu_rows(&rows(&app, 140, 40));

    let line = row_with(&rows, "rust-expert")
        .unwrap_or_else(|| panic!("no worker line:\n{}", rows.join("\n")));
    assert!(line.contains("t42"), "{line:?}");
    assert!(line.contains("refactor the parser module"), "{line:?}");
}

#[test]
fn test_w34_menu_should_cut_a_long_description_to_the_menu_width() {
    let app = app_with_menu(vec![
        started("t42", "rust-expert", LONG),
        paused(Some(("t42", "rust-expert"))),
    ]);

    let rows = menu_rows(&rows(&app, 70, 40));

    let line = row_with(&rows, "rust-expert")
        .unwrap_or_else(|| panic!("no worker line:\n{}", rows.join("\n")));
    assert!(line.contains("t42"), "{line:?}");
    assert!(
        line.contains("refactor the parser"),
        "the start is kept: {line:?}"
    );
    assert!(!line.contains("ENDMARK"), "the end is cut: {line:?}");
}

#[test]
fn test_w34_menu_when_no_worker_started_was_seen_should_still_name_agent_and_id() {
    let app = app_with_menu(vec![paused(Some(("t9", "code-reviewer")))]);

    let rows = menu_rows(&rows(&app, 140, 40));

    let line = row_with(&rows, "code-reviewer")
        .unwrap_or_else(|| panic!("no worker line:\n{}", rows.join("\n")));
    assert!(line.contains("t9"), "{line:?}");
}

#[test]
fn test_w34_menu_for_the_main_agent_should_show_no_worker_line() {
    let app = app_with_menu(vec![
        started("t42", "rust-expert", "refactor the parser module"),
        paused(None),
    ]);

    let rows = menu_rows(&rows(&app, 140, 40)).join("\n");

    assert!(!rows.contains("rust-expert"), "{rows}");
    assert!(!rows.contains("t42"), "{rows}");
    assert!(!rows.to_lowercase().contains("worker"), "{rows}");
}

#[test]
fn test_w34_a_later_pause_from_another_worker_should_replace_the_shown_worker() {
    let mut app = app_with_menu(vec![
        started("t1", "rust-expert", "first job"),
        started("t2", "code-reviewer", "second job"),
        paused(Some(("t1", "rust-expert"))),
    ]);
    app.handle_agent_event(paused(Some(("t2", "code-reviewer"))));

    let rows = menu_rows(&rows(&app, 140, 40)).join("\n");

    assert!(rows.contains("code-reviewer"), "{rows}");
    assert!(rows.contains("second job"), "{rows}");
    assert!(!rows.contains("first job"), "{rows}");
}

#[test]
fn test_w34_a_main_agent_pause_after_a_worker_pause_should_drop_the_worker_line() {
    let mut app = app_with_menu(vec![
        started("t1", "rust-expert", "first job"),
        paused(Some(("t1", "rust-expert"))),
    ]);
    app.handle_agent_event(paused(None));

    let rows = menu_rows(&rows(&app, 140, 40)).join("\n");

    assert!(!rows.contains("first job"), "{rows}");
    assert!(!rows.contains("rust-expert"), "{rows}");
}
