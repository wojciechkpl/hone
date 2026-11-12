#![allow(clippy::unwrap_used, clippy::expect_used)]
//! M4-M5 of `docs/plans/memory-recall-index-spec.md`: `SqliteIndex::search`
//! matches plain-English prompts. No new API; the query string is split into
//! word tokens (letters, digits, `_`), lower-cased, minus English stop words and
//! tokens shorter than 3 characters (at most the first 16 are used). A symbol
//! matches if its name contains any token (case-insensitive, `%`/`_` literal).
//! Ranking: distinct tokens contained, then exact over prefix over substring,
//! then shorter name; then cut to `limit`.

use std::path::PathBuf;

use hone_index::SqliteIndex;

async fn index_with(fns: &[&str]) -> SqliteIndex {
    let idx = SqliteIndex::in_memory().await.unwrap();
    let src: String = fns.iter().map(|n| format!("fn {n}() {{}}\n")).collect();
    idx.index_file(&PathBuf::from("lib.rs"), &src).await.unwrap();
    idx
}

async fn names(idx: &SqliteIndex, query: &str, limit: usize) -> Vec<String> {
    idx.search(query, limit)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.symbol.name)
        .collect()
}

#[tokio::test]
async fn test_m4_plain_english_prompt_should_find_symbols_by_its_words() {
    let idx = index_with(&["login", "handle_login", "parse_config", "render_page"]).await;
    let got = names(&idx, "fix the login bug", 10).await;
    assert!(got.contains(&"login".to_owned()), "{got:?}");
    assert!(got.contains(&"handle_login".to_owned()), "{got:?}");
    assert!(!got.contains(&"parse_config".to_owned()), "{got:?}");
}

#[tokio::test]
async fn test_m4_matching_is_case_insensitive_on_both_sides() {
    let idx = index_with(&["Login", "HandleLogin"]).await;
    let got = names(&idx, "Please FIX LOGIN", 10).await;
    assert!(got.contains(&"Login".to_owned()), "{got:?}");
    assert!(got.contains(&"HandleLogin".to_owned()), "{got:?}");
}

#[tokio::test]
async fn test_m4_more_matched_tokens_should_rank_first() {
    let idx = index_with(&["login", "login_bug_report", "bug_tracker"]).await;
    let got = names(&idx, "login bug", 10).await;
    assert_eq!(got, ["login_bug_report", "login", "bug_tracker"]);
}

#[tokio::test]
async fn test_m4_single_identifier_query_should_keep_todays_results_and_order() {
    let idx = index_with(&["handle_login", "login_user", "login", "logout"]).await;
    // exact, then prefix, then substring
    assert_eq!(names(&idx, "login", 10).await, ["login", "login_user", "handle_login"]);
    assert_eq!(names(&idx, "handle_login", 10).await, ["handle_login"]);
}

#[tokio::test]
async fn test_m4_query_without_usable_tokens_should_return_nothing() {
    let idx = index_with(&["the_end", "to_do", "a_b", "and_then"]).await;
    for q in ["the", "the and for", "a of to", "", "   ", "!!! ??? ..."] {
        assert!(names(&idx, q, 10).await.is_empty(), "query {q:?}");
    }
}

#[tokio::test]
async fn test_m5_underscore_in_a_token_should_match_literally() {
    let idx = index_with(&["a_b", "axb", "a_bc"]).await;
    let got = names(&idx, "a_b", 10).await;
    assert!(!got.contains(&"axb".to_owned()), "`_` acted as a wildcard: {got:?}");
    assert!(got.contains(&"a_b".to_owned()), "{got:?}");
}

#[tokio::test]
async fn test_m5_percent_signs_should_never_act_as_wildcards() {
    let idx = index_with(&["login", "report100", "anything"]).await;
    assert!(names(&idx, "%%%", 10).await.is_empty(), "a wildcard-only query matches nothing");
    // "100%" -> token "100": matches report100 only.
    assert_eq!(names(&idx, "100%", 10).await, ["report100"]);
}

#[tokio::test]
async fn test_m5_only_the_first_sixteen_tokens_should_be_used() {
    let idx = index_with(&["token05", "token25"]).await;
    let query: String = (1..=30).map(|i| format!("token{i:02} ")).collect();
    let got = names(&idx, &query, 50).await;
    assert!(got.contains(&"token05".to_owned()), "{got:?}");
    assert!(!got.contains(&"token25".to_owned()), "token 25 is past the cap: {got:?}");
}

#[tokio::test]
async fn test_m5_ranking_should_still_pick_the_best_when_many_symbols_match_one_token() {
    // 300 symbols match "login" only; the best one matches both tokens and comes
    // last in the file, so a row cut before ranking would lose it.
    let mut fns: Vec<String> = (0..300).map(|i| format!("login_{i:03}")).collect();
    fns.push("login_bug".to_owned());
    let refs: Vec<&str> = fns.iter().map(String::as_str).collect();
    let idx = index_with(&refs).await;
    assert_eq!(names(&idx, "login bug", 1).await, ["login_bug"]);
}

#[tokio::test]
async fn test_m4_limit_should_cut_the_ranked_results() {
    let idx = index_with(&["login", "login_user", "login_session", "handle_login"]).await;
    let got = names(&idx, "login", 2).await;
    assert_eq!(got, ["login", "login_user"]);
}
