use std::collections::HashSet;
use std::path::Path;

use async_trait::async_trait;

use crate::embedding::{ScoredMemory, cosine_similarity};
use crate::error::MemoryError;
use crate::types::{Memory, MemoryFilter, MemoryQuery};

// ---------------------------------------------------------------------------
// Trigram similarity helpers
// ---------------------------------------------------------------------------

/// Collect all character-level trigrams from `text` into a `HashSet`.
///
/// Lower-cases the input, strips non-alphanumeric characters, then slides a
/// window of width 3 over the remaining characters.  Short strings (< 3 chars
/// after normalisation) produce no trigrams; the Jaccard similarity against
/// them will be 0.0.
///
/// Stripping punctuation ensures that `"use rg, not grep"` and
/// `"use rg not grep"` produce largely overlapping trigram sets, giving a
/// high similarity score even when minor punctuation differs.
fn trigrams(text: &str) -> HashSet<[char; 3]> {
    let chars: Vec<char> = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    let mut set = HashSet::new();
    for window in chars.windows(3) {
        // windows() always yields slices of exactly 3 elements when width == 3.
        set.insert([window[0], window[1], window[2]]);
    }
    set
}

/// Jaccard similarity between the trigram sets of `a` and `b`.
///
/// Returns a value in `[0.0, 1.0]`.  Returns `0.0` when both sets are empty.
fn trigram_jaccard(a: &str, b: &str) -> f64 {
    let set_a = trigrams(a);
    let set_b = trigrams(b);
    if set_a.is_empty() && set_b.is_empty() {
        return 0.0;
    }
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

// ---------------------------------------------------------------------------
// Trait definition
// ---------------------------------------------------------------------------

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn record(&self, memory: Memory) -> Result<(), MemoryError>;

    /// Retrieve memories matching `query`.
    ///
    /// **Contract:** when `query.text` is empty, implementations MUST return
    /// all memories (up to `query.limit`) rather than returning nothing. The
    /// default `find_similar` relies on this to gather candidates for similarity
    /// scoring.
    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError>;

    /// O(log n) (or better) exact lookup by `id`. Returns `None` when no
    /// memory has that id. Implementations MUST NOT do a full table scan.
    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError>;

    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError>;

    /// Multiply all weights by `factor`. Remove memories whose `decay_at` is
    /// `Some(t)` with `t <= now`. Returns the count of memories removed.
    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError>;

    async fn export(&self, path: &Path) -> Result<(), MemoryError>;

    async fn import(path: &Path) -> Result<Self, MemoryError>
    where
        Self: Sized;

    /// Semantic recall: return up to `limit` memories ranked by cosine
    /// similarity to `query_embedding`. Memories without a stored embedding
    /// are silently skipped.
    ///
    /// The default implementation fetches all memories via
    /// `recall(MemoryQuery { text: "", limit: 1024, kind: None })`, then for
    /// each memory whose `embedding` is `Some`, computes cosine similarity and
    /// returns the top `limit` as `ScoredMemory`. Implementations with native
    /// vector indexes should override for efficiency.
    async fn recall_semantic(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<ScoredMemory>, MemoryError> {
        let candidates = self
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 1024,
                kind: None,
            })
            .await?;

        let mut scored: Vec<ScoredMemory> = candidates
            .into_iter()
            .filter_map(|m| {
                let embedding = m.embedding.clone()?;
                let score = cosine_similarity(query_embedding, &embedding);
                Some(ScoredMemory { memory: m, score })
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        scored.truncate(limit);
        Ok(scored)
    }

    /// Find the memory whose content is most similar to `query` and whose
    /// trigram-Jaccard similarity score is at or above `threshold`.
    ///
    /// Returns the highest-scoring match, or `None` if no memory exceeds the
    /// threshold (or the store is empty).
    ///
    /// The default implementation fetches up to 32 recent memories and scores
    /// them in-process using character-level trigram Jaccard similarity.  This
    /// is O(N) in the number of stored memories and is acceptable for stores
    /// with fewer than a few thousand records.  The `SqliteStore` may override
    /// this for better performance when the store grows large.
    async fn find_similar(
        &self,
        query: &str,
        threshold: f64,
    ) -> Result<Option<Memory>, MemoryError> {
        let candidates = self
            .recall(&MemoryQuery {
                text: String::new(), // fetch all (up to limit)
                limit: 32,
                kind: None,
            })
            .await?;

        let best = candidates
            .into_iter()
            .map(|m| {
                let score = trigram_jaccard(query, &m.content);
                (score, m)
            })
            .filter(|(score, _)| *score >= threshold)
            .max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        Ok(best.map(|(_, m)| m))
    }
}
