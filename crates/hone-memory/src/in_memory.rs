use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::bm25::{Bm25Index, Bm25Params};
use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Memory, MemoryFilter, MemoryQuery};

pub struct InMemoryStore {
    memories: Arc<RwLock<HashMap<String, Memory>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            memories: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn record(&self, memory: Memory) -> Result<(), MemoryError> {
        self.memories
            .write()
            .await
            .insert(memory.id.clone(), memory);
        Ok(())
    }

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        // Collect memories into a stable-ordered Vec so that BM25 doc indices
        // correspond to positions in this slice.  Sort by id to make doc-index
        // assignment deterministic across runs (HashMap iteration order is not).
        let guard = self.memories.read().await;
        let mut all: Vec<&Memory> = guard.values().collect();
        all.sort_by(|a, b| a.id.cmp(&b.id));

        // Apply kind filter BEFORE limit (and before BM25 scoring).
        if let Some(kind) = query.kind {
            all.retain(|m| m.kind == kind);
        }

        // Empty-query short-circuit: return all memories unfiltered (by kind already applied).
        if query.text.is_empty() {
            let mut scored: Vec<(f64, &Memory)> = all.iter().map(|m| (m.weight, *m)).collect();
            scored.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let results = scored
                .into_iter()
                .take(query.limit)
                .map(|(_, m)| m.clone())
                .collect();
            return Ok(results);
        }

        // Build a BM25 index over the searchable text of every (kind-filtered) memory.
        // Searchable text = description (if present) + "\n" + content.
        let texts: Vec<String> = all
            .iter()
            .map(|m| {
                let desc = m.description.as_deref().unwrap_or_default();
                format!("{desc}\n{}", m.content)
            })
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let index = Bm25Index::build(&refs, Bm25Params::default());

        let mut scored: Vec<(f64, &Memory)> = all
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let bm25 = index.score(i, &query.text);
                if bm25 > 0.0 {
                    Some((bm25 * m.weight, *m))
                } else {
                    None
                }
            })
            .collect();

        // Descending by combined score; stable sort for determinism on ties.
        scored.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        let results = scored
            .into_iter()
            .take(query.limit)
            .map(|(_, m)| m.clone())
            .collect();

        Ok(results)
    }

    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        Ok(self.memories.read().await.get(id).cloned())
    }

    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        let mut guard = self.memories.write().await;
        let before = guard.len();
        match filter {
            MemoryFilter::Tag(tag) => {
                guard.retain(|_, m| !m.tags.contains(tag));
            }
            MemoryFilter::Before(ts) => {
                guard.retain(|_, m| m.created_at >= *ts);
            }
            MemoryFilter::After(ts) => {
                guard.retain(|_, m| m.created_at <= *ts);
            }
            MemoryFilter::Kind(kind) => {
                guard.retain(|_, m| m.kind != *kind);
            }
        }
        let removed = before.saturating_sub(guard.len()) as u32;
        Ok(removed)
    }

    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        let mut guard = self.memories.write().await;
        let before = guard.len();

        // Apply weight decay and remove expired memories in one pass.
        guard.retain(|_, m| {
            m.weight *= factor;
            !matches!(m.decay_at, Some(t) if t <= now)
        });

        let removed = before.saturating_sub(guard.len()) as u32;
        Ok(removed)
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        let guard = self.memories.read().await;
        let memories: Vec<&Memory> = guard.values().collect();
        let json = serde_json::to_string_pretty(&memories)?;
        drop(guard);
        tokio::fs::write(path, json).await?;
        Ok(())
    }

    async fn import(path: &Path) -> Result<Self, MemoryError> {
        let bytes = tokio::fs::read(path).await?;
        let memories: Vec<Memory> = serde_json::from_slice(&bytes)?;
        let map = memories.into_iter().map(|m| (m.id.clone(), m)).collect();
        Ok(Self {
            memories: Arc::new(RwLock::new(map)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemoryKind;

    fn make_memory_with_embedding(id: &str, content: &str, emb: Vec<f32>) -> Memory {
        Memory {
            id: id.to_owned(),
            content: content.to_owned(),
            tags: vec![],
            weight: 1.0,
            created_at: 1_000_000,
            decay_at: None,
            kind: MemoryKind::Conversation,
            description: None,
            embedding: Some(emb),
        }
    }

    fn make_memory_no_embedding(id: &str, content: &str) -> Memory {
        Memory {
            id: id.to_owned(),
            content: content.to_owned(),
            tags: vec![],
            weight: 1.0,
            created_at: 1_000_000,
            decay_at: None,
            kind: MemoryKind::Conversation,
            description: None,
            embedding: None,
        }
    }

    /// recall_semantic must return the top-K memories by cosine similarity to
    /// the query embedding, using only memories that have a stored embedding.
    #[tokio::test]
    async fn in_memory_recall_semantic_returns_top_k_by_cosine() {
        let store = InMemoryStore::new();

        // Three orthogonal 3-dim unit vectors: rust=[1,0,0], python=[0,1,0], js=[0,0,1]
        store
            .record(make_memory_with_embedding("rust", "rust", vec![1.0, 0.0, 0.0]))
            .await
            .unwrap();
        store
            .record(make_memory_with_embedding(
                "python",
                "python",
                vec![0.0, 1.0, 0.0],
            ))
            .await
            .unwrap();
        store
            .record(make_memory_with_embedding(
                "javascript",
                "javascript",
                vec![0.0, 0.0, 1.0],
            ))
            .await
            .unwrap();

        // Query embedding aligned with "rust"
        let query = vec![1.0_f32, 0.0, 0.0];
        let results = store.recall_semantic(&query, 1).await.unwrap();

        assert_eq!(results.len(), 1, "expected exactly 1 result");
        assert_eq!(
            results[0].memory.id, "rust",
            "top result should be the rust memory"
        );
        assert!(
            (results[0].score - 1.0).abs() < 1e-5,
            "score for identical direction should be 1.0, got {}",
            results[0].score
        );
    }

    /// Memories without an embedding field must be silently skipped.
    #[tokio::test]
    async fn in_memory_recall_semantic_skips_memories_without_embedding() {
        let store = InMemoryStore::new();

        store
            .record(make_memory_with_embedding("has-emb", "has embedding", vec![1.0, 0.0]))
            .await
            .unwrap();
        store
            .record(make_memory_no_embedding("no-emb", "no embedding"))
            .await
            .unwrap();

        let query = vec![1.0_f32, 0.0];
        let results = store.recall_semantic(&query, 10).await.unwrap();

        assert_eq!(
            results.len(),
            1,
            "only the memory with an embedding should be returned"
        );
        assert_eq!(results[0].memory.id, "has-emb");
    }
}
