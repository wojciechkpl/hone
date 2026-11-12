#![allow(clippy::unwrap_used, clippy::expect_used)]
//! M6 of `docs/plans/memory-recall-index-spec.md`, at the context injector:
//! tier 2 uses the gap between the top two `SearchResult.score`s as its
//! confidence, so a better match being added to the index must never switch
//! injection off. No new API.

use std::path::PathBuf;

use hone_core::context_injector::{ContextInjector, RetrievalConfidence};
use hone_index::SqliteIndex;

const PROMPT: &str = "fix the login bug";

async fn confidence_with(fns: &[&str]) -> (RetrievalConfidence, String) {
    let dir = tempfile::tempdir().unwrap();
    let src: String = fns.iter().map(|n| format!("fn {n}() {{}}\n")).collect();
    std::fs::write(dir.path().join("lib.rs"), &src).unwrap();
    let idx = SqliteIndex::in_memory().await.unwrap();
    idx.index_file(&PathBuf::from("lib.rs"), &src).await.unwrap();

    let mut injector = ContextInjector::with_defaults();
    let result = injector.inject(PROMPT, dir.path(), Some(&idx)).await;
    (result.confidence, result.context_block)
}

#[tokio::test]
async fn test_m6_login_only_index_should_inject() {
    let (confidence, block) = confidence_with(&["login"]).await;
    assert_ne!(confidence, RetrievalConfidence::Low);
    assert!(block.contains("login"), "{block}");
}

#[tokio::test]
async fn test_m6_adding_a_better_match_should_not_switch_injection_off() {
    let (confidence, block) =
        confidence_with(&["login", "login_bug_fix", "handle_login"]).await;
    assert_ne!(
        confidence,
        RetrievalConfidence::Low,
        "the better match login_bug_fix must not turn tier 2 off"
    );
    assert!(block.contains("login_bug_fix"), "the best match is injected: {block}");
}
