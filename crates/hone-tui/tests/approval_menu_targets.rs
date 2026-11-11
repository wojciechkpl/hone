#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W42 of `docs/plans/worker-tools-spec.md`: the approval menu always shows
//! what a call touches. Short identifying fields (`path`, `file_path`,
//! `command`, `url`, `pattern`) come first; every value is capped at a few
//! wrapped rows with "… (N more chars)". Existing APIs only: real failures.

use hone_proto::AgentEvent;
use hone_tui::{app::App, render::root};
use ratatui::{Terminal, backend::TestBackend};

fn screen(app: &App, w: u16, h: u16) -> Vec<String> {
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

fn menu_for(name: &str, args: serde_json::Value) -> App {
    let mut app = App::new();
    app.skip_splash();
    app.handle_agent_event(AgentEvent::ToolCallPaused {
        id: "c1".to_owned(),
        name: name.to_owned(),
        args,
        worker: None,
    });
    assert!(app.debugger_open);
    app
}

fn row_of(rows: &[String], needle: &str) -> Option<usize> {
    rows.iter().position(|r| r.contains(needle))
}

#[test]
fn test_w42_write_file_with_huge_content_should_still_show_its_path_and_the_footer() {
    for (w, h) in [(200u16, 60u16), (100, 40)] {
        let app = menu_for(
            "write_file",
            serde_json::json!({ "path": "TARGET_PATH_MARKER.rs", "content": "x".repeat(12_000) }),
        );

        let rows = screen(&app, w, h);

        assert!(
            row_of(&rows, "TARGET_PATH_MARKER").is_some(),
            "the path must be visible at {w}x{h}:\n{}",
            rows.join("\n")
        );
        assert!(
            row_of(&rows, "proceed").is_some(),
            "the key hints must be visible at {w}x{h}"
        );
    }
}

#[test]
fn test_w42_a_long_value_should_be_capped_with_a_more_chars_note() {
    let app = menu_for(
        "write_file",
        serde_json::json!({ "path": "p.rs", "content": "x".repeat(12_000) }),
    );

    let rows = screen(&app, 100, 40);

    assert!(
        row_of(&rows, "more chars").is_some(),
        "a capped value says how much is hidden:\n{}",
        rows.join("\n")
    );
}

#[test]
fn test_w42_a_short_value_should_not_get_a_more_chars_note() {
    let app = menu_for(
        "write_file",
        serde_json::json!({ "path": "p.rs", "content": "fn main() {}" }),
    );

    let rows = screen(&app, 100, 40);

    assert!(row_of(&rows, "more chars").is_none());
    assert!(row_of(&rows, "fn main() {}").is_some());
}

#[test]
fn test_w42_identifying_field_should_come_before_other_keys_that_sort_earlier() {
    // `aaa_first` sorts before `command`; the command must still be listed first.
    let app = menu_for(
        "shell",
        serde_json::json!({ "aaa_first": "other", "command": "cargo test", "timeout_ms": 5 }),
    );

    let rows = screen(&app, 100, 40);

    let command = row_of(&rows, "cargo test").expect("command shown");
    let other = row_of(&rows, "aaa_first").expect("other key shown");
    assert!(
        command < other,
        "command row {command} vs aaa_first row {other}"
    );
}

#[test]
fn test_w42_each_identifying_key_should_be_listed_before_the_others() {
    for key in ["path", "file_path", "command", "url", "pattern"] {
        let mut args = serde_json::Map::new();
        args.insert("aaa_other".to_owned(), serde_json::json!("OTHERVALUE"));
        args.insert(key.to_owned(), serde_json::json!("IDENTIFYING"));
        let app = menu_for("shell", serde_json::Value::Object(args));

        let rows = screen(&app, 100, 40);

        let id = row_of(&rows, "IDENTIFYING").unwrap_or_else(|| panic!("{key} not shown"));
        let other = row_of(&rows, "OTHERVALUE").expect("other shown");
        assert!(id < other, "{key} must come first (rows {id} vs {other})");
    }
}
