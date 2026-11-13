#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G19 of `docs/plans/fts5-recall-spec.md` (review round 2): folding leaves non-Latin scripts
//! intact, in `recall_fts5` AND in the default BM25 `recall`.
//!
//! Contract pinned:
//! - diacritics are removed only from Latin base letters; the text is recomposed to NFC, so
//!   Hangul syllables are not decomposed to jamo, kana voicing marks (`パ` vs `バ` vs `ハ`) stay,
//!   Devanagari vowel signs and virama stay, and Thai vowel/tone marks stay;
//! - the FTS5 table is declared `tokenize = 'unicode61 remove_diacritics 2'`, and the Rust
//!   side folds Vietnamese letters (two diacritics) the way that setting does (`ệ` -> `e`,
//!   `ở` -> `o`);
//! - Latin folding keeps working (`café` = `cafe`, `kąt` = `kat`, `ł` stays `ł`).
//! No new API.

mod fts5_support;

use fts5_support::{mem, q};
use hone_memory::{MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

async fn store_of(memories: &[(&str, &str)]) -> SqliteStore {
    let store = SqliteStore::in_memory().await.unwrap();
    for (i, (id, content)) in memories.iter().enumerate() {
        store.record(mem(id, content, MemoryKind::Feedback, i as u64 + 1)).await.unwrap();
    }
    store
}

async fn fts_ids(store: &SqliteStore, prompt: &str) -> Vec<String> {
    let mut v: Vec<String> = store.recall_fts5(&q(prompt, 10, None), &[]).await.unwrap().into_iter().map(|s| s.memory.id).collect();
    v.sort();
    v
}

async fn bm25_ids(store: &SqliteStore, prompt: &str) -> Vec<String> {
    let mut v: Vec<String> = store.recall(&q(prompt, 10, None)).await.unwrap().into_iter().map(|m| m.id).collect();
    v.sort();
    v
}

/// Each (id, memory, query): the query finds exactly its own memory in both recallers.
const SCRIPTS: [(&str, &str, &str); 6] = [
    ("ko", "한국어 문서 번역 가이드와 용어집", "한국어"),
    ("ja", "データベース の バックアップ 手順", "データベース"),
    ("hi", "हिन्दी भाषा सीखने की शुरुआत", "हिन्दी"),
    ("vi-viet", "Tiếng Việt cơ bản cho người mới", "Việt"),
    ("vi-pho", "Quán phở ngon nhất thành phố", "phở"),
    ("th", "เรียน สวัสดี ภาษาไทย ทุกวัน", "สวัสดี"),
];

#[tokio::test]
async fn test_g19_non_latin_queries_should_find_their_own_memories_in_both_recallers() {
    let mut all: Vec<(&str, &str)> = SCRIPTS.iter().map(|(id, text, _)| (*id, *text)).collect();
    all.push(("noise", "lunch menu pasta and salad in the office kitchen"));
    let store = store_of(&all).await;

    for (id, _, query) in SCRIPTS {
        assert_eq!(fts_ids(&store, query).await, [id], "fts5: {query:?}");
        assert_eq!(bm25_ids(&store, query).await, [id], "bm25: {query:?}");
    }
}

#[tokio::test]
async fn test_g19_kana_voicing_marks_should_distinguish_words() {
    let store = store_of(&[
        ("pasu", "パス ワード の 管理"),
        ("basu", "バス 停 の 場所"),
        ("hasu", "ハス の 花 が 咲く"),
    ])
    .await;

    for (query, want) in [("パス", "pasu"), ("バス", "basu"), ("ハス", "hasu")] {
        assert_eq!(fts_ids(&store, query).await, [want], "fts5: {query}");
        assert_eq!(bm25_ids(&store, query).await, [want], "bm25: {query}");
    }
}

#[tokio::test]
async fn test_g19_devanagari_vowel_signs_and_virama_should_distinguish_words() {
    let store = store_of(&[
        ("hindi", "हिन्दी भाषा"),
        ("other", "हन्दी बात"), // same consonants, different vowel sign
        ("third", "हिंदी भाषा"),  // anusvara spelling
    ])
    .await;

    assert_eq!(fts_ids(&store, "हिन्दी").await, ["hindi"], "fts5");
    assert_eq!(bm25_ids(&store, "हिन्दी").await, ["hindi"], "bm25");
    assert_eq!(bm25_ids(&store, "हन्दी").await, ["other"], "bm25, other word");
}

#[tokio::test]
async fn test_g19_hangul_syllables_should_not_be_decomposed_into_jamo() {
    let store = store_of(&[("ko", "서울 지하철 노선도"), ("other", "부산 항구 도시")]).await;

    for query in ["서울", "지하철"] {
        assert_eq!(fts_ids(&store, query).await, ["ko"], "fts5: {query}");
        assert_eq!(bm25_ids(&store, query).await, ["ko"], "bm25: {query}");
    }
}

#[tokio::test]
async fn test_g19_vietnamese_double_diacritics_should_fold_like_the_index_does() {
    // Written with and without marks: the same words.
    let store = store_of(&[("marked", "Tiếng Việt cơ bản"), ("plain", "Tieng Viet co ban"), ("noise", "lunch menu")]).await;

    for query in ["Việt", "Viet", "tiếng"] {
        assert_eq!(fts_ids(&store, query).await, ["marked", "plain"], "fts5: {query}");
        assert_eq!(bm25_ids(&store, query).await, ["marked", "plain"], "bm25: {query}");
    }
}

#[tokio::test]
async fn test_g19_the_fts_table_should_use_remove_diacritics_2_and_latin_folding_should_keep_working() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    drop(SqliteStore::open_with_decay_interval(&path, 3_153_600_000).await.unwrap());
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display())).await.unwrap();

    let sql: String = sqlx::query("SELECT sql FROM sqlite_master WHERE name = 'memories_fts'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);

    assert!(sql.contains("remove_diacritics 2"), "the FTS5 table declaration: {sql}");

    // Latin folding still holds in both recallers.
    let store = store_of(&[
        ("cafe-acc", "meet at the café"),
        ("cafe-plain", "meet at the cafe"),
        ("kat-acc", "kąt prosty"),
        ("kat-plain", "kat prosty"),
        ("lodz", "łódź harbour"),
        ("plain-l", "lodz harbour plain"),
    ])
    .await;

    for (query, want) in [
        ("café", vec!["cafe-acc", "cafe-plain"]),
        ("cafe", vec!["cafe-acc", "cafe-plain"]),
        ("kąt", vec!["kat-acc", "kat-plain"]),
        ("łodz", vec!["lodz"]),
        ("lodz", vec!["plain-l"]),
    ] {
        assert_eq!(fts_ids(&store, query).await, want, "fts5: {query}");
        assert_eq!(bm25_ids(&store, query).await, want, "bm25: {query}");
    }
}
