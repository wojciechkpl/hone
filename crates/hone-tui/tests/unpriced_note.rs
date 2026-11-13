#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U13 of `docs/plans/usage-and-pricing-spec.md`: `App::handle_agent_event` shows
//! `AgentEvent::UnpricedModel { model }` as one system line naming the model (and saying it has
//! no price and is not counted towards cost or [budget]), once per model, whichever path
//! delivers the event.

use hone_proto::{AgentEvent, TokenUsage};
use hone_tui::app::App;
use hone_tui::message::DisplayRole;

fn unpriced(model: &str) -> AgentEvent {
    AgentEvent::UnpricedModel {
        model: model.to_owned(),
    }
}

fn notes(app: &App, model: &str) -> Vec<String> {
    app.messages()
        .iter()
        .filter(|m| matches!(m.role, DisplayRole::System) && m.content.contains(model))
        .map(|m| m.content.clone())
        .collect()
}

#[test]
fn test_u13_an_unpriced_model_event_should_add_one_system_line_naming_the_model() {
    let mut app = App::new();

    app.handle_agent_event(unpriced("ollama:llama3"));

    let lines = notes(&app, "ollama:llama3");
    assert_eq!(lines.len(), 1, "{:?}", app.messages());
    let text = lines[0].to_lowercase();
    assert!(text.contains("no price"), "{text}");
    assert!(text.contains("budget"), "{text}");
}

#[test]
fn test_u13_the_same_model_should_be_shown_once_even_across_turns() {
    let mut app = App::new();

    app.handle_agent_event(unpriced("ollama:llama3"));
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage::default(),
        cost_usd: 0.0,
        plan: None,
    });
    app.handle_agent_event(unpriced("ollama:llama3"));
    app.handle_agent_event(unpriced("ollama:llama3"));

    assert_eq!(notes(&app, "ollama:llama3").len(), 1);
}

#[test]
fn test_u13_another_model_should_get_its_own_line() {
    let mut app = App::new();

    app.handle_agent_event(unpriced("ollama:llama3"));
    app.handle_agent_event(unpriced("openai:other"));

    assert_eq!(notes(&app, "ollama:llama3").len(), 1);
    assert_eq!(notes(&app, "openai:other").len(), 1);
}
