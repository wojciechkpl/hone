#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R6 (source name) and R8 of `docs/plans/doc-retrieval-spec.md`: the `[retrieval]` tables.
//! API (extends `tests/strategy_selection.rs`): `Config` gains an optional `retrieval` table
//! with `deny_unknown_fields` and the keys `top_k`, `max_tokens`, `chunker` and the sub-table
//! `[retrieval.fixed]` with `lines` and `overlap`; `StrategySelection` gains
//! `retrieval_top_k: Chosen<usize>` (3), `retrieval_max_tokens: Chosen<u32>` (1200),
//! `retrieval_chunker: Chosen<String>` ("markdown" | "fixed"; default "markdown"),
//! `retrieval_fixed_lines: Chosen<usize>` (40) and `retrieval_fixed_overlap: Chosen<usize>` (5).
//! `"retrieval"` is a valid `[context] sources` name that is NOT in the default sources.
//! Ranges: `top_k` 1-20, `max_tokens` 100-8000 (errors name the file, the key and the range).
//! A project file may only lower `top_k` and `max_tokens` (a higher value is ignored with a
//! warning naming the project file); it cannot add `retrieval` to the sources.

use std::path::Path;

use hone_config::ConfigError;
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, Origin, StrategySelection};

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn layer(path: &str, raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml(path, raw).unwrap_or_else(|e| panic!("fixture should parse: {e}\n{raw}"))
}
fn no_env(_: &str) -> Option<String> {
    None
}
fn select(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> StrategySelection {
    StrategySelection::from_sources(u, p, &no_env, &Flags::default()).unwrap_or_else(|e| panic!("{e}"))
}
fn select_err(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> ConfigError {
    StrategySelection::from_sources(u, p, &no_env, &Flags::default()).expect_err("should fail")
}
fn file(p: &str) -> Origin {
    Origin::File(p.into())
}
fn warns(s: &StrategySelection, path: &str) -> Vec<String> {
    s.warnings.iter().filter(|w| w.path == Path::new(path)).map(|w| w.message.clone()).collect()
}

#[test]
fn test_r6_retrieval_should_be_off_by_default_with_the_documented_defaults() {
    let s = select(None, None);
    assert!(!s.sources.value.contains(&"retrieval".to_owned()));
    assert_eq!((s.retrieval_top_k.value, s.retrieval_top_k.origin.clone()), (3, Origin::Default));
    assert_eq!((s.retrieval_max_tokens.value, s.retrieval_max_tokens.origin.clone()), (1200, Origin::Default));
    assert_eq!((s.retrieval_chunker.value.as_str(), s.retrieval_chunker.origin), ("markdown", Origin::Default));
    assert_eq!((s.retrieval_fixed_lines.value, s.retrieval_fixed_overlap.value), (40, 5));
}

#[test]
fn test_r6_the_user_file_can_turn_it_on_but_a_project_file_cannot() {
    let u = layer(USER, "[context]\nsources = [\"recall\", \"retrieval\"]\n");
    assert!(select(Some(&u), None).sources.value.contains(&"retrieval".to_owned()));

    let p = layer(PROJECT, "[context]\nsources = [\"recall\", \"retrieval\"]\n");
    assert_eq!(select(None, Some(&p)).sources.value, ["recall"]);
    assert!(select(Some(&u), Some(&p)).sources.value.contains(&"retrieval".to_owned()), "the user turned it on");
}

#[test]
fn test_r8_every_key_should_parse_with_its_file_as_origin() {
    let u = layer(
        USER,
        "[retrieval]\ntop_k = 5\nmax_tokens = 2000\nchunker = \"fixed\"\n\n[retrieval.fixed]\nlines = 20\noverlap = 2\n",
    );

    let s = select(Some(&u), None);

    assert_eq!((s.retrieval_top_k.value, s.retrieval_top_k.origin.clone()), (5, file(USER)));
    assert_eq!((s.retrieval_max_tokens.value, s.retrieval_max_tokens.origin.clone()), (2000, file(USER)));
    assert_eq!((s.retrieval_chunker.value.as_str(), s.retrieval_chunker.origin), ("fixed", file(USER)));
    assert_eq!((s.retrieval_fixed_lines.value, s.retrieval_fixed_lines.origin), (20, file(USER)));
    assert_eq!((s.retrieval_fixed_overlap.value, s.retrieval_fixed_overlap.origin), (2, file(USER)));
}

#[test]
fn test_r8_the_ranges_should_be_enforced_naming_file_key_and_range() {
    for (raw, key, range) in [
        ("[retrieval]\ntop_k = 0\n", "top_k", "20"),
        ("[retrieval]\ntop_k = 21\n", "top_k", "20"),
        ("[retrieval]\nmax_tokens = 99\n", "max_tokens", "8000"),
        ("[retrieval]\nmax_tokens = 8001\n", "max_tokens", "8000"),
    ] {
        let e = select_err(Some(&layer(USER, raw)), None);
        match e {
            ConfigError::InvalidField { path, reason } => {
                assert_eq!(path, Path::new(USER), "{raw}");
                assert!(reason.contains(key) && reason.contains(range), "{raw}: {reason}");
            }
            other => panic!("{other:?}"),
        }
    }
    for raw in [
        "[retrieval]\ntop_k = 1\n",
        "[retrieval]\ntop_k = 20\n",
        "[retrieval]\nmax_tokens = 100\n",
        "[retrieval]\nmax_tokens = 8000\n",
    ] {
        select(Some(&layer(USER, raw)), None);
    }
}

#[test]
fn test_r8_unknown_keys_and_wrong_types_should_be_errors_naming_the_file() {
    for raw in [
        "[retrieval]\nbogus = 1\n",
        "[retrieval.fixed]\nbogus = 1\n",
        "[retrieval]\ntop_k = \"three\"\n",
        "[retrieval.fixed]\nlines = 2.5\n",
    ] {
        match ConfigLayer::from_toml(PROJECT, raw).expect_err(raw) {
            ConfigError::InvalidField { path, .. } => assert_eq!(path, Path::new(PROJECT), "{raw}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn test_r8_an_unknown_chunker_name_should_fail_listing_the_valid_names() {
    for (path, is_user) in [(USER, true), (PROJECT, false)] {
        let l = layer(path, "[retrieval]\nchunker = \"semantic\"\n");
        let e = if is_user { select_err(Some(&l), None) } else { select_err(None, Some(&l)) };
        match e {
            ConfigError::InvalidField { path: p, reason } => {
                assert_eq!(p, Path::new(path));
                assert!(reason.contains("retrieval.chunker") && reason.contains("semantic"), "{reason}");
                assert!(reason.contains("markdown") && reason.contains("fixed"), "valid names are listed: {reason}");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn test_r8_a_project_file_may_only_lower_top_k_and_max_tokens() {
    let u = layer(USER, "[retrieval]\ntop_k = 6\nmax_tokens = 3000\n");

    let s = select(Some(&u), Some(&layer(PROJECT, "[retrieval]\ntop_k = 2\nmax_tokens = 500\n")));
    assert_eq!((s.retrieval_top_k.value, s.retrieval_top_k.origin.clone()), (2, file(PROJECT)));
    assert_eq!((s.retrieval_max_tokens.value, s.retrieval_max_tokens.origin.clone()), (500, file(PROJECT)));
    assert!(warns(&s, PROJECT).is_empty());

    let s = select(Some(&u), Some(&layer(PROJECT, "[retrieval]\ntop_k = 12\nmax_tokens = 7000\n")));
    assert_eq!((s.retrieval_top_k.value, s.retrieval_top_k.origin.clone()), (6, file(USER)));
    assert_eq!((s.retrieval_max_tokens.value, s.retrieval_max_tokens.origin.clone()), (3000, file(USER)));
    let w = warns(&s, PROJECT);
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w.iter().any(|m| m.contains("top_k")) && w.iter().any(|m| m.contains("max_tokens")));
}

#[test]
fn test_r8_a_project_value_is_compared_with_the_default_when_the_user_sets_none() {
    let s = select(None, Some(&layer(PROJECT, "[retrieval]\ntop_k = 2\n")));
    assert_eq!((s.retrieval_top_k.value, s.retrieval_top_k.origin.clone()), (2, file(PROJECT)));

    let s = select(None, Some(&layer(PROJECT, "[retrieval]\ntop_k = 10\nmax_tokens = 5000\n")));
    assert_eq!((s.retrieval_top_k.value, s.retrieval_max_tokens.value), (3, 1200));
    assert_eq!(warns(&s, PROJECT).len(), 2);
}

// ---- R11 (review round 1): the fixed chunker is bounded -------------------------------------------------------------

#[test]
fn test_r11_fixed_lines_and_overlap_should_be_range_checked_naming_file_key_and_range() {
    for (raw, key, range) in [
        ("[retrieval.fixed]\nlines = 0\n", "lines", "400"),
        ("[retrieval.fixed]\nlines = 401\n", "lines", "400"),
        ("[retrieval.fixed]\nlines = 1000\noverlap = 1000\n", "lines", "400"),
        ("[retrieval.fixed]\nlines = 10\noverlap = 6\n", "overlap", "lines / 2"),
        ("[retrieval.fixed]\nlines = 1\noverlap = 1\n", "overlap", "lines / 2"),
        ("[retrieval.fixed]\noverlap = 21\n", "overlap", "lines / 2"), // default lines = 40 -> at most 20
    ] {
        match select_err(Some(&layer(USER, raw)), None) {
            ConfigError::InvalidField { path, reason } => {
                assert_eq!(path, Path::new(USER), "{raw}");
                assert!(reason.contains(key) && reason.contains(range), "{raw}: {reason}");
            }
            other => panic!("{raw}: {other:?}"),
        }
    }

    // The limits themselves are accepted.
    for (raw, lines, overlap) in [
        ("[retrieval.fixed]\nlines = 1\noverlap = 0\n", 1, 0),
        ("[retrieval.fixed]\nlines = 400\noverlap = 200\n", 400, 200),
        ("[retrieval.fixed]\nlines = 10\noverlap = 5\n", 10, 5),
        ("[retrieval.fixed]\noverlap = 20\n", 40, 20),
    ] {
        let s = select(Some(&layer(USER, raw)), None);
        assert_eq!((s.retrieval_fixed_lines.value, s.retrieval_fixed_overlap.value), (lines, overlap), "{raw}");
    }
}

#[test]
fn test_r11_a_project_file_cannot_set_the_fixed_chunker_parameters() {
    let u = layer(USER, "[retrieval.fixed]\nlines = 30\noverlap = 3\n");
    let p = layer(PROJECT, "[retrieval.fixed]\nlines = 1000\noverlap = 1000\n");

    let s = select(Some(&u), Some(&p));

    assert_eq!((s.retrieval_fixed_lines.value, s.retrieval_fixed_overlap.value), (30, 3), "the user's values win");
    assert_eq!(s.retrieval_fixed_lines.origin, file(USER));
    let w = warns(&s, PROJECT);
    assert!(w.iter().any(|m| m.contains("retrieval.fixed")), "a warning names the project file and the table: {w:?}");

    // Even a valid project value is ignored.
    let p = layer(PROJECT, "[retrieval.fixed]\nlines = 20\noverlap = 2\n");
    let s = select(None, Some(&p));
    assert_eq!((s.retrieval_fixed_lines.value, s.retrieval_fixed_overlap.value), (40, 5), "defaults stay");
    assert_eq!(s.retrieval_fixed_lines.origin, Origin::Default);
    assert!(!warns(&s, PROJECT).is_empty());
}
