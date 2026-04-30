//! Red-phase tests for R5: `classify_turn` in `hone_core::memory_worthy`.
//!
//! These tests MUST fail to compile on `main` because `hone_core::memory_worthy`
//! and its public items (`MemoryCandidate`, `classify_turn`) do not exist yet.
//!
//! Do not modify these tests during the green phase — they are the contract.

use hone_core::memory_worthy::{MemoryCandidate, classify_turn};
use hone_memory::MemoryKind;

// ---------------------------------------------------------------------------
// Helper: assert a turn produces a specific MemoryKind.
// ---------------------------------------------------------------------------

#[track_caller]
fn assert_kind(user: &str, reply: &str, expected: MemoryKind) {
    let result = classify_turn(user, reply);
    let candidate = result.unwrap_or_else(|| {
        panic!(
            "expected Some({expected:?}) for input {user:?} but got None"
        )
    });
    assert_eq!(
        candidate.kind, expected,
        "kind mismatch for input {user:?}: expected {expected:?}, got {:?}",
        candidate.kind
    );
}

#[track_caller]
fn assert_none(user: &str, reply: &str) {
    let result = classify_turn(user, reply);
    assert!(
        result.is_none(),
        "expected None for input {user:?} but got Some({:?})",
        result.map(|c| c.kind)
    );
}

// ---------------------------------------------------------------------------
// Test 7 — correction patterns → Feedback
// ---------------------------------------------------------------------------

#[test]
fn correction_patterns_produce_feedback() {
    assert_kind("don't mock the database", "Understood.", MemoryKind::Feedback);
    assert_kind(
        "stop summarizing every response",
        "Got it, I'll be more concise.",
        MemoryKind::Feedback,
    );
    assert_kind(
        "no, not that — use the existing helper",
        "Of course, let me use the existing helper.",
        MemoryKind::Feedback,
    );
}

// ---------------------------------------------------------------------------
// Test 8 — validation patterns → Feedback
// ---------------------------------------------------------------------------

#[test]
fn validation_patterns_produce_feedback() {
    assert_kind(
        "yes, exactly that approach",
        "Great, I'll continue that way.",
        MemoryKind::Feedback,
    );
    assert_kind(
        "perfect, keep doing it that way",
        "Noted.",
        MemoryKind::Feedback,
    );
}

// ---------------------------------------------------------------------------
// Test 9 — external links → Reference
// ---------------------------------------------------------------------------

#[test]
fn external_links_produce_reference() {
    assert_kind(
        "see linear.app/proj/INGEST-42",
        "I'll take a look at the Linear ticket.",
        MemoryKind::Reference,
    );
    assert_kind(
        "the dashboard at grafana.internal/d/api",
        "Noted the Grafana dashboard URL.",
        MemoryKind::Reference,
    );
    assert_kind(
        "posted in #ops Slack",
        "I see it was discussed in Slack.",
        MemoryKind::Reference,
    );
}

// ---------------------------------------------------------------------------
// Test 10 — project state → Project
// ---------------------------------------------------------------------------

#[test]
fn project_state_produces_project() {
    assert_kind(
        "we're freezing merges Thursday",
        "Understood, no merges until Friday.",
        MemoryKind::Project,
    );
    assert_kind(
        "Q3 deadline for the auth rewrite",
        "Got it, Q3 target for auth.",
        MemoryKind::Project,
    );
}

// ---------------------------------------------------------------------------
// Test 11 — role declarations → User
// ---------------------------------------------------------------------------

#[test]
fn role_declarations_produce_user() {
    assert_kind(
        "I'm a data scientist working on logging",
        "Good to know.",
        MemoryKind::User,
    );
    assert_kind(
        "first time touching the React side here",
        "No problem, I'll add more context.",
        MemoryKind::User,
    );
}

// ---------------------------------------------------------------------------
// Test 12 — noisy turns return None
// ---------------------------------------------------------------------------

#[test]
fn noisy_turns_return_none() {
    assert_none("how do I list files in src/", "use ls or fd");
    assert_none("what is rust?", "rust is a systems programming language...");
    assert_none("", "");
}

// ---------------------------------------------------------------------------
// Test 12b — over-eager trigger negative cases (must all return None)
//
// Each row exercises a trigger that currently fires too broadly.  The
// assertion message names the trigger so the rust-expert knows which
// pattern to tighten:
//
//  • "stop " appearing anywhere in a sentence → fires Feedback (wrong)
//  • "don't " appearing anywhere            → fires Feedback (wrong)
//  • "release" as a plain verb              → fires Project  (wrong)
//  • "sprint" as a plain verb               → fires Project  (wrong)
//  • any github.com URL (not just /issues/) → fires Reference (wrong)
// ---------------------------------------------------------------------------

#[test]
fn over_eager_triggers_return_none_for_ordinary_phrasing() {
    // "stop" used in a temporal sense, not as a correction imperative.
    assert_none(
        "stop the migration before Friday",
        "ok will do",
        // If this fails: tighten the Feedback "stop " trigger so it only
        // matches when used as a direct imperative correction to the agent,
        // e.g. "stop doing X" or "stop summarizing", not "stop the <noun>".
    );

    // "don't" used as a reminder, not a correction to the agent.
    assert_none(
        "don't forget to read the README",
        "noted",
        // If this fails: tighten the Feedback "don't " trigger so it only
        // fires when the agent's behaviour is being corrected, not general
        // reminders like "don't forget to <do something>".
    );

    // "release" used as a noun / event, not a project-state declaration.
    assert_none(
        "we should release this PR today",
        "agreed",
        // If this fails: tighten the Project "release" trigger so it only
        // fires for explicit milestone / deadline statements, not casual
        // suggestions like "we should release X today".
    );

    // "sprint" used as a verb (ran quickly), not an Agile sprint reference.
    assert_none(
        "I sprinted through the auth code",
        "got it",
        // If this fails: tighten the Project "sprint" trigger so it only
        // fires for sprint names / sprint-planning context, not casual usage
        // of the verb "sprint".
    );

    // github.com URL that is NOT an issue or PR link.
    assert_none(
        "the github.com/foo/bar repo has the answer",
        "ok",
        // If this fails: tighten the Reference "github.com" trigger so it
        // only fires for github.com/*/issues/* or github.com/*/pull/*,
        // not bare repo URLs.
    );
}

// ---------------------------------------------------------------------------
// Test 13 — description is non-empty for typed kinds
// ---------------------------------------------------------------------------

/// For any trigger that produces a `MemoryCandidate`, `description` must be
/// `Some(s)` where `s` is non-empty.  The exact content is implementation-defined;
/// we only pin that it is not None and not blank.
#[test]
fn memory_candidate_description_is_non_empty_for_typed_kinds() {
    let cases: &[(&str, &str)] = &[
        ("don't mock the database", "Understood."),
        ("yes, exactly that approach", "Great."),
        ("see linear.app/proj/INGEST-42", "Noted."),
        ("Q3 deadline for the auth rewrite", "Got it."),
        ("I'm a data scientist working on logging", "Good to know."),
    ];

    for (user, reply) in cases {
        let candidate = classify_turn(user, reply)
            .unwrap_or_else(|| panic!("expected Some(_) for {user:?}"));

        match &candidate.description {
            Some(desc) => {
                assert!(
                    !desc.trim().is_empty(),
                    "description must not be blank for kind {:?} (input: {user:?}); got: {desc:?}",
                    candidate.kind
                );
            }
            None => {
                panic!(
                    "description must be Some(_) for kind {:?} (input: {user:?})",
                    candidate.kind
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test 14 — body.rule is non-empty for typed kinds
// ---------------------------------------------------------------------------

/// For any trigger that produces a `MemoryCandidate`, `body.rule` must be a
/// non-empty string.  We do not pin exact contents — just non-empty.
#[test]
fn memory_candidate_body_rule_is_non_empty_for_typed_kinds() {
    let cases: &[(&str, &str)] = &[
        ("don't mock the database", "Understood."),
        ("yes, exactly that approach", "Great."),
        ("see linear.app/proj/INGEST-42", "Noted."),
        ("Q3 deadline for the auth rewrite", "Got it."),
        ("I'm a data scientist working on logging", "Good to know."),
    ];

    for (user, reply) in cases {
        let candidate = classify_turn(user, reply)
            .unwrap_or_else(|| panic!("expected Some(_) for {user:?}"));

        assert!(
            !candidate.body.rule.trim().is_empty(),
            "body.rule must not be blank for kind {:?} (input: {user:?})",
            candidate.kind
        );
    }
}
