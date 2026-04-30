//! Red-phase tests for R4: `format_memory_body` in `hone_core::memory_format`.
//!
//! These tests MUST fail to compile on `main` because `hone_core::memory_format`
//! and its public items (`StructuredBody`, `format_memory_body`) do not exist yet.
//!
//! Do not modify these tests during the green phase — they are the contract.

use hone_core::memory_format::{StructuredBody, format_memory_body};
use hone_memory::MemoryKind;

// ---------------------------------------------------------------------------
// Test 1
// ---------------------------------------------------------------------------

/// `format_memory_body(Feedback, all_fields)` must contain the rule text, a
/// `**Why:**` line, and a `**How to apply:**` line, each on its own line.
#[test]
fn feedback_body_has_all_three_sections() {
    let body = StructuredBody {
        rule: "use rg, not grep".to_string(),
        why: Some("faster".to_string()),
        how: Some("default to rg in this repo".to_string()),
    };

    let output = format_memory_body(MemoryKind::Feedback, body);

    assert!(
        output.contains("use rg, not grep"),
        "output must contain the rule; got: {output:?}"
    );
    assert!(
        output.contains("**Why:** faster"),
        "output must contain the Why section; got: {output:?}"
    );
    assert!(
        output.contains("**How to apply:** default to rg in this repo"),
        "output must contain the How to apply section; got: {output:?}"
    );

    // Sections must be separated by at least one newline.
    let rule_pos = output.find("use rg, not grep").unwrap();
    let why_pos = output.find("**Why:**").unwrap();
    let how_pos = output.find("**How to apply:**").unwrap();
    assert!(rule_pos < why_pos, "rule must come before Why section");
    assert!(why_pos < how_pos, "Why must come before How to apply section");
    let between = &output[rule_pos + "use rg, not grep".len()..why_pos];
    assert!(
        between.contains('\n'),
        "there must be a newline between rule and Why section; gap: {between:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2
// ---------------------------------------------------------------------------

/// When `why` is `None`, the output must not contain `**Why:**`.
/// When `how` is `None`, the output must not contain `**How to apply:**`.
#[test]
fn feedback_body_omits_missing_sections() {
    // Only why is None.
    let body_no_why = StructuredBody {
        rule: "prefer iterators".to_string(),
        why: None,
        how: Some("use .iter() instead of index loops".to_string()),
    };
    let out1 = format_memory_body(MemoryKind::Feedback, body_no_why);
    assert!(
        out1.contains("prefer iterators"),
        "rule must still appear; got: {out1:?}"
    );
    assert!(
        !out1.contains("**Why:**"),
        "**Why:** must be absent when why is None; got: {out1:?}"
    );
    assert!(
        out1.contains("**How to apply:**"),
        "**How to apply:** must still appear; got: {out1:?}"
    );

    // Only how is None.
    let body_no_how = StructuredBody {
        rule: "avoid unwrap in production".to_string(),
        why: Some("panics crash the process".to_string()),
        how: None,
    };
    let out2 = format_memory_body(MemoryKind::Feedback, body_no_how);
    assert!(
        out2.contains("**Why:** panics crash the process"),
        "Why section must appear; got: {out2:?}"
    );
    assert!(
        !out2.contains("**How to apply:**"),
        "**How to apply:** must be absent when how is None; got: {out2:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 3
// ---------------------------------------------------------------------------

/// `MemoryKind::Project` must also use the structured body format.
#[test]
fn project_kind_uses_structured_body() {
    let body = StructuredBody {
        rule: "freeze merges on Thursday".to_string(),
        why: Some("release train".to_string()),
        how: Some("check #ops before any merge".to_string()),
    };

    let output = format_memory_body(MemoryKind::Project, body);

    assert!(
        output.contains("freeze merges on Thursday"),
        "rule must appear; got: {output:?}"
    );
    assert!(
        output.contains("**Why:** release train"),
        "Why section must appear; got: {output:?}"
    );
    assert!(
        output.contains("**How to apply:** check #ops before any merge"),
        "How to apply section must appear; got: {output:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 4
// ---------------------------------------------------------------------------

/// `MemoryKind::Conversation` must return exactly `body.rule` — no added
/// structure, no extra newlines, no Markdown headers.
#[test]
fn conversation_kind_returns_rule_unchanged() {
    let rule = "Q: how do I list files? A: use ls or fd".to_string();
    let body = StructuredBody {
        rule: rule.clone(),
        why: None,
        how: None,
    };

    let output = format_memory_body(MemoryKind::Conversation, body);

    assert_eq!(
        output, rule,
        "Conversation kind must return rule unchanged; got: {output:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 5
// ---------------------------------------------------------------------------

/// `MemoryKind::User` must also return exactly `body.rule`.
#[test]
fn user_kind_returns_rule_unchanged() {
    let rule = "I am a data scientist working on logging pipelines".to_string();
    let body = StructuredBody {
        rule: rule.clone(),
        why: None,
        how: None,
    };

    let output = format_memory_body(MemoryKind::User, body);

    assert_eq!(
        output, rule,
        "User kind must return rule unchanged; got: {output:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 6
// ---------------------------------------------------------------------------

/// `MemoryKind::Reference` must also return exactly `body.rule`.
#[test]
fn reference_kind_returns_rule_unchanged() {
    let rule = "https://linear.app/proj/INGEST-42 — ingestion refactor epic".to_string();
    let body = StructuredBody {
        rule: rule.clone(),
        why: None,
        how: None,
    };

    let output = format_memory_body(MemoryKind::Reference, body);

    assert_eq!(
        output, rule,
        "Reference kind must return rule unchanged; got: {output:?}"
    );
}
