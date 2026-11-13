#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G13 of `docs/plans/fts5-recall-spec.md` (review round 1): the in-Rust BM25 (`recall`) and
//! `recall_fts5` agree on what a word is.
//!
//! Contract pinned:
//! - both split on non-(Unicode letter or digit), lowercase, and fold diacritics the same way
//!   (NFD, combining marks removed): `kąt`/`kat` and `café`/`cafe` are the same word in both;
//!   `ł` is not decomposable and stays `ł` in both (as in FTS5's unicode61), so `łódź` is
//!   `łodz` and does not match `lodz`;
//! - the FTS5 query keeps tokens of 2 or more characters that are not stop words, removes
//!   duplicates, and when more than 32 remain keeps the 32 RAREST (lowest document
//!   frequency in the index; ties in order of appearance), not the first 32.

mod fts5_support;

use fts5_support::{mem, q};
use hone_memory::{MemoryKind, MemoryStore, SqliteStore};

async fn store_of(memories: &[(&str, &str)]) -> SqliteStore {
    let store = SqliteStore::in_memory().await.unwrap();
    for (i, (id, content)) in memories.iter().enumerate() {
        store.record(mem(id, content, MemoryKind::Feedback, i as u64 + 1)).await.unwrap();
    }
    store
}

async fn both(store: &SqliteStore, prompt: &str) -> (Vec<String>, Vec<String>) {
    let mut bm25: Vec<String> = store.recall(&q(prompt, 20, None)).await.unwrap().into_iter().map(|m| m.id).collect();
    let mut fts: Vec<String> = store.recall_fts5(&q(prompt, 20, None), &[]).await.unwrap().into_iter().map(|s| s.memory.id).collect();
    bm25.sort();
    fts.sort();
    (bm25, fts)
}

#[tokio::test]
async fn test_g13_both_recalls_should_fold_diacritics_identically() {
    let store = store_of(&[
        ("kat-acc", "kąt prosty w trójkącie"),
        ("kat-plain", "kat prosty w trojkacie"),
        ("cafe-acc", "meet at the café"),
        ("cafe-plain", "meet at the cafe"),
        ("noise", "lunch menu pasta"),
    ])
    .await;

    for (prompt, want) in [
        ("kąt", vec!["kat-acc", "kat-plain"]),
        ("kat", vec!["kat-acc", "kat-plain"]),
        ("café", vec!["cafe-acc", "cafe-plain"]),
        ("cafe", vec!["cafe-acc", "cafe-plain"]),
        ("trójkącie", vec!["kat-acc", "kat-plain"]),
    ] {
        let (bm25, fts) = both(&store, prompt).await;
        assert_eq!(bm25, want, "bm25 for {prompt:?}");
        assert_eq!(fts, want, "fts5 for {prompt:?}");
    }
}

#[tokio::test]
async fn test_g13_l_with_stroke_should_stay_l_in_both() {
    let store = store_of(&[("lodz", "łódź harbour report"), ("plain", "lodz harbour report plain")]).await;

    let (bm25, fts) = both(&store, "łodz").await;
    assert_eq!((bm25, fts), (vec!["lodz".to_owned()], vec!["lodz".to_owned()]), "ł matches ł (ó and ź fold)");
    let (bm25, fts) = both(&store, "lodz").await;
    assert_eq!((bm25, fts), (vec!["plain".to_owned()], vec!["plain".to_owned()]), "l does not match ł");
}

#[tokio::test]
async fn test_g13_two_letter_words_should_be_searchable() {
    let store = store_of(&[
        ("go", "Go CLI release notes"),
        ("ui", "UI CI PR checklist"),
        ("noise", "lunch menu pasta and salad"),
    ])
    .await;

    for (prompt, want) in [("Go", "go"), ("Go CLI", "go"), ("UI CI PR", "ui"), ("pr", "ui")] {
        let got = store.recall_fts5(&q(prompt, 5, None), &[]).await.unwrap();
        assert_eq!(got.first().map(|s| s.memory.id.as_str()), Some(want), "{prompt:?}");
        let bm25 = store.recall(&q(prompt, 5, None)).await.unwrap();
        assert_eq!(bm25.first().map(|m| m.id.as_str()), Some(want), "bm25 {prompt:?}");
    }
    // one-character words and stop words still don't count
    for prompt in ["a", "x y z", "to of in"] {
        assert!(store.recall_fts5(&q(prompt, 5, None), &[]).await.unwrap().is_empty(), "{prompt:?}");
    }
}

#[tokio::test]
async fn test_g13_duplicate_tokens_should_count_once() {
    let store = store_of(&[("hit", "zorblax note"), ("noise", "alpha beta")]).await;
    let prompt = format!("{}zorblax", "alpha ".repeat(40));

    let got = store.recall_fts5(&q(&prompt, 5, None), &[]).await.unwrap();

    let mut ids: Vec<&str> = got.iter().map(|s| s.memory.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["hit", "noise"], "40 copies of one word are one token, so the 41st word is not cut");
}

#[tokio::test]
async fn test_g13_a_long_prompt_should_keep_its_rarest_words_not_its_first_ones() {
    // 40 common words (in every document) then one rare word (in one document).
    let common: Vec<String> = (1..=40).map(|i| format!("common{i:02}")).collect();
    let mut docs: Vec<(String, String)> = (0..60)
        .map(|i| (format!("d{i:02}"), format!("{} filler {i}", common.join(" "))))
        .collect();
    // Its only common word is the first one: when the rare word is cut it barely matches.
    docs.push(("rare".to_owned(), "common01 zyzzyva".to_owned()));
    let store = SqliteStore::in_memory().await.unwrap();
    for (i, (id, text)) in docs.iter().enumerate() {
        store.record(mem(id, text, MemoryKind::Feedback, i as u64 + 1)).await.unwrap();
    }
    let prompt = format!("{} zyzzyva", common.join(" "));

    let fts = store.recall_fts5(&q(&prompt, 5, None), &[]).await.unwrap();
    let bm25 = store.recall(&q(&prompt, 5, None)).await.unwrap();

    assert_eq!(bm25[0].id, "rare", "fixture check: the in-Rust BM25 sees every token");
    assert_eq!(fts[0].memory.id, "rare", "the rare word survives the 32-token cap");
}

#[tokio::test]
async fn test_g13_top_results_should_match_the_rust_bm25_on_two_letter_accented_and_long_queries() {
    let mut memories: Vec<(String, String)> = vec![
        ("go".into(), "Go CLI release notes and the Go toolchain".into()),
        ("ui".into(), "UI CI PR checklist for the UI team".into()),
        ("kat".into(), "kąt widzenia i kąt prosty".into()),
        ("cafe".into(), "café menu and café opening hours".into()),
    ];
    for i in 0..12 {
        memories.push((format!("noise-{i}"), format!("lunch menu number {i}: pasta and salad in the kitchen")));
    }
    let store = SqliteStore::in_memory().await.unwrap();
    for (i, (id, text)) in memories.iter().enumerate() {
        store.record(mem(id, text, MemoryKind::Feedback, i as u64 + 1)).await.unwrap();
    }
    let long: String = (0..40).map(|i| format!("lunchword{i} ")).collect::<String>() + "kąt widzenia";

    for (prompt, want) in [
        ("Go CLI".to_owned(), "go"),
        ("UI CI PR".to_owned(), "ui"),
        ("kat widzenia".to_owned(), "kat"),
        ("cafe menu".to_owned(), "cafe"),
        (long, "kat"),
    ] {
        let bm25 = store.recall(&q(&prompt, 5, None)).await.unwrap();
        let fts = store.recall_fts5(&q(&prompt, 5, None), &[]).await.unwrap();
        assert_eq!(bm25[0].id, want, "fixture check: bm25 top for {prompt:?}");
        assert_eq!(fts[0].memory.id, bm25[0].id, "{prompt:?}: fts5 top equals bm25 top");
    }
}
