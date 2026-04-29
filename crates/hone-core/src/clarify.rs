/// Scope Clarification Mode — detects ambiguous requests and surfaces
/// clarifying questions before the agent begins executing.
///
/// Design goals:
/// - Zero false positives on clear requests (file path, function name, or
///   unambiguous imperative like "run cargo test").
/// - Heuristics run in O(n) over the request string — no allocation hot-paths.
pub struct ClarificationDetector;

impl ClarificationDetector {
    /// Check whether `request` is ambiguous.
    ///
    /// Returns `None` when the request is specific enough to proceed immediately.
    /// Returns `Some(questions)` when clarification would reduce rework.
    pub fn needs_clarification(request: &str) -> Option<Vec<String>> {
        let trimmed = request.trim();

        // Empty or trivially short strings are handled by the caller; skip.
        if trimmed.is_empty() {
            return None;
        }

        // Simple commands (single words, "hello", "help", etc.) need no
        // clarification — asking questions would be more annoying than useful.
        if is_simple_command(trimmed) {
            return None;
        }

        // Specific targets (file paths, extensions, function names) are always
        // clear enough to proceed without asking.
        if mentions_specific_target(trimmed) {
            return None;
        }

        let mut questions: Vec<String> = Vec::new();

        // Heuristic 1: Very short requests without a known simple command are
        // almost always under-specified.
        if trimmed.len() < 20 {
            questions.push("Could you provide more details about what you'd like me to do?".into());
        }

        // Heuristic 2: Ambiguous action verbs with no concrete object.
        if contains_ambiguous_terms(trimmed) {
            let action = extract_action(trimmed);
            questions.push(format!("What specifically should I {action}?"));
        }

        // Heuristic 3: Code-change request with no file or function target.
        if is_code_change_request(trimmed) && !mentions_specific_target(trimmed) {
            questions.push("Which file(s) or function(s) should I focus on?".into());
        }

        // Heuristic 4: Multiple concerns in a single request — scope is unclear.
        if mentions_multiple_concerns(trimmed) {
            questions.push(
                "Should I address all of these in one change, or would you like \
                 them handled separately?"
                    .into(),
            );
        }

        // Deduplicate while preserving order — rare in practice but keeps the
        // output clean when multiple heuristics fire on the same request.
        questions.dedup();

        if questions.is_empty() {
            None
        } else {
            Some(questions)
        }
    }

    /// Format a list of clarifying questions into a message ready for display.
    pub fn format_questions(questions: &[String]) -> String {
        let mut out = String::from("Before proceeding, I have a few clarifying questions:\n");
        for (i, q) in questions.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, q));
        }
        out.push_str("\nPlease answer these so I can help you more effectively.");
        out
    }
}

// ---------------------------------------------------------------------------
// Detection helpers
// ---------------------------------------------------------------------------

/// Returns `true` for requests that are clearly single-purpose commands that
/// need no clarification (e.g. "hello", "help", "/cost").
fn is_simple_command(input: &str) -> bool {
    // Single words are always unambiguous.
    if !input.contains(char::is_whitespace) {
        return true;
    }

    let lower = input.to_lowercase();

    // Well-known greetings and meta-queries.
    const SIMPLE: &[&str] = &[
        "hello",
        "hi",
        "hey",
        "help",
        "quit",
        "exit",
        "bye",
        "what can you do",
        "what are you",
        "who are you",
    ];
    SIMPLE.iter().any(|s| lower == *s || lower.starts_with(s))
}

/// Returns `true` when the request contains vague action terms without a
/// concrete target — the most reliable signal for ambiguity.
fn contains_ambiguous_terms(input: &str) -> bool {
    let lower = input.to_lowercase();

    // "fix it", "update it", "make it better" — pronoun with no antecedent.
    let pronoun_patterns = [
        "fix it",
        "update it",
        "change it",
        "improve it",
        "make it",
        "do it",
        "do that",
        "make that",
    ];
    if pronoun_patterns.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // Lone vague verbs with very few words following (< 3 tokens after the verb).
    const VAGUE_VERBS: &[&str] = &[
        "fix",
        "update",
        "change",
        "improve",
        "make better",
        "clean up",
        "cleanup",
        "optimise",
        "optimize",
    ];

    for verb in VAGUE_VERBS {
        if let Some(pos) = lower.find(verb) {
            let rest = &input[pos + verb.len()..].trim_start();
            let word_count = rest.split_whitespace().count();
            // If the verb is followed by 0-1 words and no specific target, flag it.
            if word_count <= 1 && !mentions_specific_target(rest) {
                return true;
            }
        }
    }

    false
}

/// Returns `true` for requests that ask to modify code (as opposed to questions
/// or read-only tasks).
fn is_code_change_request(input: &str) -> bool {
    let lower = input.to_lowercase();
    const CHANGE_VERBS: &[&str] = &[
        "fix",
        "add",
        "remove",
        "delete",
        "refactor",
        "implement",
        "update",
        "rewrite",
        "edit",
        "create",
        "write",
        "change",
        "rename",
        "move",
        "extract",
    ];
    CHANGE_VERBS.iter().any(|v| {
        // Match as a whole word so "fixed" or "prefix" don't trigger.
        lower
            .split_whitespace()
            .any(|w| w == *v || w.trim_matches(|c: char| !c.is_alphanumeric()) == *v)
    })
}

/// Returns `true` when the request names a concrete file, path, or function.
///
/// Presence of any of these signals means the user has already scoped the
/// work — no clarification needed.
fn mentions_specific_target(input: &str) -> bool {
    // File extension: .rs, .py, .js, .ts, .go, .toml, .yaml, .json …
    let has_extension = input.split_whitespace().any(|word| {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '/');
        w.contains('.')
            && w.rsplit_once('.')
                .map(|(_, ext)| {
                    matches!(
                        ext.to_lowercase().as_str(),
                        "rs" | "py"
                            | "js"
                            | "ts"
                            | "go"
                            | "toml"
                            | "yaml"
                            | "yml"
                            | "json"
                            | "md"
                            | "html"
                            | "css"
                            | "sh"
                            | "c"
                            | "cpp"
                            | "h"
                            | "java"
                            | "kt"
                            | "rb"
                            | "ex"
                            | "exs"
                            | "lua"
                            | "zig"
                            | "swift"
                            | "cs"
                    )
                })
                .unwrap_or(false)
    });

    if has_extension {
        return true;
    }

    // Path separator indicates a file or module path.
    if input.contains('/') || input.contains('\\') {
        return true;
    }

    // `fn <name>` or `function <name>` patterns.
    let lower = input.to_lowercase();
    if lower.contains("fn ") || lower.contains("function ") || lower.contains("method ") {
        return true;
    }

    // Backtick-quoted identifiers are almost always specific enough.
    if input.contains('`') {
        return true;
    }

    false
}

/// Returns `true` when the request spans multiple distinct concerns.
fn mentions_multiple_concerns(input: &str) -> bool {
    let lower = input.to_lowercase();

    // Explicit multi-part connectors.
    const MULTI_MARKERS: &[&str] = &[
        "and also",
        "plus also",
        "additionally",
        "on top of that",
        "as well as",
        "in addition",
    ];
    if MULTI_MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }

    // Multiple sentences each starting with a different imperative verb.
    // Count sentences that begin with a code-change verb.
    let imperative_sentence_count = input
        .split(|c| c == '.' || c == ';' || c == '\n')
        .filter(|s| {
            let s = s.trim();
            if s.is_empty() {
                return false;
            }
            is_code_change_request(s)
        })
        .count();

    imperative_sentence_count >= 2
}

/// Extract the primary action verb from a request for use in a clarifying
/// question.  Falls back to "do that" when no verb can be identified.
fn extract_action(input: &str) -> &'static str {
    let lower = input.to_lowercase();
    const VERBS: &[&str] = &[
        "fix",
        "update",
        "change",
        "improve",
        "refactor",
        "implement",
        "add",
        "remove",
        "rewrite",
        "create",
        "write",
        "edit",
    ];
    for verb in VERBS {
        if lower.split_whitespace().any(|w| w == *verb) {
            // Leak is acceptable here — these are all 'static string literals.
            return verb;
        }
    }
    "do that"
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_simple_command ---------------------------------------------------

    #[test]
    fn single_word_is_simple() {
        assert!(is_simple_command("hello"));
        assert!(is_simple_command("help"));
        assert!(is_simple_command("quit"));
    }

    #[test]
    fn multi_word_help_is_simple() {
        assert!(is_simple_command("what can you do"));
    }

    #[test]
    fn real_request_is_not_simple() {
        assert!(!is_simple_command("fix the login bug"));
    }

    // --- mentions_specific_target -------------------------------------------

    #[test]
    fn file_extension_is_specific() {
        assert!(mentions_specific_target("fix auth.rs"));
        assert!(mentions_specific_target("update src/main.py"));
        assert!(mentions_specific_target("edit config.toml"));
    }

    #[test]
    fn path_separator_is_specific() {
        assert!(mentions_specific_target("refactor src/auth/mod.rs"));
        assert!(mentions_specific_target("look at crates/hone-core"));
    }

    #[test]
    fn backtick_is_specific() {
        assert!(mentions_specific_target("rename `foo` to `bar`"));
    }

    #[test]
    fn vague_request_is_not_specific() {
        assert!(!mentions_specific_target("fix the bug"));
        assert!(!mentions_specific_target("make it better"));
    }

    // --- is_code_change_request ---------------------------------------------

    #[test]
    fn change_verbs_detected() {
        assert!(is_code_change_request("fix the auth bug"));
        assert!(is_code_change_request("add a new endpoint"));
        assert!(is_code_change_request("refactor the parser"));
        assert!(is_code_change_request("remove dead code"));
    }

    #[test]
    fn question_is_not_code_change() {
        assert!(!is_code_change_request("what does this function do?"));
        assert!(!is_code_change_request("how does the cache work?"));
    }

    // --- contains_ambiguous_terms -------------------------------------------

    #[test]
    fn fix_it_is_ambiguous() {
        assert!(contains_ambiguous_terms("fix it"));
        assert!(contains_ambiguous_terms("can you update it please"));
    }

    #[test]
    fn fix_specific_is_not_ambiguous() {
        // "fix" followed by a specific file doesn't trigger.
        assert!(!contains_ambiguous_terms("fix auth.rs"));
    }

    // --- mentions_multiple_concerns -----------------------------------------

    #[test]
    fn and_also_triggers() {
        assert!(mentions_multiple_concerns(
            "fix the login bug and also add a test"
        ));
    }

    #[test]
    fn two_imperative_sentences_trigger() {
        assert!(mentions_multiple_concerns(
            "fix the login bug. Add a test for it."
        ));
    }

    #[test]
    fn single_sentence_does_not_trigger() {
        assert!(!mentions_multiple_concerns("fix the login bug"));
    }

    // --- ClarificationDetector::needs_clarification -------------------------

    #[test]
    fn clear_request_returns_none() {
        // Specific file → no clarification needed.
        assert!(ClarificationDetector::needs_clarification("fix auth.rs").is_none());
        assert!(ClarificationDetector::needs_clarification("refactor src/main.rs").is_none());
    }

    #[test]
    fn simple_command_returns_none() {
        assert!(ClarificationDetector::needs_clarification("hello").is_none());
        assert!(ClarificationDetector::needs_clarification("help").is_none());
    }

    #[test]
    fn vague_request_returns_questions() {
        let q = ClarificationDetector::needs_clarification("fix it");
        assert!(q.is_some(), "expected questions for 'fix it'");
    }

    #[test]
    fn short_vague_request_returns_questions() {
        let q = ClarificationDetector::needs_clarification("make better");
        assert!(q.is_some(), "expected questions for very short request");
    }

    #[test]
    fn multi_concern_returns_questions() {
        let q = ClarificationDetector::needs_clarification(
            "fix the login bug and also add rate limiting",
        );
        assert!(q.is_some(), "expected questions for multi-concern request");
    }

    #[test]
    fn format_questions_produces_numbered_list() {
        let qs = vec!["Q1?".to_string(), "Q2?".to_string()];
        let formatted = ClarificationDetector::format_questions(&qs);
        assert!(formatted.contains("1. Q1?"));
        assert!(formatted.contains("2. Q2?"));
    }
}
