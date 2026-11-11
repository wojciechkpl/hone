#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W49 (TUI part) of `docs/plans/worker-tools-spec.md`: identifying fields
//! (`command`, `url`, ...) get a larger cap than bulk fields, so a long command
//! is not cut after a few rows; bulk fields such as `content` stay capped.

use hone_proto::AgentEvent;
use hone_tui::{app::App, render::root};
use ratatui::{Terminal, backend::TestBackend};

fn screen(app: &App, w: u16, h: u16) -> String {
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
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn menu_for(name: &str, args: serde_json::Value) -> App {
    let mut app = App::new();
    app.skip_splash();
    app.handle_agent_event(AgentEvent::ToolCallPaused {
        id: "c1".to_owned(),
        name: name.to_owned(),
        args,
        worker: None,
    });
    app
}

#[test]
fn test_w49_a_long_shell_command_should_show_its_end_on_a_normal_terminal() {
    // About 15 wrapped rows at ~110 columns: 120 words of "word " plus a tail.
    let command = format!("{} ENDMARK", "word ".repeat(300));
    let app = menu_for("shell", serde_json::json!({ "command": command }));

    let shown = screen(&app, 120, 40);

    assert!(
        shown.contains("ENDMARK"),
        "the tail of a long command must be visible (it decides what runs):\n{shown}"
    );
    assert!(shown.contains("proceed"), "the key hints stay visible");
}

#[test]
fn test_w49_a_bulk_field_should_still_be_capped_with_a_more_chars_note() {
    let app = menu_for(
        "write_file",
        serde_json::json!({ "path": "p.rs", "content": "x".repeat(12_000) }),
    );

    let shown = screen(&app, 120, 40);

    assert!(shown.contains("more chars"), "{shown}");
    assert!(shown.contains("p.rs"));
    assert!(shown.contains("proceed"));
}
