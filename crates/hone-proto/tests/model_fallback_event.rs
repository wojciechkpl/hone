#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! N1 of `docs/plans/fallback-notice-spec.md`: `ProviderEvent::ModelFallback` and
//! `AgentEvent::ModelFallback` serialize as `{"type":"model_fallback","requested":…,
//! "fallback":…,"reason":…}` and round-trip.

use hone_proto::{AgentEvent, ProviderEvent};
use serde_json::json;

fn expected() -> serde_json::Value {
    json!({
        "type": "model_fallback",
        "requested": "anthropic:claude-x",
        "fallback": "openai:gpt-4o",
        "reason": "overloaded",
    })
}

#[test]
fn test_n1_provider_event_model_fallback_serializes_with_the_spec_shape() {
    let event = ProviderEvent::ModelFallback {
        requested: "anthropic:claude-x".into(),
        fallback: "openai:gpt-4o".into(),
        reason: "overloaded".into(),
    };
    assert_eq!(serde_json::to_value(&event).unwrap(), expected());
}

#[test]
fn test_n1_provider_event_model_fallback_roundtrips() {
    let event = ProviderEvent::ModelFallback {
        requested: "a:b".into(),
        fallback: "c:d".into(),
        reason: "why \"quoted\" é".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert_eq!(serde_json::from_str::<ProviderEvent>(&json).unwrap(), event);
}

#[test]
fn test_n1_provider_event_model_fallback_deserializes_from_the_spec_json() {
    let event: ProviderEvent = serde_json::from_value(expected()).unwrap();
    assert_eq!(
        event,
        ProviderEvent::ModelFallback {
            requested: "anthropic:claude-x".into(),
            fallback: "openai:gpt-4o".into(),
            reason: "overloaded".into(),
        }
    );
}

#[test]
fn test_n1_agent_event_model_fallback_serializes_with_the_spec_shape() {
    let event = AgentEvent::ModelFallback {
        requested: "anthropic:claude-x".into(),
        fallback: "openai:gpt-4o".into(),
        reason: "overloaded".into(),
    };
    assert_eq!(serde_json::to_value(&event).unwrap(), expected());
}

#[test]
fn test_n1_agent_event_model_fallback_roundtrips() {
    let event = AgentEvent::ModelFallback {
        requested: "a:b".into(),
        fallback: "c:d".into(),
        reason: "".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert_eq!(serde_json::from_str::<AgentEvent>(&json).unwrap(), event);
}
