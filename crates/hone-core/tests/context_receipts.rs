#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR1-CR7 and CR10 of `docs/plans/context-receipts-spec.md`, through a real
//! `Agent` with a scripted provider that records the requests it receives.
//!
//! API required (new): everything listed in `crates/hone-proto/tests/receipt.rs`,
//! plus `hone_core::estimate_text_tokens(&str) -> u32` re-exported at the crate
//! root (the agent's token estimator, `context::estimate_text_tokens`).
//!
//! Strategy names are hard-coded: recall `bm25` / `bm25+semantic:rrf`, injection
//! `skeleton:<as_str>` (`skeleton:flat`), system prompt and history any name.
//! Recall `params_sha256` is `sha256_hex` of `{"exclude":["outcome"],"limit":5}`.

mod receipts_support;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use hone_core::{SkeletonStrategy, Skill, SkillRegistry, estimate_text_tokens};
use hone_index::SqliteIndex;
use hone_memory::{
    EmbedError, Embedder, Embedding, InMemoryStore, Memory, MemoryKind, MemoryStore,
};
use hone_proto::{
    AgentEvent, ContextReceipt, Message, Role, Slot, SourceRef, message_sha256, request_sha256,
    sha256_hex,
};
use receipts_support::*;

fn receipts(events: &[AgentEvent]) -> Vec<ContextReceipt> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .collect()
}

fn slots(r: &ContextReceipt) -> Vec<Slot> {
    r.blocks.iter().map(|b| b.slot).collect()
}

fn block(r: &ContextReceipt, slot: Slot) -> &hone_proto::BlockReceipt {
    r.blocks
        .iter()
        .find(|b| b.slot == slot)
        .unwrap_or_else(|| panic!("no {slot:?} block in {:?}", slots(r)))
}

fn memory_ids(b: &hone_proto::BlockReceipt) -> Vec<String> {
    b.sources
        .iter()
        .filter_map(|s| match &s.source {
            SourceRef::Memory { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn mem(id: &str, content: &str, kind: MemoryKind, embedding: Option<Vec<f32>>) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        kind,
        description: None,
        embedding,
    }
}

async fn store_with(memories: Vec<Memory>) -> Arc<InMemoryStore> {
    let store = Arc::new(InMemoryStore::new());
    for m in memories {
        store.record(m).await.unwrap();
    }
    store
}

fn skill(name: &str, trigger: &str, body: &str) -> Skill {
    Skill {
        name: name.to_owned(),
        description: format!("about {name}"),
        triggers: vec![trigger.to_owned()],
        tools: vec![],
        model: None,
        always: false,
        body: body.to_owned(),
    }
}

fn registry(skills: Vec<Skill>) -> Arc<SkillRegistry> {
    let mut reg = SkillRegistry::new();
    for s in skills {
        reg.register(s);
    }
    Arc::new(reg)
}

// ---- CR1: one receipt per model call -------------------------------------------------

#[tokio::test]
async fn test_cr1_every_request_should_be_preceded_by_exactly_one_receipt() {
    let mut r = rig(vec![vec![read("r1")], vec![read("r2")]], Opts::default());

    let events = r.turn("look around").await;

    assert_eq!(
        r.provider.request_count(),
        3,
        "two tool rounds and an answer"
    );
    let got = receipts(&events);
    assert_eq!(got.len(), 3);
    assert_eq!(
        *r.provider.assembled_before_request.lock().unwrap(),
        [1, 2, 3],
        "the k-th receipt was sent before the k-th request reached the provider"
    );
}

#[tokio::test]
async fn test_cr1_turn_and_call_should_count_from_one_and_call_should_restart_each_turn() {
    let mut r = rig(vec![vec![read("r1")], vec![read("r2")]], Opts::default());

    let first = receipts(&r.turn("one").await);
    let second = receipts(&r.turn("two").await);

    let numbers: Vec<(u32, u32)> = first
        .iter()
        .chain(&second)
        .map(|x| (x.turn, x.call))
        .collect();
    assert_eq!(numbers, [(1, 1), (1, 2), (1, 3), (2, 1)]);
}

// ---- CR2: the request hash -------------------------------------------------------------

#[tokio::test]
async fn test_cr2_request_hash_should_identify_the_recorded_request() {
    let mut r = rig(vec![vec![read("r1")], vec![read("r2")]], Opts::default());

    let got = receipts(&r.turn("look around").await);

    assert_eq!(got.len(), r.provider.request_count());
    for (i, receipt) in got.iter().enumerate() {
        let req = r.provider.request(i);
        let names: Vec<String> = req.tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(
            receipt.request_sha256,
            request_sha256(&req.messages, &names),
            "call {}",
            i + 1
        );
        assert_eq!(
            receipt.tools_offered, names,
            "tools_offered lists the names in order"
        );
        assert_eq!(receipt.model, "mock:test");
    }
    let hashes: std::collections::HashSet<_> = got.iter().map(|x| &x.request_sha256).collect();
    assert_eq!(hashes.len(), 3, "each call sent different messages");
}

// ---- CR3: blocks ------------------------------------------------------------------------

#[tokio::test]
async fn test_cr3_blocks_should_follow_prompt_order_and_skip_empty_slots() {
    let store = store_with(vec![mem(
        "m-fb",
        "prefer small commits when refactoring the parser",
        MemoryKind::Feedback,
        None,
    )])
    .await;
    let skills = registry(vec![skill(
        "refactor-helper",
        "refactor",
        "Keep diffs small.",
    )]);
    let mut r = rig(
        vec![],
        Opts {
            memory: Some(store),
            skills: Some(skills),
            ..Default::default()
        },
    );

    let got = receipts(&r.turn("refactor the parser").await);

    assert_eq!(
        slots(&got[0]),
        [
            Slot::SystemPrompt,
            Slot::Recall,
            Slot::Skills,
            Slot::History
        ]
    );
    let req = r.provider.request(0);
    let sys = block(&got[0], Slot::SystemPrompt);
    assert!(sys.sources.is_empty(), "the system prompt has no sources");
    assert_eq!(sys.tokens, estimate_text_tokens(SYSTEM_PROMPT));
    // The recall and skills blocks are the system messages at positions 1 and 2.
    assert_eq!(
        block(&got[0], Slot::Recall).tokens,
        estimate_text_tokens(&req.messages[1].content.as_text())
    );
    assert_eq!(
        block(&got[0], Slot::Skills).tokens,
        estimate_text_tokens(&req.messages[2].content.as_text())
    );
    assert!(block(&got[0], Slot::History).tokens > 0);
    for b in &got[0].blocks {
        assert_eq!(
            b.strategy.slot, b.slot,
            "a block's strategy belongs to its slot"
        );
    }
}

#[tokio::test]
async fn test_cr3_a_bare_session_should_have_only_system_prompt_and_history_blocks() {
    let mut r = rig(vec![], Opts::default());

    let got = receipts(&r.turn("hello").await);

    assert_eq!(slots(&got[0]), [Slot::SystemPrompt, Slot::History]);
}

// ---- CR4: recall --------------------------------------------------------------------------

#[tokio::test]
async fn test_cr4_recall_sources_should_be_the_recalled_memory_ids_in_prompt_order() {
    let store = store_with(vec![
        mem(
            "m-a",
            "NOTE-A rebasing the zorblax branch before merging",
            MemoryKind::Feedback,
            None,
        ),
        mem(
            "m-b",
            "NOTE-B zorblax branch rebasing is preferred",
            MemoryKind::Project,
            None,
        ),
        mem(
            "m-out",
            "OUTCOME-LOG rebasing the zorblax branch before merging, result: done",
            MemoryKind::Outcome,
            None,
        ),
        mem("m-x", "lunch menu", MemoryKind::User, None),
    ])
    .await;
    let mut r = rig(
        vec![vec![read("r1")]],
        Opts {
            memory: Some(store),
            ..Default::default()
        },
    );

    let got = receipts(&r.turn("rebasing the zorblax branch").await);

    let recall = block(&got[0], Slot::Recall);
    let ids = memory_ids(recall);
    assert!(
        ids.contains(&"m-a".to_owned()) && ids.contains(&"m-b".to_owned()),
        "{ids:?}"
    );
    assert!(
        !ids.contains(&"m-out".to_owned()),
        "outcome logs are not recalled: {ids:?}"
    );
    assert!(!ids.contains(&"m-x".to_owned()));
    // Same order as the memories appear in the system message that was sent.
    let text = r.provider.request(0).messages[1].content.as_text();
    let mut by_position = ids.clone();
    by_position.sort_by_key(|id| text.find(if id == "m-a" { "NOTE-A" } else { "NOTE-B" }));
    assert_eq!(ids, by_position, "prompt order");
    assert_eq!(recall.strategy.name, "bm25");
    assert_eq!(
        recall.strategy.params_sha256,
        sha256_hex(br#"{"exclude":["outcome"],"limit":5}"#)
    );
    // The block is part of every request of the turn.
    assert_eq!(memory_ids(block(&got[1], Slot::Recall)), ids);
}

struct FixedEmbedder;

#[async_trait]
impl Embedder for FixedEmbedder {
    fn dim(&self) -> usize {
        2
    }
    async fn embed(&self, _text: &str) -> Result<Embedding, EmbedError> {
        Ok(vec![1.0, 0.0])
    }
}

#[tokio::test]
async fn test_cr4_semantic_recall_should_name_the_fusion_and_record_cut_candidates() {
    let mut memories = Vec::new();
    for i in 0..9 {
        memories.push(mem(
            &format!("f{i}"),
            &format!("FEEDBACK-NOTE number {i} observed"),
            MemoryKind::Feedback,
            Some(vec![1.0 - i as f32 * 0.05, 0.1 * i as f32]),
        ));
    }
    let store = store_with(memories).await;
    let mut r = rig(
        vec![],
        Opts {
            memory: Some(store),
            embedder: Some(Arc::new(FixedEmbedder)),
            ..Default::default()
        },
    );

    let got = receipts(&r.turn("quokka wrangling").await);

    let recall = block(&got[0], Slot::Recall);
    assert_eq!(recall.strategy.name, "bm25+semantic:rrf");
    let ids = memory_ids(recall);
    assert_eq!(ids.len(), 5, "five recalled: {ids:?}");
    for d in &got[0].dropped {
        match d {
            SourceRef::Memory { id, .. } => {
                assert!(!ids.contains(id), "{id} is both recalled and dropped")
            }
            other => panic!("dropped candidates are memories here: {other:?}"),
        }
    }
}

// ---- CR5: injection --------------------------------------------------------------------------

#[tokio::test]
async fn test_cr5_injection_block_should_list_the_hit_files_on_every_call_of_turn_one_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/auth.rs"), "pub fn login() {}\n").unwrap();
    let idx = SqliteIndex::in_memory().await.unwrap();
    idx.index_file(&PathBuf::from("src/auth.rs"), "pub fn login() {}\n")
        .await
        .unwrap();
    let mut r = rig(
        vec![vec![read("r1")]],
        Opts {
            inject: true,
            strategy: SkeletonStrategy::Flat,
            index: Some(Arc::new(idx)),
            working_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
    );

    let first = receipts(&r.turn("fix the login").await);
    let second = receipts(&r.turn("and again").await);

    assert_eq!(first.len(), 2);
    for receipt in &first {
        assert_eq!(
            slots(receipt),
            [Slot::SystemPrompt, Slot::Injection, Slot::History],
            "prompt order"
        );
        let inj = block(receipt, Slot::Injection);
        assert_eq!(inj.strategy.name, "skeleton:flat");
        let file = inj
            .sources
            .iter()
            .find_map(|s| match &s.source {
                SourceRef::File { path, .. } => Some((path.clone(), s.score)),
                _ => None,
            })
            .expect("a File source for the tier-2 hit");
        assert!(file.0.ends_with("auth.rs"), "{:?}", file.0);
        assert!(file.1.is_some_and(|s| s > 0.0), "the hit's score");
    }
    assert!(
        !slots(&second[0]).contains(&Slot::Injection),
        "later turns have no injection block"
    );
}

// ---- CR6: skills -----------------------------------------------------------------------------

#[tokio::test]
async fn test_cr6_skills_block_should_list_the_matched_skill_names() {
    let skills = registry(vec![
        skill("refactor-helper", "refactor", "Keep diffs small."),
        skill("deploy-helper", "deploy", "Run the smoke test."),
    ]);
    let mut r = rig(
        vec![],
        Opts {
            skills: Some(skills),
            ..Default::default()
        },
    );

    let got = receipts(&r.turn("please refactor this").await);

    let b = block(&got[0], Slot::Skills);
    assert_eq!(b.sources.len(), 1);
    assert_eq!(
        b.sources[0].source,
        SourceRef::Skill {
            name: "refactor-helper".to_owned()
        }
    );
    assert_eq!(b.sources[0].score, None);
}

// ---- CR7: history ----------------------------------------------------------------------------

#[tokio::test]
async fn test_cr7_history_should_list_every_non_system_message_with_its_hash_and_position() {
    let store = store_with(vec![mem("m-a", "zorblax note", MemoryKind::Feedback, None)]).await;
    let skills = registry(vec![skill("zorblax-helper", "zorblax", "Be careful.")]);
    let mut r = rig(
        vec![vec![read("r1")], vec![read("r2")]],
        Opts {
            memory: Some(store),
            skills: Some(skills),
            ..Default::default()
        },
    );

    let got = receipts(&r.turn("zorblax please").await);

    assert_eq!(got.len(), 3);
    for (i, receipt) in got.iter().enumerate() {
        let req = r.provider.request(i);
        let expected: Vec<(usize, String)> = req
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m): &(usize, &Message)| m.role != Role::System)
            .map(|(idx, m)| (idx, message_sha256(m)))
            .collect();
        let history = block(receipt, Slot::History);
        let actual: Vec<(usize, String)> = history
            .sources
            .iter()
            .map(|s| match &s.source {
                SourceRef::Message { index, sha256 } => (*index, sha256.clone()),
                other => panic!("history lists messages: {other:?}"),
            })
            .collect();
        assert_eq!(actual, expected, "call {}", i + 1);
    }
}

// ---- CR10: no text in receipts ------------------------------------------------------------------

#[tokio::test]
async fn test_cr10_receipts_should_not_contain_any_message_memory_or_skill_text() {
    const MARKER: &str = "SECRETMARKER9f3a";
    let store = store_with(vec![mem(
        "m-a",
        &format!("{MARKER} memory text about the zorblax branch"),
        MemoryKind::Feedback,
        None,
    )])
    .await;
    let mut s = skill("zorblax-helper", "zorblax", &format!("{MARKER} skill body"));
    s.description = format!("{MARKER} skill description");
    let skills = registry(vec![s]);
    let mut r = rig(
        vec![vec![call(
            "c1",
            "read_file",
            serde_json::json!({ "tag": MARKER }),
        )]],
        Opts {
            memory: Some(store),
            skills: Some(skills),
            ..Default::default()
        },
    );

    let events = r.turn(&format!("{MARKER} about the zorblax branch")).await;

    let got = receipts(&events);
    assert!(!got.is_empty());
    let json = serde_json::to_string(&got).unwrap();
    assert!(
        !json.contains(MARKER),
        "receipts must carry ids and hashes, never text: {json}"
    );
    // Sanity: the marker really was in the request.
    let sent = serde_json::to_string(&r.provider.request(0).messages).unwrap();
    assert!(sent.contains(MARKER));
}
