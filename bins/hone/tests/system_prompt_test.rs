//! Red-phase tests 7, 8, 9 — assert that `build_system_prompt()` contains
//! the R2 "What NOT to remember" and R3 "Verify before recommending" guardrail
//! sections added in PR1.
//!
//! Tests 7, 8, and 9 MUST FAIL on main because the sections do not exist yet
//! in `BASE_SYSTEM_PROMPT`.
//!
//! # Linking strategy
//!
//! `bins/hone` has no `[lib]` target, so we pull in the module directly with
//! `#[path]`. Only the `system_prompt` module is needed here and it has no
//! internal dependencies beyond `std`, so this is safe.

#[path = "../src/system_prompt.rs"]
mod system_prompt;

use system_prompt::{BASE_SYSTEM_PROMPT, build_system_prompt};

// ---------------------------------------------------------------------------
// Test 7 — prompt contains "What NOT to remember" section heading
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_contains_what_not_to_remember_section() {
    // Assert on the base constant so the test catches any build_* wrapper too.
    assert!(
        BASE_SYSTEM_PROMPT.contains("What NOT to remember"),
        "BASE_SYSTEM_PROMPT must contain the 'What NOT to remember' section (R2). \
         Add the section to bins/hone/src/system_prompt.rs."
    );

    // Also verify it survives through build_system_prompt().
    let built = build_system_prompt(&[]);
    assert!(
        built.contains("What NOT to remember"),
        "build_system_prompt() output must also contain 'What NOT to remember'"
    );
}

// ---------------------------------------------------------------------------
// Test 8 — prompt contains "Verify before recommending from memory" section
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_contains_verify_before_recommending_section() {
    assert!(
        BASE_SYSTEM_PROMPT.contains("Verify before recommending from memory"),
        "BASE_SYSTEM_PROMPT must contain the 'Verify before recommending from memory' \
         section (R3). Add the section to bins/hone/src/system_prompt.rs."
    );

    let built = build_system_prompt(&[]);
    assert!(
        built.contains("Verify before recommending from memory"),
        "build_system_prompt() output must also contain 'Verify before recommending from memory'"
    );
}

// ---------------------------------------------------------------------------
// Test 9 — prompt contains the "ask back" pattern for the PR-list case
//
// The plan says: "If the user asks to 'remember this PR list,' ask back:
// 'what was *surprising* or *non-obvious*?'"
// We assert on a stable phrase from that sentence.
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_contains_ask_back_for_surprising_pattern() {
    // Stable phrase from the plan's "ask back" guidance.
    let stable_phrase = "ask back";
    assert!(
        BASE_SYSTEM_PROMPT.contains(stable_phrase),
        "BASE_SYSTEM_PROMPT must contain '{stable_phrase}' — the guardrail that tells \
         the model to ask back for the *surprising* or *non-obvious* part instead of \
         blindly recording PR lists or other ephemeral state. \
         Add the 'What NOT to remember' section to bins/hone/src/system_prompt.rs."
    );
}

#[test]
fn test_prompt_contains_do_not_record_ephemeral_state() {
    // Another stable phrase from the "What NOT to remember" section.
    let phrase = "Ephemeral state";
    assert!(
        BASE_SYSTEM_PROMPT.contains(phrase),
        "BASE_SYSTEM_PROMPT must contain '{phrase}' from the exclusion list in the \
         'What NOT to remember' section (R2)."
    );
}

// ---------------------------------------------------------------------------
// Sanity guard — existing token-budget test still holds with the new sections
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_with_new_sections_still_within_token_budget() {
    // The plan says the length increase must be <= ~600 chars.
    // This test asserts the overall budget stays under 20 000 bytes.
    // It will pass today (prompt is short) and must also pass after Phase 2.
    let built = build_system_prompt(&[]);
    assert!(
        built.len() < 20_000,
        "system prompt is too large ({} bytes); trim it",
        built.len()
    );
}
