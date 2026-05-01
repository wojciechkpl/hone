//! Red-phase tests for R7 Phase B: memory-index injection into the system prompt.
//!
//! Every test in this file MUST fail to compile on `main` because
//! `build_system_prompt` currently has the signature:
//!
//!   `pub fn build_system_prompt(hone_md_sections: &[(String, String)]) -> String`
//!
//! Phase B requires a second parameter:
//!
//!   `pub fn build_system_prompt(
//!       hone_md_sections: &[(String, String)],
//!       memory_index: Option<&str>,
//!   ) -> String`
//!
//! # Contract pinned by these tests
//!
//! - `memory_index: Some(non_empty_str)` → prompt contains `## Memory index`
//!   section and the rendered string verbatim.
//! - `memory_index: Some("")` (store exists but empty) → prompt contains
//!   `## Memory index` and the literal text `(no memories)`.
//! - `memory_index: None` (no store at all) → prompt does NOT contain
//!   `## Memory index`.
//! - When the section is present the body must include the literal token
//!   `memory_read` and text instructing the agent to call it with the id.
//!
//! The caller renders the index via `hone_core::memory_index::render_index` and
//! passes the result in; `build_system_prompt` is synchronous and I/O-free.
//!
//! # Linking strategy
//!
//! `bins/hone` has no `[lib]` target, so the module is pulled in with `#[path]`
//! mirroring the pattern used by `system_prompt_test.rs`.

#[path = "../src/system_prompt.rs"]
mod system_prompt;

use system_prompt::build_system_prompt;

// ---------------------------------------------------------------------------
// Test 8 — non-empty rendered index is embedded under the section header
// ---------------------------------------------------------------------------

#[test]
fn system_prompt_includes_memory_index_section_with_content() {
    let rendered = "### User\n- [user-1] prefer 4-space indents (kind=user, w=1)";
    let prompt = build_system_prompt(&[], Some(rendered));

    assert!(
        prompt.contains("## Memory index"),
        "prompt must contain '## Memory index' heading when a non-empty index is passed; \
         got prompt length {} bytes",
        prompt.len()
    );
    assert!(
        prompt.contains(rendered),
        "prompt must embed the rendered index verbatim under '## Memory index'"
    );
}

// ---------------------------------------------------------------------------
// Test 9 — empty rendered string produces "(no memories)"
// ---------------------------------------------------------------------------

#[test]
fn system_prompt_includes_no_memories_when_empty() {
    let prompt = build_system_prompt(&[], Some(""));

    assert!(
        prompt.contains("## Memory index"),
        "prompt must contain '## Memory index' even when the rendered index is empty; \
         the section header must always appear when a store is available"
    );
    assert!(
        prompt.contains("(no memories)"),
        "prompt must contain '(no memories)' as the body when the rendered index is empty"
    );
}

// ---------------------------------------------------------------------------
// Test 10 — None means no store at all → section is absent
// ---------------------------------------------------------------------------

#[test]
fn system_prompt_omits_memory_index_section_when_none_passed() {
    let prompt = build_system_prompt(&[], None);

    assert!(
        !prompt.contains("## Memory index"),
        "prompt must NOT contain '## Memory index' when memory_index is None \
         (distinguishes 'no store' from 'store exists but empty')"
    );
}

// ---------------------------------------------------------------------------
// Test 11 — the section body contains "memory_read" and call-with-id guidance
// ---------------------------------------------------------------------------

#[test]
fn system_prompt_mentions_memory_read_tool_in_section() {
    let rendered = "### Feedback\n- [fb-1] use read_file before edit_file (kind=feedback, w=1.5)";
    let prompt = build_system_prompt(&[], Some(rendered));

    assert!(
        prompt.contains("memory_read"),
        "the '## Memory index' section must mention the 'memory_read' tool by name so \
         the agent knows how to fetch full bodies"
    );
    // Verify the guidance explains *how* to call it (with the id).
    // We pin on a flexible phrase rather than the exact sentence so minor
    // wording changes do not break the test.
    let section_start = prompt
        .find("## Memory index")
        .expect("section header not found");
    let section_body = &prompt[section_start..];
    assert!(
        section_body.contains("id"),
        "the '## Memory index' section must include the word 'id' to guide the agent \
         on what to pass to memory_read"
    );
}
