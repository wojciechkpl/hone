use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hone_core::ContextManager;
use hone_proto::Message;

fn bench_context_token_estimation(c: &mut Criterion) {
    let mut ctx = ContextManager::new(100_000);
    let msg = Message::user("This is a test message with some content to estimate tokens for.");

    c.bench_function("estimate_tokens_per_message", |b| {
        b.iter(|| {
            ctx.add_message(black_box(msg.clone()));
        });
    });
}

fn bench_context_build_messages(c: &mut Criterion) {
    let mut ctx = ContextManager::new(100_000);
    for i in 0..50 {
        ctx.add_message(Message::user(format!("Message {i}")));
        ctx.add_message(Message::assistant(format!("Response {i}")));
    }

    c.bench_function("build_messages_50", |b| {
        b.iter(|| black_box(ctx.build_messages(black_box("system prompt"))));
    });
}

criterion_group!(
    benches,
    bench_context_token_estimation,
    bench_context_build_messages
);
criterion_main!(benches);
