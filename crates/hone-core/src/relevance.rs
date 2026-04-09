//! Lightweight BM25 scorer for context-window relevance ranking.
//!
//! Reorders middle-zone messages so the most query-relevant content sits
//! at the edges of the context window (where LLM attention is strongest),
//! based on the "Lost in the Middle" finding (Liu et al., TACL 2024).

use std::collections::HashMap;

const K1: f32 = 1.2;
const B: f32 = 0.75;

/// BM25 scorer built from a small corpus of document strings.
pub struct Bm25Scorer {
    /// IDF values: term → inverse document frequency.
    idf: HashMap<String, f32>,
    /// Average document length (in terms) across the corpus.
    avg_dl: f32,
    /// Number of documents in the corpus.
    n: usize,
}

impl Bm25Scorer {
    /// Build a scorer from a corpus of document texts.
    ///
    /// Computes IDF for all terms across the corpus. Designed for small
    /// corpora (< 100 documents) typical of a conversation context window.
    pub fn from_corpus(documents: &[String]) -> Self {
        let n = documents.len();
        if n == 0 {
            return Self {
                idf: HashMap::new(),
                avg_dl: 0.0,
                n: 0,
            };
        }

        // Count document frequency for each term.
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut total_len: usize = 0;

        for doc in documents {
            let terms = tokenize(doc);
            total_len += terms.len();
            // Unique terms in this document.
            let unique: std::collections::HashSet<&str> =
                terms.iter().map(|s| s.as_str()).collect();
            for term in unique {
                *df.entry(term.to_string()).or_default() += 1;
            }
        }

        let avg_dl = total_len as f32 / n as f32;

        // Compute IDF: log((N - df + 0.5) / (df + 0.5) + 1)
        let idf: HashMap<String, f32> = df
            .into_iter()
            .map(|(term, freq)| {
                let val = ((n as f32 - freq as f32 + 0.5) / (freq as f32 + 0.5) + 1.0).ln();
                (term, val.max(0.0))
            })
            .collect();

        Self { idf, avg_dl, n }
    }

    /// Score a single document against a query.
    ///
    /// Returns 0.0 when the scorer has an empty corpus.
    pub fn score(&self, document: &str, query: &str) -> f32 {
        if self.n == 0 || self.avg_dl == 0.0 {
            return 0.0;
        }

        let doc_terms = tokenize(document);
        let query_terms = tokenize(query);
        let dl = doc_terms.len() as f32;

        // Term frequency in the document.
        let mut tf: HashMap<&str, u32> = HashMap::new();
        for t in &doc_terms {
            *tf.entry(t.as_str()).or_default() += 1;
        }

        let mut score = 0.0f32;
        for qt in &query_terms {
            let idf = self.idf.get(qt.as_str()).copied().unwrap_or(0.0);
            let freq = tf.get(qt.as_str()).copied().unwrap_or(0) as f32;
            let numerator = freq * (K1 + 1.0);
            let denominator = freq + K1 * (1.0 - B + B * dl / self.avg_dl);
            score += idf * numerator / denominator;
        }

        score
    }
}

/// Tokenize text into lowercase terms.
///
/// Splits on whitespace and common punctuation, producing terms suitable
/// for BM25 scoring on code. Keeps identifiers intact (snake_case stays
/// as one token, which is desirable for matching function names).
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '{' | '}' | '[' | ']' | ',' | ';' | ':' | '"' | '\'' | '<' | '>' | '=' | '+' | '/' | '\\' | '#'))
        .filter(|s| !s.is_empty() && s.len() > 1)
        .map(|s| s.to_lowercase())
        .collect()
}

/// Interleave items so the highest-scored sit at the edges (first and last
/// positions) and the lowest-scored sit in the center.
///
/// Input must be sorted by score descending.
pub fn interleave_edges<T>(sorted_desc: Vec<T>) -> Vec<T> {
    if sorted_desc.len() <= 2 {
        return sorted_desc;
    }

    let n = sorted_desc.len();
    let mut result: Vec<Option<T>> = (0..n).map(|_| None).collect();
    let mut front = 0;
    let mut back = n - 1;
    let mut place_front = true;

    for item in sorted_desc {
        if place_front {
            result[front] = Some(item);
            front += 1;
        } else {
            result[back] = Some(item);
            if back > 0 {
                back -= 1;
            }
        }
        place_front = !place_front;
    }

    result.into_iter().map(|o| o.unwrap()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_splits_on_punctuation() {
        let tokens = tokenize("fn main() { println!(\"hello\"); }");
        assert!(tokens.contains(&"fn".to_string()));
        assert!(tokens.contains(&"main".to_string()));
        // println! keeps the ! since it's a Rust macro — that's fine for matching
        assert!(tokens.contains(&"println!".to_string()));
        assert!(tokens.contains(&"hello".to_string()));
    }

    #[test]
    fn test_tokenize_lowercases() {
        let tokens = tokenize("ReadFile MyStruct");
        assert!(tokens.contains(&"readfile".to_string()));
        assert!(tokens.contains(&"mystruct".to_string()));
    }

    #[test]
    fn test_tokenize_keeps_snake_case() {
        let tokens = tokenize("my_function_name");
        assert!(tokens.contains(&"my_function_name".to_string()));
    }

    #[test]
    fn test_empty_corpus_scores_zero() {
        let scorer = Bm25Scorer::from_corpus(&[]);
        assert_eq!(scorer.score("hello world", "hello"), 0.0);
    }

    #[test]
    fn test_relevant_document_scores_higher() {
        let docs = vec![
            "fn login(user: &str, password: &str) -> Result<Token>".to_string(),
            "fn read_config(path: &Path) -> Config".to_string(),
            "fn run_database_migration() -> Result<()>".to_string(),
        ];
        let scorer = Bm25Scorer::from_corpus(&docs);

        let s_login = scorer.score(&docs[0], "fix login authentication bug");
        let s_config = scorer.score(&docs[1], "fix login authentication bug");
        let s_db = scorer.score(&docs[2], "fix login authentication bug");

        assert!(
            s_login > s_config,
            "login doc ({s_login}) should score higher than config doc ({s_config})"
        );
        assert!(
            s_login > s_db,
            "login doc ({s_login}) should score higher than db doc ({s_db})"
        );
    }

    #[test]
    fn test_identical_query_and_doc_scores_high() {
        let docs = vec![
            "authentication error in login handler".to_string(),
            "unrelated database schema migration".to_string(),
        ];
        let scorer = Bm25Scorer::from_corpus(&docs);
        let s1 = scorer.score(&docs[0], "authentication error in login handler");
        let s2 = scorer.score(&docs[1], "authentication error in login handler");
        assert!(s1 > s2 * 2.0, "exact match should score much higher");
    }

    #[test]
    fn test_interleave_edges_empty() {
        let result: Vec<i32> = interleave_edges(vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_interleave_edges_single() {
        assert_eq!(interleave_edges(vec![1]), vec![1]);
    }

    #[test]
    fn test_interleave_edges_places_highest_at_boundaries() {
        // Input sorted desc: [10, 8, 6, 4, 2]
        let result = interleave_edges(vec![10, 8, 6, 4, 2]);
        // Highest (10) at front, 2nd highest (8) at back
        assert_eq!(result[0], 10);
        assert_eq!(result[result.len() - 1], 8);
        // Lowest (2) should be near the center
        let center = result.len() / 2;
        assert!(
            result[center] <= 6,
            "center should have low-scored items, got {}",
            result[center]
        );
    }

    #[test]
    fn test_interleave_preserves_all_elements() {
        let input = vec![5, 4, 3, 2, 1];
        let result = interleave_edges(input.clone());
        assert_eq!(result.len(), input.len());
        let mut sorted_result = result.clone();
        sorted_result.sort();
        assert_eq!(sorted_result, vec![1, 2, 3, 4, 5]);
    }
}
