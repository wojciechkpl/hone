//! Plan-preview overlay — rendered when the user invokes `/plan <prompt>`
//! and the in-process orchestrator has returned a `TaskPlan`.
//!
//! Shows the goal, the cost estimate (real lead-LLM cost + projected
//! per-worker spend), and the subtasks. Footer lists the keybindings
//! handled by `App::handle_key_plan_preview`.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};

use crate::app::App;

/// Render the plan-preview overlay into `area`. Called from `render/root.rs`
/// after every other widget so it sits on top.
pub fn render_plan_preview(app: &App, frame: &mut Frame, area: Rect) {
    let preview = match app.plan_preview() {
        Some(p) => p,
        None => return,
    };
    let theme = app.theme();

    let border_style = Style::default().fg(theme.accent_hot);
    let title_style = Style::default()
        .fg(theme.accent_hot)
        .add_modifier(Modifier::BOLD);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(" Plan Preview ", title_style));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 4 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // Goal line.
    let goal_label = Style::default().fg(theme.text_muted);
    lines.push(Line::from(vec![
        Span::styled("  goal  ", goal_label),
        Span::styled(
            preview.goal.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));

    // Cost line — paid (real) and projected (paid + N × constant).
    let cost_label = Style::default().fg(theme.text_muted);
    lines.push(Line::from(vec![
        Span::styled("  cost  ", cost_label),
        Span::raw(format!(
            "paid ${:.4}  ·  projected ~${:.4}",
            preview.lead_cost_usd, preview.estimated_total_usd,
        )),
    ]));

    // Blank separator.
    lines.push(Line::from(""));

    // Subtask list.
    let subtask_count = preview.subtasks.len();
    lines.push(Line::from(Span::styled(
        format!("  subtasks ({subtask_count})"),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    let agent_style = Style::default()
        .fg(theme.accent_hot)
        .add_modifier(Modifier::BOLD);
    let id_style = Style::default().fg(theme.text_muted);

    for st in preview.subtasks.iter().take(20) {
        lines.push(Line::from(vec![
            Span::raw("  · "),
            Span::styled(st.agent.clone(), agent_style),
            Span::raw(" "),
            Span::styled(format!("[{}]", st.id), id_style),
            Span::raw(" "),
            Span::raw(st.description.clone()),
        ]));
    }
    if subtask_count > 20 {
        lines.push(Line::from(Span::styled(
            format!("  … +{} more", subtask_count - 20),
            id_style,
        )));
    }

    // Blank separator before footer.
    lines.push(Line::from(""));

    // Footer with keybinding hints.
    let footer_style = Style::default().fg(theme.text_muted);
    lines.push(Line::from(Span::styled(
        "  r/Enter run    e edit (restore prompt)    c/Esc cancel",
        footer_style,
    )));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(0)])
        .split(inner);

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, chunks[0]);
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::plan_preview::{PlanPreviewState, PlanPreviewSubtask};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn fixture() -> PlanPreviewState {
        PlanPreviewState {
            prompt: "refactor auth".into(),
            goal: "Refactor the auth module".into(),
            subtasks: vec![
                PlanPreviewSubtask {
                    id: "1".into(),
                    agent: "rust-expert".into(),
                    description: "Move session state to a dedicated struct".into(),
                },
                PlanPreviewSubtask {
                    id: "2".into(),
                    agent: "code-reviewer".into(),
                    description: "Audit the new module for race conditions".into(),
                },
            ],
            lead_cost_usd: 0.012,
            estimated_total_usd: 0.112,
        }
    }

    #[test]
    fn render_plan_preview_shows_goal_and_costs() {
        let mut app = App::new();
        app.set_plan_preview(fixture());

        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).expect("terminal");
        term.draw(|f| render_plan_preview(&app, f, f.area())).unwrap();

        let buf = term.backend().buffer().clone();
        let mut content = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                content.push_str(buf[(x, y)].symbol());
            }
            content.push('\n');
        }

        assert!(content.contains("Plan Preview"), "title visible: {content}");
        assert!(content.contains("Refactor the auth module"), "goal visible");
        assert!(content.contains("$0.0120"), "lead cost visible");
        assert!(content.contains("$0.1120"), "projected cost visible");
        assert!(content.contains("rust-expert"), "subtask agent visible");
        assert!(content.contains("code-reviewer"), "second subtask visible");
        assert!(
            content.contains("r/Enter run"),
            "footer keybinds visible: {content}",
        );
    }

    #[test]
    fn render_plan_preview_truncates_long_subtask_list() {
        let mut state = fixture();
        // Push 25 subtasks — should show 20 + "+5 more".
        state.subtasks.clear();
        for i in 0..25 {
            state.subtasks.push(PlanPreviewSubtask {
                id: i.to_string(),
                agent: format!("agent-{i}"),
                description: format!("step {i}"),
            });
        }

        let mut app = App::new();
        app.set_plan_preview(state);

        let backend = TestBackend::new(80, 30);
        let mut term = Terminal::new(backend).expect("terminal");
        term.draw(|f| render_plan_preview(&app, f, f.area())).unwrap();

        let buf = term.backend().buffer().clone();
        let mut content = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                content.push_str(buf[(x, y)].symbol());
            }
            content.push('\n');
        }

        assert!(content.contains("+5 more"), "truncation indicator visible");
    }

    #[test]
    fn render_plan_preview_noop_without_state() {
        let app = App::new();
        let backend = TestBackend::new(40, 6);
        let mut term = Terminal::new(backend).expect("terminal");
        term.draw(|f| render_plan_preview(&app, f, f.area())).unwrap();
        // Should not panic; nothing to assert beyond "didn't crash".
    }
}
