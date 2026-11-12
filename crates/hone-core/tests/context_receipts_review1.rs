#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR18 and CR19 of `docs/plans/context-receipts-spec.md` (review round 1).
//!
//! CR18: the recall strategy is `bm25+semantic:rrf` whenever an embedder is configured
//! and ran for the query, even when it returned nothing. Its params come from the
//! agent's constants: `{"exclude":["outcome"],"limit":5,"rrf_k":60,"semantic_fetch":20}`
//! (agent.rs: limit 5, `SEMANTIC_RECALL_FETCH` = 20, RRF k = 60), hashed as canonical
//! JSON with sorted keys.
//!
//! CR19: the `Injection` block's `tokens` is the estimate of the REDACTED block that
//! was put into the prompt (redaction is on when the agent has a policy with
//! `builtins: true`).

mod receipts_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use hone_config::policy::{CompiledPolicy, PolicyContext, builtin_rules};
use hone_core::{PolicyConfig, SkeletonStrategy, estimate_text_tokens};
use hone_index::SqliteIndex;
use hone_memory::{
    EmbedError, Embedder, Embedding, InMemoryStore, Memory, MemoryKind, MemoryStore,
};
use hone_proto::{AgentEvent, ContextReceipt, Role, Slot, sha256_hex};
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

fn block(r: &ContextReceipt, slot: Slot) -> &hone_proto::BlockReceipt {
    r.blocks
        .iter()
        .find(|b| b.slot == slot)
        .unwrap_or_else(|| panic!("no {slot:?} block"))
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

// ---- CR18 ---------------------------------------------------------------------------------

#[tokio::test]
async fn test_cr18_an_embedder_that_found_nothing_should_still_name_the_fusion_strategy() {
    // The memory has no embedding, so the semantic search returns nothing; BM25
    // finds it. The embedder was configured and ran.
    let store = Arc::new(InMemoryStore::new());
    store
        .record(Memory {
            id: "m-a".to_owned(),
            content: "NOTE-A rebasing the zorblax branch before merging".to_owned(),
            tags: vec![],
            weight: 1.0,
            created_at: 1_000_000,
            decay_at: None,
            kind: MemoryKind::Feedback,
            description: None,
            embedding: None,
        })
        .await
        .unwrap();
    let mut r = rig(
        vec![],
        Opts {
            memory: Some(store),
            embedder: Some(Arc::new(FixedEmbedder)),
            ..Default::default()
        },
    );

    let got = receipts(&r.turn("rebasing the zorblax branch").await);

    let recall = block(&got[0], Slot::Recall);
    assert_eq!(recall.sources.len(), 1, "BM25 recalled the memory");
    assert_eq!(recall.strategy.name, "bm25+semantic:rrf");
    assert_eq!(
        recall.strategy.params_sha256,
        sha256_hex(br#"{"exclude":["outcome"],"limit":5,"rrf_k":60,"semantic_fetch":20}"#),
        "params carry the agent's limit, RRF k and semantic fetch size"
    );
}

// ---- CR19 ---------------------------------------------------------------------------------

#[tokio::test]
async fn test_cr19_injection_tokens_should_count_the_redacted_block_that_was_sent() {
    let tmp = tempfile::tempdir().unwrap();
    let dir: PathBuf = std::fs::canonicalize(tmp.path()).unwrap();
    // A secret-shaped identifier (a GitHub token shape) in a signature the skeleton shows.
    let secret = "ghp_0123456789abcdefghijklmnopqrstuvwxyz";
    let code = format!("pub fn {secret}() {{}}\npub fn logout() {{}}\n");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/auth.rs"), &code).unwrap();
    let idx = SqliteIndex::in_memory().await.unwrap();
    idx.index_file(Path::new("src/auth.rs"), &code).await.unwrap();

    let context = PolicyContext::for_platform(dir.clone(), None);
    let compiled = CompiledPolicy::compile(&context, &builtin_rules(), &[], &[]).unwrap();
    let policy = PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: Vec::new(),
        builtins: true,
    };
    let mut r = rig(
        vec![],
        Opts {
            inject: true,
            strategy: SkeletonStrategy::Flat,
            index: Some(Arc::new(idx)),
            working_dir: Some(dir),
            policy: Some(policy),
            ..Default::default()
        },
    );

    let got = receipts(&r.turn("fix the login").await);

    let sent = r
        .provider
        .request(0)
        .messages
        .iter()
        .find(|m| m.role == Role::User)
        .unwrap()
        .content
        .as_text()
        .to_string();
    assert!(
        !sent.contains(secret),
        "harness: the injected block must have been redacted: {sent}"
    );
    let (block_text, _) = sent
        .split_once("\n\n---\n\nfix the login")
        .unwrap_or_else(|| panic!("the injected block precedes the user text: {sent}"));
    assert!(!block_text.is_empty());
    assert_eq!(
        block(&got[0], Slot::Injection).tokens,
        estimate_text_tokens(block_text),
        "tokens of the text actually sent"
    );
}
