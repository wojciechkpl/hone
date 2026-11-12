#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR3 byte-identity: building receipts must never change the requests the agent
//! sends. This records the requests of a fixed, READ-ONLY scripted session (no
//! write or test tools, so phase compaction cannot change it) and compares them
//! with `fixtures/context_request_golden.json`, generated from the code BEFORE
//! receipts existed (`UPDATE_GOLDEN=1 cargo test ... --test context_request_golden`
//! rewrites it; only do that on a build without receipts).
//!
//! It compiles and passes against today's code; it must keep passing.

mod receipts_support;

use std::sync::Arc;

use hone_core::{Skill, SkillRegistry};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use receipts_support::*;
use serde_json::json;

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/context_request_golden.json"
);

async fn seeded_store() -> Arc<InMemoryStore> {
    let store = Arc::new(InMemoryStore::new());
    for (id, text, kind) in [
        (
            "m1",
            "prefer small commits when refactoring the parser module",
            MemoryKind::Feedback,
        ),
        (
            "m2",
            "the parser module lives in src/parser and has golden tests",
            MemoryKind::Project,
        ),
        ("m3", "unrelated note about lunch", MemoryKind::User),
    ] {
        store
            .record(Memory {
                id: id.to_owned(),
                content: text.to_owned(),
                tags: vec![],
                weight: 1.0,
                created_at: 1_000_000,
                decay_at: None,
                kind,
                description: None,
                embedding: None,
            })
            .await
            .unwrap();
    }
    store
}

fn skills() -> Arc<SkillRegistry> {
    let mut reg = SkillRegistry::new();
    reg.register(Skill {
        name: "refactor-helper".to_owned(),
        description: "help with refactors".to_owned(),
        triggers: vec!["refactor".to_owned()],
        tools: vec![],
        model: None,
        always: false,
        body: "Keep diffs small.".to_owned(),
    });
    Arc::new(reg)
}

#[tokio::test]
async fn test_cr3_requests_of_a_fixed_read_only_session_should_match_the_golden_fixture() {
    let store = seeded_store().await;
    let mut r = rig(
        vec![vec![read("r1")], vec![read("r2")]],
        Opts {
            memory: Some(store),
            skills: Some(skills()),
            ..Default::default()
        },
    );

    r.turn("refactor the parser module").await;
    r.turn("thanks, now add docs").await;

    let recorded: Vec<serde_json::Value> = (0..r.provider.request_count())
        .map(|i| {
            let req = r.provider.request(i);
            json!({
                "model": req.model.to_string(),
                "messages": req.messages,
                "tool_names": req.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let actual = serde_json::to_string_pretty(&recorded).unwrap() + "\n";

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(GOLDEN, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(GOLDEN).expect("golden fixture missing");
    assert_eq!(actual, expected, "the requests the agent sends changed");
}
