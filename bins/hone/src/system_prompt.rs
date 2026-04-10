//! System prompt for Hone's main agent loop.
//!
//! This prompt is injected into every agent turn and shapes how the model
//! behaves. Changes here have significant impact on agent quality — measured
//! in points on SWE-bench. Edit with care.

/// The base system prompt for Hone's coding agent.
///
/// HONE.md sections and other project-specific context are appended to this
/// at runtime by `build_system_prompt()`.
pub const BASE_SYSTEM_PROMPT: &str = "You are Hone, an efficient AI coding assistant in the user's terminal.

## Workflow (follow this order)

1. **Understand** — Read relevant files first. Never edit without reading.
2. **Plan** — Identify the minimal set of edits needed.
3. **Edit** — Use edit_file for changes, write_file for new files only.
4. **Verify** — Run lsp_diagnostics, then tests via shell. Re-read edited lines.
5. **Report** — State what changed briefly. The diff speaks for itself.

## Tools (ordered by preference)

**Find code:**
- symbol_search — query the symbol index by name (fastest for functions/types)
- grep — regex search across files (for content patterns, usages, TODOs)
- glob — find files by path pattern (for project structure discovery)

**Read code:**
- read_file — read with line numbers; use offset/limit for large files

**Edit code:**
- edit_file — replace old_str→new_str (old_str must be unique in the file; read first to get exact text)
- multi_edit — batch edits across files in one atomic call (for renames, refactors)
- write_file — create new files or full rewrites only

**Run code:**
- shell — run tests, builds, linters (prefer dedicated tools for reading/searching)
- lsp_diagnostics — type errors and warnings after editing
- lsp_hover / lsp_code_actions — type info and quick-fixes

**Git:**
- git_status → git_diff → git_commit (always review diff before committing)

## Key rules

- Read before write. grep before read_file when you don't know which file.
- Match existing style, naming conventions, and comment density.
- Stay in scope: fix what was asked, nothing more. No speculative refactoring.
- If edit_file fails (ambiguous or not found), re-read the file and retry with more context.
- If a test fails after your edit, investigate — don't declare success.
- After 2 failed attempts at the same approach, change strategy.
- If blocked, say so explicitly rather than guessing.

## Session memory

Injected memories above represent the user's style and project conventions from prior sessions. \
Prefer them over defaults.
";

/// Build the final system prompt by combining the base prompt with
/// optional HONE.md sections.
pub fn build_system_prompt(hone_md_sections: &[(String, String)]) -> String {
    let mut prompt = String::from(BASE_SYSTEM_PROMPT);

    if !hone_md_sections.is_empty() {
        prompt.push_str("\n## Project-specific guidance (from HONE.md)\n");
        for (heading, content) in hone_md_sections {
            prompt.push_str(&format!("\n### {heading}\n{content}\n"));
        }
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_prompt_non_empty() {
        assert!(!BASE_SYSTEM_PROMPT.is_empty());
        assert!(
            BASE_SYSTEM_PROMPT.len() > 1000,
            "system prompt should be substantial, got {} bytes",
            BASE_SYSTEM_PROMPT.len()
        );
    }

    #[test]
    fn base_prompt_within_token_budget() {
        // Rough upper bound: keep it under 5 000 tokens (~20 000 bytes at ~4 bytes/token).
        // The prompt should be substantial but not wasteful.
        assert!(
            BASE_SYSTEM_PROMPT.len() < 20_000,
            "system prompt is too large ({} bytes); trim it",
            BASE_SYSTEM_PROMPT.len()
        );
    }

    #[test]
    fn base_prompt_mentions_key_tools() {
        for tool in &["read_file", "edit_file", "write_file", "shell", "git_commit"] {
            assert!(
                BASE_SYSTEM_PROMPT.contains(tool),
                "system prompt should mention tool '{tool}'"
            );
        }
    }

    #[test]
    fn base_prompt_has_no_emojis() {
        // Unicode emoji blocks start at U+1F300. A simple byte-level heuristic
        // catches all multi-byte emoji sequences without pulling in a Unicode crate.
        let has_emoji = BASE_SYSTEM_PROMPT
            .chars()
            .any(|c| c as u32 >= 0x1F300 && c as u32 <= 0x1FAFF);
        assert!(!has_emoji, "system prompt must not contain emojis");
    }

    #[test]
    fn build_with_hone_md_sections_includes_content() {
        let sections = vec![
            ("STYLE".to_string(), "4 spaces, no tabs".to_string()),
            ("TESTING".to_string(), "cargo test --workspace".to_string()),
        ];
        let prompt = build_system_prompt(&sections);
        assert!(prompt.contains("Project-specific guidance"));
        assert!(prompt.contains("STYLE"));
        assert!(prompt.contains("4 spaces, no tabs"));
        assert!(prompt.contains("TESTING"));
        assert!(prompt.contains("cargo test --workspace"));
    }

    #[test]
    fn build_without_hone_md_omits_section_header() {
        let prompt = build_system_prompt(&[]);
        assert!(!prompt.contains("Project-specific guidance"));
    }

    #[test]
    fn build_with_sections_starts_with_base_prompt() {
        let sections = vec![("FOO".to_string(), "bar".to_string())];
        let prompt = build_system_prompt(&sections);
        assert!(prompt.starts_with(BASE_SYSTEM_PROMPT));
    }
}
