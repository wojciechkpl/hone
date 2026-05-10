use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hone_tui::{App, render::root};
use ratatui::{Terminal, backend::TestBackend};

fn bench_render_empty(c: &mut Criterion) {
    let mut app = App::new();
    app.skip_splash();
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();

    c.bench_function("render_empty_tui", |b| {
        b.iter(|| {
            terminal
                .draw(|frame| root::render(black_box(&app), frame))
                .unwrap();
        });
    });
}

fn bench_render_with_messages(c: &mut Criterion) {
    use hone_proto::AgentEvent;
    let mut app = App::new();
    app.skip_splash();

    // Add 50 messages
    for i in 0..50 {
        app.handle_agent_event(AgentEvent::Token {
            text: format!("Message {i} with some content\n"),
        });
        app.handle_agent_event(AgentEvent::Done {
            usage: hone_proto::TokenUsage::default(),
            cost_usd: 0.0,
            plan: None,
        });
    }

    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();

    c.bench_function("render_50_messages", |b| {
        b.iter(|| {
            terminal
                .draw(|frame| root::render(black_box(&app), frame))
                .unwrap();
        });
    });
}

criterion_group!(benches, bench_render_empty, bench_render_with_messages);
criterion_main!(benches);
