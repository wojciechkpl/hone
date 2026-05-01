use hone_memory::bm25::{tokenize, Bm25Index, Bm25Params};

// ---------------------------------------------------------------------------
// tokenize tests
// ---------------------------------------------------------------------------

#[test]
fn tokenize_lowercases_and_splits_on_punctuation() {
    let tokens = tokenize("Hello, World!");
    assert_eq!(tokens, vec!["hello", "world"]);

    // `-` is not alphanumeric, so it is a separator.
    // `_` is not alphanumeric either, so it splits too.
    let tokens = tokenize("snake_case-and-kebab");
    assert_eq!(tokens, vec!["snake", "case", "and", "kebab"]);
}

#[test]
fn tokenize_strips_empty_and_whitespace_only() {
    assert!(tokenize("   ").is_empty());

    let tokens = tokenize("a  b");
    assert_eq!(tokens, vec!["a", "b"]);
}

#[test]
fn tokenize_handles_unicode_alphanumeric() {
    let tokens = tokenize("café résumé");
    assert_eq!(tokens, vec!["café", "résumé"]);
}

// ---------------------------------------------------------------------------
// Bm25Index tests
// ---------------------------------------------------------------------------

fn default_params() -> Bm25Params {
    Bm25Params::default()
}

#[test]
fn bm25_higher_score_for_more_query_term_occurrences() {
    // doc[0]: "rust" once
    // doc[1]: "rust rust rust" (three occurrences)
    // doc[2]: no "rust"
    let docs = &[
        "rust programming",
        "rust rust rust systems",
        "python is great",
    ];
    let idx = Bm25Index::build(docs, default_params());

    let s0 = idx.score(0, "rust");
    let s1 = idx.score(1, "rust");
    let s2 = idx.score(2, "rust");

    assert!(s1 > s0, "doc with 3x 'rust' should outscore doc with 1x 'rust'");
    assert!(s0 > 0.0, "doc with 'rust' should have positive score");
    assert_eq!(s2, 0.0, "doc without 'rust' should score 0.0");
}

#[test]
fn bm25_idf_penalises_common_terms() {
    // "the" appears in ALL docs; "rust" appears in only doc[0].
    // Querying for "rust" should give a higher score to doc[0] than querying
    // "the" gives to any doc (rare-term gets stronger IDF boost).
    let docs = &[
        "the rust programming language",
        "the quick brown fox",
        "the lazy dog sleeps",
    ];
    let idx = Bm25Index::build(docs, default_params());

    let rust_score = idx.score(0, "rust");
    // "the" is in every doc, so IDF is near 0; pick any doc.
    let the_score_max = (0..3).map(|i| idx.score(i, "the")).fold(0.0_f64, f64::max);

    assert!(
        rust_score > the_score_max,
        "rare term 'rust' should beat common term 'the': rust={rust_score} the_max={the_score_max}"
    );
}

#[test]
fn bm25_length_normalises_long_docs() {
    // doc A: 5 tokens including "rust" once.
    // doc B: 50 tokens including "rust" once (padded with filler words).
    let short_doc = "rust is great for systems";
    let long_filler = "filler ".repeat(45);
    let long_doc = format!("rust {long_filler}");

    let docs: Vec<&str> = vec![short_doc, long_doc.as_str()];
    let idx = Bm25Index::build(&docs, default_params());

    let s_short = idx.score(0, "rust");
    let s_long = idx.score(1, "rust");

    assert!(
        s_short > s_long,
        "shorter doc should outscore longer doc for same tf: short={s_short} long={s_long}"
    );
}

#[test]
fn bm25_zero_score_for_disjoint_query() {
    let docs = &["python machine learning", "deep neural networks"];
    let idx = Bm25Index::build(docs, default_params());

    assert_eq!(idx.score(0, "rust"), 0.0);
    assert_eq!(idx.score(1, "rust"), 0.0);
}

#[test]
fn bm25_empty_query_returns_zero() {
    let docs = &["some content here"];
    let idx = Bm25Index::build(docs, default_params());

    assert_eq!(idx.score(0, ""), 0.0);
}

#[test]
fn bm25_empty_corpus_score_does_not_panic() {
    // Building on an empty corpus must not panic.
    // Scoring on an empty corpus always returns 0.0 (no documents).
    let idx = Bm25Index::build(&[], default_params());
    // With an empty corpus there are no valid doc indices; the safe contract
    // is that score returns 0.0 rather than panicking.
    assert_eq!(idx.score(0, "rust"), 0.0);
}

#[test]
fn bm25_index_is_deterministic() {
    let docs = &[
        "rust ownership borrowing lifetimes",
        "python dynamic typing duck typing",
        "rust async await tokio",
    ];
    let params = Bm25Params { k1: 1.5, b: 0.75 };

    let idx_a = Bm25Index::build(docs, params);
    let idx_b = Bm25Index::build(docs, Bm25Params { k1: 1.5, b: 0.75 });

    for i in 0..docs.len() {
        let sa = idx_a.score(i, "rust ownership");
        let sb = idx_b.score(i, "rust ownership");
        assert_eq!(
            sa, sb,
            "scores must be identical across two builds for doc {i}"
        );
    }
}
