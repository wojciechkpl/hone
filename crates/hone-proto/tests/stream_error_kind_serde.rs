#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E1 (`docs/plans/provider-error-kind-spec.md`): `ProviderEvent::Error` carries a
//! `StreamErrorKind`; JSON without `kind` reads as `Unclassified`.

use hone_proto::{ProviderEvent, StreamErrorKind};

const KINDS: [StreamErrorKind; 6] = [
    StreamErrorKind::Unclassified,
    StreamErrorKind::Interrupted,
    StreamErrorKind::IdleTimeout,
    StreamErrorKind::Provider { status: None },
    StreamErrorKind::Provider { status: Some(400) },
    StreamErrorKind::Provider { status: Some(529) },
];

#[test]
fn test_e1_every_kind_round_trips_through_json() {
    for kind in KINDS {
        let event = ProviderEvent::Error {
            message: "boom".to_owned(),
            kind,
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: ProviderEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event, "{kind:?} via {json}");
    }
}

#[test]
fn test_e1_the_kind_alone_round_trips() {
    for kind in KINDS {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(
            serde_json::from_str::<StreamErrorKind>(&json).unwrap(),
            kind,
            "{json}"
        );
    }
}

#[test]
fn test_e1_json_without_a_kind_reads_as_unclassified() {
    let event: ProviderEvent = serde_json::from_str(r#"{"type":"error","message":"x"}"#).unwrap();
    assert_eq!(
        event,
        ProviderEvent::Error {
            message: "x".to_owned(),
            kind: StreamErrorKind::Unclassified
        }
    );
}

#[test]
fn test_e1_the_default_kind_is_unclassified_and_the_message_is_unchanged() {
    assert_eq!(StreamErrorKind::default(), StreamErrorKind::Unclassified);
    let json = serde_json::to_value(ProviderEvent::Error {
        message: "openai response timed out: x".to_owned(),
        kind: StreamErrorKind::IdleTimeout,
    })
    .unwrap();
    assert_eq!(json["message"], "openai response timed out: x");
}
