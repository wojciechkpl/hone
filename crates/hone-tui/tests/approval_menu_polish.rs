#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W40 of `docs/plans/worker-tools-spec.md`: approval menu polish. These use
//! only existing APIs (the `worker` field and `WorkerRef` exist since W33), so
//! the reds are real assertion failures.

use hone_proto::{AgentEvent, WorkerRef};
use hone_tui::{app::App, render::root};
use ratatui::{Terminal, backend::TestBackend};

fn started(task: &str, agent: &str, description: &str) -> AgentEvent {
    AgentEvent::WorkerStarted {
        task_id: task.to_owned(),
        agent: agent.to_owned(),
        description: description.to_owned(),
    }
}

fn paused(args: serde_json::Value, worker: Option<(&str, &str)>) -> AgentEvent {
    AgentEvent::ToolCallPaused {
        id: "c1".to_owned(),
        name: "shell".to_owned(),
        args,
        worker: worker.map(|(task, agent)| WorkerRef {
            task_id: task.to_owned(),
            agent: agent.to_owned(),
        }),
    }
}

fn render_rows(app: &App, w: u16, h: u16) -> Vec<String> {
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

/// Rows of the menu box (title row to bottom border), cut to the box columns,
/// so the conversation pane behind it does not count.
fn menu_rows(all: &[String]) -> Vec<String> {
    let title = all
        .iter()
        .position(|r| r.contains("Agent Debugger"))
        .unwrap_or_else(|| panic!("no menu:\n{}", all.join("\n")));
    let chars: Vec<char> = all[title].chars().collect();
    let left = chars.iter().position(|c| *c == '\u{256d}').expect("corner");
    let right = chars
        .iter()
        .rposition(|c| *c == '\u{256e}')
        .expect("corner");
    let mut out = Vec::new();
    for row in &all[title..] {
        out.push(
            row.chars()
                .skip(left)
                .take(right - left + 1)
                .collect::<String>(),
        );
        if row.chars().nth(left) == Some('\u{2570}') {
            break;
        }
    }
    out
}

fn app_with(events: Vec<AgentEvent>) -> App {
    let mut app = App::new();
    app.skip_splash();
    for e in events {
        app.handle_agent_event(e);
    }
    assert!(app.debugger_open);
    app
}

fn squashed(rows: &[String]) -> String {
    rows.join("")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{2502}')
        .collect()
}

#[test]
fn test_w40_footer_should_stay_visible_on_a_short_terminal() {
    let many_args: serde_json::Map<String, serde_json::Value> = (0..30)
        .map(|i| {
            (
                format!("key{i:02}"),
                serde_json::json!(format!("value {i}")),
            )
        })
        .collect();
    let app = app_with(vec![paused(serde_json::Value::Object(many_args), None)]);

    for height in [10u16, 12, 16] {
        let rows = render_rows(&app, 100, height);
        assert!(
            rows.iter().any(|r| r.contains("proceed")),
            "the key hints must show at height {height}:\n{}",
            rows.join("\n")
        );
    }
}

#[test]
fn test_w40_long_argument_line_should_wrap_instead_of_being_cut() {
    let command = format!("cargo test {} ENDMARK", "--flag value ".repeat(20));
    let app = app_with(vec![paused(
        serde_json::json!({ "command": command }),
        None,
    )]);

    let rows = menu_rows(&render_rows(&app, 80, 40));

    let text = squashed(&rows);
    assert!(text.contains("cargotest"), "the start is shown");
    assert!(
        text.contains("ENDMARK"),
        "the end of a long argument line must wrap into view:\n{}",
        rows.join("\n")
    );
}

#[test]
fn test_w40_worker_line_should_quote_the_planner_written_description() {
    let description = "refactor the parser module";
    let app = app_with(vec![
        started("t42", "rust-expert", description),
        paused(serde_json::json!({}), Some(("t42", "rust-expert"))),
    ]);

    let rows = menu_rows(&render_rows(&app, 140, 40));

    let line = rows
        .iter()
        .find(|r| r.contains("rust-expert"))
        .unwrap_or_else(|| panic!("no worker line:\n{}", rows.join("\n")));
    let quoted = [
        format!("\"{description}\""),
        format!("\u{201c}{description}\u{201d}"),
        format!("'{description}'"),
    ];
    assert!(
        quoted.iter().any(|q| line.contains(q.as_str())),
        "the description is marked as quoted task text: {line:?}"
    );
}

#[test]
fn test_w40_plan_created_should_forget_earlier_workers_descriptions() {
    let mut app = App::new();
    app.skip_splash();
    app.handle_agent_event(started("t42", "rust-expert", "STALE-DESCRIPTION"));
    app.handle_agent_event(AgentEvent::PlanCreated {
        goal: "a new goal".to_owned(),
        subtask_count: 1,
    });
    // The new run reuses task id t42 but its WorkerStarted was not seen.
    app.handle_agent_event(paused(serde_json::json!({}), Some(("t42", "rust-expert"))));

    let rows = menu_rows(&render_rows(&app, 140, 40));

    let text = rows.join("\n");
    assert!(text.contains("rust-expert"), "{text}");
    assert!(text.contains("t42"), "{text}");
    assert!(
        !text.contains("STALE-DESCRIPTION"),
        "a description from an earlier plan must not be shown:\n{text}"
    );
}
