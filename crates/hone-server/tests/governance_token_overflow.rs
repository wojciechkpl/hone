// AUDIT-P1 #7 — governance.rs:86 and :95 use bare `+=` on u64 token counters,
// which wraps silently in release mode and panics in debug mode on overflow.
// These tests prove the bug exists (red phase) and will pass once both sites
// are changed to `saturating_add`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use hone_server::AppState;

// ── helper ────────────────────────────────────────────────────────────────────

fn make_state() -> AppState {
    AppState::new(None)
}

// ── 1. total_tokens saturates at u64::MAX ────────────────────────────────────

#[tokio::test]
async fn add_usage_saturates_at_u64_max_for_total_tokens() {
    // Given – total_tokens is just below u64::MAX
    let state = make_state();
    state
        .governance
        .add_usage("claude-sonnet-4-5", u64::MAX - 100, 0.0)
        .await;

    // When – add another 1 000 tokens (would wrap without saturating_add)
    state
        .governance
        .add_usage("claude-sonnet-4-5", 1_000, 0.0)
        .await;

    // Then – must saturate, not wrap or panic
    let usage = state.governance.get_usage().await;
    assert_eq!(
        usage.total_tokens,
        u64::MAX,
        "total_tokens must saturate at u64::MAX, not wrap"
    );
}

// ── 2. per-entry tokens saturates at u64::MAX ────────────────────────────────

#[tokio::test]
async fn add_usage_saturates_at_u64_max_for_per_entry_tokens() {
    // Given – seed the model entry close to u64::MAX
    let state = make_state();
    let model = "claude-opus-4-5";
    state.governance.add_usage(model, u64::MAX - 100, 0.0).await;

    // When – overflow the per-entry counter
    state.governance.add_usage(model, 1_000, 0.0).await;

    // Then – per-entry tokens must saturate, not wrap or panic
    let usage = state.governance.get_usage().await;
    let entry_tokens = usage
        .by_model
        .get(model)
        .expect("model entry must exist")
        .tokens;
    assert_eq!(
        entry_tokens,
        u64::MAX,
        "entry.tokens must saturate at u64::MAX, not wrap"
    );
}

// ── 3. no panic in debug builds on overflow ───────────────────────────────────

#[tokio::test]
async fn add_usage_does_not_panic_on_overflow_in_debug_build() {
    // In a debug build the bare `+=` at governance.rs:86/:95 raises
    // "attempt to add with overflow" — if this test completes without
    // panicking the fix is in place.
    let state = make_state();
    state
        .governance
        .add_usage("claude-haiku-3-5", u64::MAX, 0.0)
        .await;
    // A second call would overflow both total_tokens and entry.tokens.
    state.governance.add_usage("claude-haiku-3-5", 1, 0.0).await;
    // Reaching this line is the assertion.
}
