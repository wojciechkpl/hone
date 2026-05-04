use async_trait::async_trait;

use crate::types::Memory;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A pre-computed embedding vector. Implementations must use a consistent
/// dimensionality across calls (mismatched dims trip a runtime check in
/// cosine_similarity).
pub type Embedding = Vec<f32>;

/// Errors produced by Embedder implementations.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("network error: {0}")]
    Network(String),
    #[error("rate limited")]
    RateLimited,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("provider error: {0}")]
    Provider(String),
}

/// Producer of embeddings for arbitrary text.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Return the dimensionality of vectors this embedder produces.
    fn dim(&self) -> usize;

    /// Embed a single string.
    async fn embed(&self, text: &str) -> Result<Embedding, EmbedError>;

    /// Embed a batch. Default implementation calls `embed` per item.
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Embedding>, EmbedError> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(t).await?);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

/// Cosine similarity between two vectors of equal length, in [-1.0, 1.0].
/// Returns 0.0 when either vector is all zeros (degenerate).
/// Panics in debug builds when dims mismatch; returns 0.0 in release.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(
        a.len(),
        b.len(),
        "cosine_similarity: dimension mismatch {} vs {}",
        a.len(),
        b.len()
    );
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let norm_a = norm_a.sqrt();
    let norm_b = norm_b.sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// Reciprocal Rank Fusion of N ranked lists of memory ids.
///
/// Score for doc d: sum over each ranking r containing d of 1/(k + rank_r(d)),
/// where rank is 1-based. `k` defaults to 60 (the standard).
///
/// Returns memory ids sorted by descending fused score; ties broken by id asc.
pub fn rrf_fuse(rankings: &[Vec<String>], k: usize) -> Vec<String> {
    let k = if k == 0 { 60 } else { k };
    let mut scores: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for ranking in rankings {
        for (rank, id) in ranking.iter().enumerate() {
            *scores.entry(id.clone()).or_insert(0.0) +=
                1.0 / (k as f64 + (rank + 1) as f64);
        }
    }
    let mut entries: Vec<(String, f64)> = scores.into_iter().collect();
    // Sort descending by score; ties broken by id ascending.
    entries.sort_by(|(id_a, score_a), (id_b, score_b)| {
        score_b
            .partial_cmp(score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| id_a.cmp(id_b))
    });
    entries.into_iter().map(|(id, _)| id).collect()
}

/// One memory paired with its similarity score (for ranking / debugging).
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredMemory {
    pub memory: Memory,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// MockEmbedder — deterministic, for tests
// ---------------------------------------------------------------------------

/// Deterministic embedder for tests.
///
/// Algorithm:
/// 1. Lowercase and trim the input.
/// 2. For each byte b at position i: vec[(i + (b as usize)) % dim] += 1.0
/// 3. L2-normalise the result.
///
/// Same text -> identical vector. Similar texts -> high cosine similarity.
/// Gated behind `#[cfg(any(test, feature = "test-stub"))]`.
#[cfg(any(test, feature = "test-stub"))]
pub struct MockEmbedder {
    dim: usize,
}

#[cfg(any(test, feature = "test-stub"))]
impl MockEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[cfg(any(test, feature = "test-stub"))]
#[async_trait]
impl Embedder for MockEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, text: &str) -> Result<Embedding, EmbedError> {
        let text = text.to_lowercase();
        let text = text.trim();
        if text.is_empty() {
            return Err(EmbedError::InvalidInput("empty text".into()));
        }
        let mut vec = vec![0.0_f32; self.dim];
        for (i, byte) in text.bytes().enumerate() {
            vec[(i + byte as usize) % self.dim] += 1.0;
        }
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut vec {
                *x /= norm;
            }
        }
        Ok(vec)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- cosine_similarity ---------------------------------------------------

    #[test]
    fn cosine_identical_vectors_returns_one() {
        let v = vec![1.0_f32, 2.0, 3.0];
        let result = cosine_similarity(&v, &v);
        assert!(
            (result - 1.0).abs() < 1e-5,
            "identical vectors should have cosine similarity 1.0, got {result}"
        );
    }

    #[test]
    fn cosine_orthogonal_vectors_returns_zero() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        let result = cosine_similarity(&a, &b);
        assert!(
            result.abs() < 1e-5,
            "orthogonal vectors should have cosine similarity 0.0, got {result}"
        );
    }

    #[test]
    fn cosine_opposite_vectors_returns_negative_one() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![-1.0_f32, 0.0, 0.0];
        let result = cosine_similarity(&a, &b);
        assert!(
            (result + 1.0).abs() < 1e-5,
            "opposite vectors should have cosine similarity -1.0, got {result}"
        );
    }

    #[test]
    fn cosine_zero_vector_returns_zero() {
        let zero = vec![0.0_f32, 0.0, 0.0];
        let nonzero = vec![1.0_f32, 2.0, 3.0];
        let result = cosine_similarity(&zero, &nonzero);
        assert!(
            result.abs() < 1e-5,
            "zero vector should yield 0.0 similarity, got {result}"
        );
        // also symmetric
        let result2 = cosine_similarity(&nonzero, &zero);
        assert!(
            result2.abs() < 1e-5,
            "zero vector (second arg) should yield 0.0 similarity, got {result2}"
        );
    }

    // --- rrf_fuse ------------------------------------------------------------

    #[test]
    fn rrf_empty_rankings_returns_empty() {
        let result = rrf_fuse(&[], 60);
        assert!(result.is_empty(), "empty rankings should return empty vec");
    }

    #[test]
    fn rrf_single_ranking_preserves_order() {
        let ranking = vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        ];
        let result = rrf_fuse(&ranking, 60);
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn rrf_two_rankings_with_overlap_higher_doc_wins() {
        // Rankings: [a, b, c] and [b, a, d]
        // a_score = 1/(60+1) + 1/(60+2) = 1/61 + 1/62 ≈ 0.03252
        // b_score = 1/(60+2) + 1/(60+1) = 1/62 + 1/61 ≈ 0.03252  (same as a)
        // c_score = 1/(60+3) = 1/63 ≈ 0.01587
        // d_score = 1/(60+3) = 1/63 ≈ 0.01587  (same as c)
        // a and b tied → tie broken by id asc → a before b
        // c and d tied → tie broken by id asc → c before d
        // Expected: [a, b, c, d]
        let rankings = vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["b".to_string(), "a".to_string(), "d".to_string()],
        ];
        let result = rrf_fuse(&rankings, 60);
        assert_eq!(result, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn rrf_doc_in_all_rankings_ranks_first() {
        // "common" appears in both rankings at rank 1 → highest score
        let rankings = vec![
            vec!["common".to_string(), "x".to_string()],
            vec!["common".to_string(), "y".to_string()],
        ];
        let result = rrf_fuse(&rankings, 60);
        assert_eq!(
            result[0], "common",
            "doc appearing in all rankings should rank first"
        );
    }

    // --- MockEmbedder --------------------------------------------------------

    #[tokio::test]
    async fn mock_embedder_dim_returns_constructed_dim() {
        let embedder = MockEmbedder::new(384);
        assert_eq!(embedder.dim(), 384);
    }

    #[tokio::test]
    async fn mock_embedder_same_text_same_embedding() {
        let embedder = MockEmbedder::new(64);
        let v1 = embedder.embed("hello world").await.unwrap();
        let v2 = embedder.embed("hello world").await.unwrap();
        assert_eq!(v1, v2, "same text must produce identical embedding");
    }

    #[tokio::test]
    async fn mock_embedder_different_text_different_embedding() {
        let embedder = MockEmbedder::new(64);
        let v1 = embedder.embed("rust programming").await.unwrap();
        let v2 = embedder.embed("zzzzzzzzzzzzzzzzz").await.unwrap();
        assert_ne!(
            v1, v2,
            "different texts should produce different embeddings"
        );
    }

    #[tokio::test]
    async fn mock_embedder_l2_normalized() {
        let embedder = MockEmbedder::new(32);
        let v = embedder.embed("normalize me").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "output vector must be L2-normalized, got norm={norm}"
        );
    }
}
