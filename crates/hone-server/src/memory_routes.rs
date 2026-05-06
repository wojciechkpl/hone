//! `GET /memory` and `GET /memory/stats` — read-only viewers over the
//! optional memory store. Both return `503 Service Unavailable` when no
//! memory store is configured on the daemon (the GUI shows a "no memory
//! configured" hint instead of bombing).
//!
//! The store implements `MemoryStore`, so any backend (sqlite, in-memory,
//! encrypted) plugs in transparently.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use hone_memory::{MemoryKind, MemoryQuery, MemoryStore};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Default page size for `GET /memory` when the client does not specify
/// `limit`. Generous but bounded so a curious client cannot pull a 100k
/// row store into a single response.
const DEFAULT_LIMIT: usize = 100;
/// Hard ceiling enforced regardless of `?limit=`.
const MAX_LIMIT: usize = 500;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<usize>,
    /// Optional substring filter applied to memory `content`. Matches BM25
    /// in the underlying store; an empty string returns all rows.
    #[serde(default)]
    pub q: Option<String>,
    /// Optional kind filter (matches the `MemoryKind` discriminant).
    #[serde(default)]
    pub kind: Option<MemoryKind>,
}

/// One memory row as the GUI sees it. Mirrors `hone_memory::Memory` minus
/// the embedding (a 1024-d float vector blows up the wire shape and is
/// rarely useful in a list view).
#[derive(Debug, Clone, Serialize)]
pub struct MemoryWire {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub weight: f64,
    pub created_at: u64,
    pub decay_at: Option<u64>,
    pub kind: MemoryKind,
    pub description: Option<String>,
    pub has_embedding: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStats {
    pub total: usize,
    /// Per-kind count, keyed by kind discriminant (snake_case).
    pub by_kind: BTreeMap<String, usize>,
    /// Top tags by frequency (capped at 20).
    pub top_tags: Vec<(String, usize)>,
}

/// `GET /memory?limit=N&q=text&kind=fact`
pub async fn list_memory_handler(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<MemoryWire>>, (StatusCode, String)> {
    let store = state
        .memory_store
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "memory store not configured".to_string()))?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let mq = MemoryQuery {
        text: query.q.unwrap_or_default(),
        limit,
        kind: query.kind,
    };
    let memories = store
        .recall(&mq)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("recall failed: {e}")))?;
    let wire: Vec<MemoryWire> = memories.into_iter().map(memory_to_wire).collect();
    Ok(Json(wire))
}

/// `GET /memory/stats`
pub async fn memory_stats_handler(
    State(state): State<AppState>,
) -> Result<Json<MemoryStats>, (StatusCode, String)> {
    let store = state
        .memory_store
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "memory store not configured".to_string()))?;
    // Pull a wide page to compute stats. For very large stores this should
    // become a dedicated count query in the trait, but the GUI's primary
    // use case is desktop projects with at most a few thousand entries.
    let mq = MemoryQuery {
        text: String::new(),
        limit: 5000,
        kind: None,
    };
    let memories = store
        .recall(&mq)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("recall failed: {e}")))?;
    Ok(Json(compute_stats(&memories)))
}

fn memory_to_wire(m: hone_memory::Memory) -> MemoryWire {
    let has_embedding = m.embedding.is_some();
    MemoryWire {
        id: m.id,
        content: m.content,
        tags: m.tags,
        weight: m.weight,
        created_at: m.created_at,
        decay_at: m.decay_at,
        kind: m.kind,
        description: m.description,
        has_embedding,
    }
}

fn compute_stats(memories: &[hone_memory::Memory]) -> MemoryStats {
    let total = memories.len();
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut tag_counts: BTreeMap<String, usize> = BTreeMap::new();
    for m in memories {
        let kind_key = serde_json::to_value(&m.kind)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{:?}", m.kind).to_lowercase());
        *by_kind.entry(kind_key).or_insert(0) += 1;
        for t in &m.tags {
            *tag_counts.entry(t.clone()).or_insert(0) += 1;
        }
    }
    let mut top_tags: Vec<(String, usize)> = tag_counts.into_iter().collect();
    top_tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top_tags.truncate(20);
    MemoryStats { total, by_kind, top_tags }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use hone_memory::{InMemoryStore, Memory};

    fn fake_memory(id: &str, kind: MemoryKind, tags: &[&str], content: &str) -> Memory {
        Memory {
            id: id.to_string(),
            content: content.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            weight: 1.0,
            created_at: 0,
            decay_at: None,
            kind,
            description: None,
            embedding: None,
        }
    }

    #[test]
    fn compute_stats_counts_total_and_groups_by_kind() {
        let memories = vec![
            fake_memory("a", MemoryKind::User, &["rust"], "x"),
            fake_memory("b", MemoryKind::User, &["rust", "perf"], "y"),
            fake_memory("c", MemoryKind::Project, &["style"], "z"),
        ];
        let s = compute_stats(&memories);
        assert_eq!(s.total, 3);
        assert_eq!(s.by_kind.get("user"), Some(&2));
        assert_eq!(s.by_kind.get("project"), Some(&1));
    }

    #[test]
    fn compute_stats_top_tags_orders_by_frequency() {
        let memories = vec![
            fake_memory("a", MemoryKind::User, &["rust", "test"], "x"),
            fake_memory("b", MemoryKind::User, &["rust"], "y"),
            fake_memory("c", MemoryKind::User, &["test"], "z"),
        ];
        let s = compute_stats(&memories);
        // rust appears 2x, test appears 2x, ties broken alphabetically.
        let counts: Vec<(String, usize)> = s.top_tags.into_iter().collect();
        assert_eq!(counts[0].1, 2);
        assert_eq!(counts[1].1, 2);
        // Alphabetical tie-break — "rust" comes before "test".
        assert_eq!(counts[0].0, "rust");
        assert_eq!(counts[1].0, "test");
    }

    #[tokio::test]
    async fn list_memory_handler_returns_503_when_store_unset() {
        let state = AppState::new(None);
        let resp = list_memory_handler(
            axum::extract::State(state),
            axum::extract::Query(ListQuery {
                limit: None,
                q: None,
                kind: None,
            }),
        )
        .await;
        match resp {
            Ok(_) => panic!("expected 503"),
            Err((code, _)) => assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE),
        }
    }

    #[tokio::test]
    async fn list_memory_handler_serves_recalled_entries_when_store_set() {
        use std::sync::Arc;
        let store = Arc::new(InMemoryStore::new());
        store
            .record(fake_memory("m1", MemoryKind::User, &["t"], "hello"))
            .await
            .expect("record");
        let state = AppState::new(None).with_memory_store(store);
        let resp = list_memory_handler(
            axum::extract::State(state),
            axum::extract::Query(ListQuery {
                limit: None,
                q: None,
                kind: None,
            }),
        )
        .await
        .expect("list ok");
        let entries = resp.0;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "m1");
        assert_eq!(entries[0].content, "hello");
    }
}
