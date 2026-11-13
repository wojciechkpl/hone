#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E7 of `docs/plans/env-snapshot-spec.md`: `Slot` gains an `Environment`
//! variant, `environment` on the wire. Receipts built with it round-trip.

use hone_proto::{BlockReceipt, ScoredSource, Slot, SourceRef, StrategyId};

#[test]
fn test_e7_environment_slot_should_be_snake_case_on_the_wire_and_round_trip() {
    assert_eq!(serde_json::to_value(Slot::Environment).unwrap(), serde_json::json!("environment"));
    let back: Slot = serde_json::from_value(serde_json::json!("environment")).unwrap();
    assert_eq!(back, Slot::Environment);
}

#[test]
fn test_e7_an_environment_block_with_command_sources_should_round_trip() {
    let block = BlockReceipt {
        slot: Slot::Environment,
        strategy: StrategyId {
            slot: Slot::Environment,
            name: "env-snapshot".to_owned(),
            version: "0.1.0".to_owned(),
            params_sha256: "ab".repeat(32),
        },
        tokens: 120,
        sources: vec![
            ScoredSource {
                source: SourceRef::Command {
                    argv: vec!["rustc".to_owned(), "--version".to_owned()],
                    sha256: "cd".repeat(32),
                },
                score: None,
            },
            ScoredSource {
                source: SourceRef::Command {
                    argv: vec!["<builtin:listing>".to_owned()],
                    sha256: "ef".repeat(32),
                },
                score: None,
            },
        ],
        duration_ms: 40,
    };
    let v = serde_json::to_value(&block).unwrap();
    assert_eq!(v["slot"], "environment");
    assert_eq!(v["sources"][0]["source"]["kind"], "command");
    let back: BlockReceipt = serde_json::from_value(v).unwrap();
    assert_eq!(back, block);
}
