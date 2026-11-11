//! N11 of `docs/plans/fallback-notice-spec.md`: the SSE frame names a model fallback
//! `model_fallback` (not `unknown`). The execution / orchestration maps are private;
//! their cases are in `#[cfg(test)]` modules next to them.

use hone_proto::AgentEvent;
use hone_server::sse::encode_event;

#[test]
fn test_n11_encode_event_names_a_model_fallback_model_fallback() {
    let event = AgentEvent::ModelFallback {
        requested: "anthropic:claude-x".to_owned(),
        fallback: "openai:gpt-4o".to_owned(),
        reason: "overloaded".to_owned(),
    };

    let encoded = encode_event(&event, 7);

    assert!(encoded.starts_with("id: 7\n"), "{encoded}");
    assert!(encoded.contains("event: model_fallback\n"), "{encoded}");
    assert!(!encoded.contains("event: unknown"), "{encoded}");
    assert!(
        encoded.contains("\"fallback\":\"openai:gpt-4o\""),
        "{encoded}"
    );
    assert!(encoded.ends_with("\n\n"), "{encoded}");
}
