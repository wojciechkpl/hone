//! BM25 ranking index.
//!
//! Provides [`tokenize`], [`Bm25Params`], and [`Bm25Index`].
//! The index is built once over a corpus of documents and can then score any
//! document against an arbitrary query.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Tuning parameters for BM25.
///
/// `k1` controls term-frequency saturation; `b` controls length normalisation.
pub struct Bm25Params {
    pub k1: f64,
    pub b: f64,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.5, b: 0.75 }
    }
}

// ---------------------------------------------------------------------------
// Tokenisation
// ---------------------------------------------------------------------------

/// Tokenise `text` for BM25 indexing / querying.
///
/// Rules:
/// - Lowercases all characters.
/// - Splits on any character that is **not** Unicode-alphanumeric (`char::is_alphanumeric()`).
///   This means `_` and `-` are separators, but accented letters like `é` are kept.
/// - Drops empty tokens.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------

/// Pre-computed per-document data stored at build time.
struct DocData {
    /// Term → raw term frequency in this document.
    tf: HashMap<String, u32>,
    /// Number of tokens in this document.
    len: u32,
}

/// BM25 index built over a fixed corpus.
///
/// Build once with [`Bm25Index::build`]; call [`Bm25Index::score`] as many
/// times as needed — all per-document tokenisation happens at build time.
pub struct Bm25Index {
    docs: Vec<DocData>,
    /// Document-frequency: term → number of docs containing it.
    df: HashMap<String, u32>,
    /// Average document length (in tokens), pre-computed.
    avgdl: f64,
    params: Bm25Params,
}

impl Bm25Index {
    /// Build an index over `docs`.
    ///
    /// Each element of `docs` is treated as a single document.  Tokenisation
    /// (lowercase + split on non-alphanumeric) happens once per document here.
    pub fn build(docs: &[&str], params: Bm25Params) -> Self {
        let mut doc_data: Vec<DocData> = Vec::with_capacity(docs.len());
        let mut df: HashMap<String, u32> = HashMap::new();
        let mut total_len: u64 = 0;

        for &text in docs {
            let tokens = tokenize(text);
            let len = tokens.len() as u32;
            total_len += len as u64;

            // Build per-doc tf map.
            let mut tf: HashMap<String, u32> = HashMap::new();
            for token in tokens {
                *tf.entry(token).or_insert(0) += 1;
            }

            // Update global df: each term counts once per doc regardless of tf.
            for term in tf.keys() {
                *df.entry(term.clone()).or_insert(0) += 1;
            }

            doc_data.push(DocData { tf, len });
        }

        let n = docs.len() as f64;
        let avgdl = if docs.is_empty() {
            0.0
        } else {
            total_len as f64 / n
        };

        Self {
            docs: doc_data,
            df,
            avgdl,
            params,
        }
    }

    /// Compute the BM25 score of document `doc_idx` against `query`.
    ///
    /// Returns `0.0` for:
    /// - An empty query.
    /// - `doc_idx` out of bounds.
    /// - A corpus where `avgdl == 0` (all documents were empty).
    /// - A query term that does not appear in the document.
    pub fn score(&self, doc_idx: usize, query: &str) -> f64 {
        if query.is_empty() {
            return 0.0;
        }
        if doc_idx >= self.docs.len() {
            return 0.0;
        }
        // Guard against all-empty-doc corpus.
        if self.avgdl == 0.0 {
            return 0.0;
        }

        let doc = &self.docs[doc_idx];
        let n = self.docs.len() as f64;
        let k1 = self.params.k1;
        let b = self.params.b;
        let dl = doc.len as f64;

        let query_tokens = tokenize(query);
        let mut total = 0.0_f64;

        for term in &query_tokens {
            let tf = doc.tf.get(term).copied().unwrap_or(0) as f64;
            if tf == 0.0 {
                // Term not in document — contributes 0 regardless of IDF.
                continue;
            }

            // `df == 0` only when the term is absent from the corpus; in that
            // case `tf == 0` for every doc and the contribution is skipped above
            // by the `continue`, so this branch is unreachable in practice.
            let df = self.df.get(term).copied().unwrap_or(0) as f64;
            // IDF: ln( 1 + (N - df + 0.5) / (df + 0.5) )
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();

            // TF normalised for length.
            let tf_norm = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / self.avgdl));

            total += idf * tf_norm;
        }

        total
    }
}
