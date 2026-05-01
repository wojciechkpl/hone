//! Red-phase tests for `friendly_memory_key` in `hone_core::memory_key`.
//!
//! All tests here MUST fail to compile today because `hone_core::memory_key`
//! and `hone_core::friendly_memory_key` do not exist yet.
//!
//! Do not modify these tests during the green phase — they are the contract.

use hone_core::friendly_memory_key;
use hone_memory::{Memory, MemoryKind};

// ---------------------------------------------------------------------------
// Helper: build a Memory with sane defaults, overriding only the fields that
// matter for each test.
// ---------------------------------------------------------------------------

fn make_memory(
    id: &str,
    kind: MemoryKind,
    created_at: u64,
    description: Option<&str>,
) -> Memory {
    Memory {
        id: id.to_owned(),
        content: "some content".to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: description.map(|s| s.to_owned()),
    }
}

// ---------------------------------------------------------------------------
// Test 1 — description present and non-blank: returns description trimmed
// ---------------------------------------------------------------------------

/// When `memory.description` is `Some("  Storage split  ")`, the function
/// must return `"Storage split"` (leading/trailing whitespace stripped).
#[test]
fn description_present_returns_description_trimmed() {
    let memory = make_memory(
        "any-id",
        MemoryKind::Conversation,
        0,
        Some("  Storage split  "),
    );

    let key = friendly_memory_key(&memory);

    assert_eq!(
        key, "Storage split",
        "expected trimmed description; got: {key:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — description is an empty string: falls back to synthetic key
// ---------------------------------------------------------------------------

/// When `description` is `Some("")`, the function must fall back to the
/// `kind-HHMM-XX` synthetic format.
#[test]
fn description_empty_string_falls_back_to_synthetic() {
    let memory = make_memory("fixed-id-001", MemoryKind::User, 3661, Some(""));

    let key = friendly_memory_key(&memory);

    // Must NOT return the empty string.
    assert!(!key.is_empty(), "key must not be empty");

    // Must match `^<kind>-\d{4}-[0-9a-f]{2}$`.
    let re = regex::Regex::new(r"^[a-z]+-\d{4}-[0-9a-f]{2}$").unwrap();
    assert!(
        re.is_match(&key),
        "key must match synthetic pattern ^<kind>-HHMM-XX$; got: {key:?}"
    );

    // The kind label must start the key.
    assert!(
        key.starts_with("user-"),
        "key for MemoryKind::User must start with 'user-'; got: {key:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — description is whitespace-only: falls back to synthetic key
// ---------------------------------------------------------------------------

/// When `description` is `Some("   \t\n  ")`, the function must treat it as
/// absent and return the synthetic format.
#[test]
fn description_whitespace_only_falls_back_to_synthetic() {
    let memory = make_memory("fixed-id-002", MemoryKind::Project, 7200, Some("   \t\n  "));

    let key = friendly_memory_key(&memory);

    assert!(!key.is_empty(), "key must not be empty");

    let re = regex::Regex::new(r"^[a-z]+-\d{4}-[0-9a-f]{2}$").unwrap();
    assert!(
        re.is_match(&key),
        "key must match synthetic pattern; got: {key:?}"
    );

    assert!(
        key.starts_with("project-"),
        "key for MemoryKind::Project must start with 'project-'; got: {key:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — no description: synthetic key with correct kind label and HHMM
// ---------------------------------------------------------------------------

/// `Memory { kind: Feedback, created_at: 1714478400 (2024-04-30 12:00:00 UTC),
/// id: "fixed-id-aaa", description: None }` must yield a key that:
///   - starts with `"feedback-1200-"`
///   - ends with exactly two lowercase hex characters.
#[test]
fn no_description_uses_synthetic_with_correct_kind_label() {
    // 2024-04-30 12:00:00 UTC == 1714478400
    // secs_of_day = 1714478400 % 86400 = 43200
    // hh = 43200 / 3600 = 12, mm = (43200 % 3600) / 60 = 0  → HHMM = "1200"
    let memory = make_memory("fixed-id-aaa", MemoryKind::Feedback, 1714478400, None);

    let key = friendly_memory_key(&memory);

    assert!(
        key.starts_with("feedback-1200-"),
        "key must start with 'feedback-1200-'; got: {key:?}"
    );

    // Suffix must be two lowercase hex chars.
    let suffix = key.trim_start_matches("feedback-1200-");
    assert_eq!(
        suffix.len(),
        2,
        "hex suffix must be exactly 2 chars; got: {suffix:?}"
    );
    assert!(
        suffix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
        "hex suffix must be lowercase hex; got: {suffix:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — synthetic key is deterministic across two calls
// ---------------------------------------------------------------------------

/// The same `(kind, created_at, id)` must yield byte-identical output every
/// time — no randomness.
#[test]
fn synthetic_key_is_deterministic() {
    let memory = make_memory("stable-id-xyz", MemoryKind::Reference, 54321, None);

    let first = friendly_memory_key(&memory);
    let second = friendly_memory_key(&memory);

    assert_eq!(
        first, second,
        "friendly_memory_key must be deterministic; got {first:?} vs {second:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 6 — HHMM derived correctly from UTC seconds
// ---------------------------------------------------------------------------

/// Boundary checks for the HHMM segment:
///   - `created_at = 0`     → `"<kind>-0000-XX"` (midnight)
///   - `created_at = 86399` → `"<kind>-2359-XX"` (23:59:59)
///   - `created_at = 86400` → `"<kind>-0000-XX"` (wraps to next day midnight)
#[test]
fn synthetic_key_hh_mm_format_uses_utc_seconds() {
    let kind = MemoryKind::Conversation;
    let id = "time-test-id";

    let key_midnight = friendly_memory_key(&make_memory(id, kind, 0, None));
    assert!(
        key_midnight.starts_with("conversation-0000-"),
        "created_at=0 must yield HHMM=0000; got: {key_midnight:?}"
    );

    let key_2359 = friendly_memory_key(&make_memory(id, kind, 86399, None));
    assert!(
        key_2359.starts_with("conversation-2359-"),
        "created_at=86399 must yield HHMM=2359; got: {key_2359:?}"
    );

    let key_wrap = friendly_memory_key(&make_memory(id, kind, 86400, None));
    assert!(
        key_wrap.starts_with("conversation-0000-"),
        "created_at=86400 must wrap to HHMM=0000; got: {key_wrap:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 7 — hex suffix changes when id changes
// ---------------------------------------------------------------------------

/// Two memories with the same kind and created_at but different ids must
/// produce different hex suffixes (the hash is over the id string).
/// Ids "id-alpha-000" and "id-beta-999" are chosen to be distinct enough
/// that a 1-byte FNV-1a hash reliably produces different outputs.
#[test]
fn synthetic_key_hex_suffix_changes_with_id() {
    let kind = MemoryKind::Outcome;
    let created_at = 3600u64;

    // Use strings that are far apart in the FNV-1a space.
    let key_a = friendly_memory_key(&make_memory("id-alpha-000", kind, created_at, None));
    let key_b = friendly_memory_key(&make_memory("id-beta-999", kind, created_at, None));

    // Both must be well-formed.
    let re = regex::Regex::new(r"^outcome-\d{4}-[0-9a-f]{2}$").unwrap();
    assert!(re.is_match(&key_a), "key_a malformed: {key_a:?}");
    assert!(re.is_match(&key_b), "key_b malformed: {key_b:?}");

    // The hex suffixes must differ.
    let suffix_a: &str = key_a.split('-').nth(2).unwrap();
    let suffix_b: &str = key_b.split('-').nth(2).unwrap();
    assert_ne!(
        suffix_a, suffix_b,
        "different ids must produce different hex suffixes; \
         key_a={key_a:?}, key_b={key_b:?}. \
         If these two specific ids collide, choose a different pair."
    );
}

// ---------------------------------------------------------------------------
// Test 8 — kind_label covers all MemoryKind variants
// ---------------------------------------------------------------------------

/// Smoke test: one memory per `MemoryKind` variant (no description) must
/// produce a key whose prefix matches the expected lowercase label.
#[test]
fn kind_label_covers_all_variants() {
    let cases: &[(MemoryKind, &str)] = &[
        (MemoryKind::Conversation, "conversation"),
        (MemoryKind::User, "user"),
        (MemoryKind::Feedback, "feedback"),
        (MemoryKind::Project, "project"),
        (MemoryKind::Reference, "reference"),
        (MemoryKind::Outcome, "outcome"),
    ];

    for (kind, expected_prefix) in cases {
        let memory = make_memory("smoke-id", *kind, 0, None);
        let key = friendly_memory_key(&memory);

        assert!(
            key.starts_with(&format!("{expected_prefix}-")),
            "MemoryKind::{kind:?} must produce key starting with '{expected_prefix}-'; got: {key:?}"
        );
    }
}
