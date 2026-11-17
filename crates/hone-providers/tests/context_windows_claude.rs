#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Contract: Claude context windows in `ContextWindowTable` (spec section A).

use hone_proto::ModelId;
use hone_providers::ContextWindowTable;

const PREFIXES: [&str; 2] = ["anthropic", "claude-cli"];

const ONE_M: &[&str] = &[
    "claude-fable-5-2",
    "claude-mythos-5-2",
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-fable-5",
    "claude-mythos-5",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-5-5",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-haiku-5-5",
];

const TWO_HUNDRED_K: &[&str] = &["claude-opus-4-5", "claude-sonnet-4-5", "claude-haiku-4-5"];

fn window(table: &ContextWindowTable, id: &str) -> u32 {
    table.get(&ModelId::new(id))
}

// A1
#[test]
fn test_get_when_1m_claude_model_should_return_1m_under_both_prefixes() {
    let table = ContextWindowTable::default_table();
    for prefix in PREFIXES {
        for name in ONE_M {
            let id = format!("{prefix}:{name}");
            assert_eq!(window(&table, &id), 1_000_000, "model {id}");
        }
    }
}

// A1
#[test]
fn test_get_when_200k_claude_model_should_return_200k_under_both_prefixes() {
    let table = ContextWindowTable::default_table();
    for prefix in PREFIXES {
        for name in TWO_HUNDRED_K {
            let id = format!("{prefix}:{name}");
            assert_eq!(window(&table, &id), 200_000, "model {id}");
        }
    }
}

// A2
#[test]
fn test_get_when_dated_id_without_entry_should_use_base_entry() {
    let table = ContextWindowTable::default_table();
    let cases = [
        ("claude-haiku-4-5-20251001", 200_000),
        ("claude-opus-4-5-20251101", 200_000),
        ("claude-sonnet-4-5-20250929", 200_000),
        // Dashed date suffix (the `-YYYY-MM-DD` form), and a 1M base.
        ("claude-sonnet-4-5-2025-09-29", 200_000),
        ("claude-opus-4-7-20260115", 1_000_000),
    ];
    for prefix in PREFIXES {
        for (name, want) in cases {
            let id = format!("{prefix}:{name}");
            assert_eq!(window(&table, &id), want, "model {id}");
        }
    }
}

// A3
#[test]
fn test_get_when_exact_legacy_entry_exists_should_still_win() {
    let table = ContextWindowTable::default_table();
    let cases = [
        ("anthropic:claude-3-5-sonnet-20241022", 200_000),
        ("anthropic:claude-3-5-haiku-20241022", 200_000),
        ("claude-cli:claude-3-5-sonnet-20241022", 200_000),
        ("openai:gpt-4o", 128_000),
        ("openai:gpt-4", 8_192),
        ("gemini-cli:gemini-1.5-pro", 2_000_000),
        ("ollama:codellama", 16_384),
    ];
    for (id, want) in cases {
        assert_eq!(window(&table, id), want, "model {id}");
    }
}

// A3
#[test]
fn test_get_when_unknown_model_should_return_128k_default() {
    let table = ContextWindowTable::default_table();
    for id in ["unknown:model", "anthropic:claude-future-model"] {
        assert_eq!(window(&table, id), 128_000, "model {id}");
    }
}
