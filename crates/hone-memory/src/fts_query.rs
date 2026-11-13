//! What a word is, for every memory search: the in-Rust BM25 and the FTS5 query
//! split text the same way (on every character that is not a Unicode letter or
//! digit), lowercase it and fold diacritics of LATIN base letters only (NFD,
//! their combining marks removed, so `kąt` is `kat`, `café` is `cafe` and the
//! Vietnamese `ệ` is `e`; `ł` is not decomposable and stays `ł`, as in FTS5's
//! `unicode61 remove_diacritics 2`). Other scripts keep their marks and are
//! recomposed to NFC: Hangul syllables, kana voicing marks (`パ` is not `ハ`),
//! Devanagari vowel signs and virama, Thai marks.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// English stop words and two-letter function words dropped from FTS5 queries.
const STOP_WORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "are",
    "was",
    "were",
    "been",
    "being",
    "but",
    "not",
    "you",
    "your",
    "yours",
    "his",
    "her",
    "hers",
    "its",
    "our",
    "ours",
    "their",
    "theirs",
    "this",
    "that",
    "these",
    "those",
    "who",
    "whom",
    "whose",
    "which",
    "what",
    "when",
    "where",
    "why",
    "how",
    "can",
    "could",
    "should",
    "would",
    "will",
    "shall",
    "may",
    "might",
    "must",
    "has",
    "have",
    "had",
    "having",
    "does",
    "did",
    "doing",
    "done",
    "from",
    "into",
    "onto",
    "over",
    "under",
    "about",
    "above",
    "below",
    "between",
    "through",
    "during",
    "before",
    "after",
    "than",
    "then",
    "too",
    "very",
    "also",
    "just",
    "any",
    "all",
    "some",
    "such",
    "each",
    "both",
    "few",
    "more",
    "most",
    "other",
    "only",
    "own",
    "same",
    "out",
    "off",
    "again",
    "once",
    "here",
    "there",
    "because",
    "until",
    "while",
    "against",
    "per",
    "via",
    "let",
    "get",
    "got",
    "him",
    "she",
    "they",
    "them",
    "myself",
    "yourself",
    "itself",
    "themselves",
    "ourselves",
    "of",
    "to",
    "in",
    "is",
    "it",
    "an",
    "as",
    "at",
    "be",
    "by",
    "do",
    "he",
    "if",
    "me",
    "my",
    "no",
    "on",
    "or",
    "so",
    "up",
    "us",
    "we",
    "am",
];

/// The most tokens an FTS5 query carries.
pub const MAX_QUERY_TOKENS: usize = 32;

/// A letter of the Latin script (the ones whose diacritics `unicode61` folds).
fn is_latin(c: char) -> bool {
    matches!(c,
        'a'..='z' | 'A'..='Z'
        | '\u{00C0}'..='\u{024F}'
        | '\u{1E00}'..='\u{1EFF}'
        | '\u{2C60}'..='\u{2C7F}'
        | '\u{A720}'..='\u{A7FF}')
}

/// `text` lowercased, with the diacritics of Latin letters removed and the
/// rest recomposed (NFC).
pub fn fold(text: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    let mut after_latin = false;
    for c in text.to_lowercase().nfd() {
        if is_combining_mark(c) {
            if after_latin {
                continue;
            }
        } else {
            after_latin = is_latin(c);
        }
        out.push(c);
    }
    out.into_iter().nfc().collect()
}

/// The words of `text`: folded, split on non-(letter or digit), empty ones dropped.
pub fn word_tokens(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !(c.is_alphanumeric() || is_combining_mark(c)))
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The distinct query tokens of a prompt in order of appearance: words of 2 or
/// more characters that are not stop words.
pub fn query_tokens(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    word_tokens(text)
        .into_iter()
        .filter(|t| t.chars().count() >= 2 && !STOP_WORDS.contains(&t.as_str()))
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

/// Keep at most [`MAX_QUERY_TOKENS`] of `tokens`: the rarest by `frequency`
/// (documents holding the token), ties in order of appearance; the kept ones stay
/// in order of appearance.
pub fn rarest(tokens: Vec<String>, frequency: &[u64]) -> Vec<String> {
    if tokens.len() <= MAX_QUERY_TOKENS {
        return tokens;
    }
    let mut order: Vec<usize> = (0..tokens.len()).collect();
    order.sort_by_key(|&i| (frequency[i], i));
    let mut keep: Vec<usize> = order.into_iter().take(MAX_QUERY_TOKENS).collect();
    keep.sort_unstable();
    keep.into_iter().map(|i| tokens[i].clone()).collect()
}

/// The FTS5 `MATCH` expression of `tokens`: each double-quoted, OR-ed. `None`
/// when there is none.
pub fn match_expression(tokens: &[String]) -> Option<String> {
    if tokens.is_empty() {
        return None;
    }
    Some(
        tokens
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}
