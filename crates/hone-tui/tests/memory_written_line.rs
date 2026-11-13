#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W7 of `docs/plans/memory-write-tool-spec.md`: `App::handle_agent_event` renders
//! `AgentEvent::MemoryWritten { id, kind, status }` as exactly one system message
//! `memory <status>: <kind> <id>` (for example `memory added: feedback 0190-abc`).

use hone_proto::AgentEvent;
use hone_tui::app::App;
use hone_tui::message::DisplayRole;

fn written(id: &str, kind: &str, status: &str) -> AgentEvent {
    AgentEvent::MemoryWritten {
        id: id.to_owned(),
        kind: kind.to_owned(),
        status: status.to_owned(),
    }
}

fn system_lines(app: &App) -> Vec<String> {
    app.messages()
        .iter()
        .filter(|m| matches!(m.role, DisplayRole::System))
        .map(|m| m.content.clone())
        .filter(|t| t.starts_with("memory "))
        .collect()
}

#[test]
fn test_w7_a_memory_written_event_should_push_one_system_line() {
    let mut app = App::new();

    app.handle_agent_event(written("0190-abc", "feedback", "added"));

    assert_eq!(system_lines(&app), ["memory added: feedback 0190-abc"]);
    assert!(matches!(app.messages().last().unwrap().role, DisplayRole::System));
}

#[test]
fn test_w7_each_write_should_get_its_own_line_with_its_status() {
    let mut app = App::new();

    app.handle_agent_event(written("a1", "user", "added"));
    app.handle_agent_event(written("a1", "user", "updated"));

    assert_eq!(
        system_lines(&app),
        ["memory added: user a1", "memory updated: user a1"]
    );
}
