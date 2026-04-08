/// Complexity tier that controls which agent execution strategy is selected.
///
/// Based on the Agentless (2024) finding that 40–50 % of tasks don't need
/// multi-agent orchestration, saving 50–80 % of token spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComplexityTier {
    /// Simple task: single agent, no orchestration.
    ///
    /// Heuristic signals: < 100-char request, mentions a single file, or
    /// contains low-complexity keywords ("fix", "typo", "rename", "update").
    Simple,
    /// Medium task: single agent with self-refine / test-verification loop.
    ///
    /// Heuristic signals: everything that is not Simple or Complex.
    Medium,
    /// Complex task: multi-agent orchestrator pipeline.
    ///
    /// Heuristic signals: > 300-char request, architectural keywords
    /// ("architecture", "refactor multiple", "implement", "across"), or
    /// the request explicitly mentions three or more distinct files.
    Complex,
}

/// Rule-based complexity classifier (v1 — ML routing can be added later).
pub struct ComplexityClassifier;

impl ComplexityClassifier {
    /// Classify a user request into a [`ComplexityTier`].
    ///
    /// The algorithm applies heuristics in priority order:
    /// 1. Keyword scan for complexity signals (always checked first).
    /// 2. File-count heuristic (three or more `.ext` file mentions → Complex).
    /// 3. Explicit simple keywords → Simple.
    /// 4. Length-based defaults: < 100 chars → Simple, > 300 chars → Complex.
    /// 5. Default → Medium.
    pub fn classify(request: &str) -> ComplexityTier {
        let char_count = request.chars().count();

        // ── Complex keywords take priority over length shortcuts ────────────
        if has_complex_keyword(request) {
            return ComplexityTier::Complex;
        }

        // ── Three or more file references signals multi-file work ───────────
        if count_file_references(request) >= 3 {
            return ComplexityTier::Complex;
        }

        // ── Long requests strongly suggest complex, multi-file work ─────────
        if char_count > 300 {
            return ComplexityTier::Complex;
        }

        // ── Explicit simple keywords → Simple ───────────────────────────────
        if has_simple_keyword(request) {
            return ComplexityTier::Simple;
        }

        ComplexityTier::Medium
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Keywords that signal a complex, multi-file or architectural task.
fn has_complex_keyword(s: &str) -> bool {
    let lower = s.to_lowercase();
    let complex_terms = [
        "architecture",
        "refactor multiple",
        "add feature",
        "implement",
        "across",
        "across multiple",
        "multiple modules",
        "multiple files",
        "entire codebase",
        "system-wide",
        "redesign",
        "migration",
        "integrate",
    ];
    complex_terms.iter().any(|kw| lower.contains(kw))
}

/// Keywords that signal a simple, single-touch task.
fn has_simple_keyword(s: &str) -> bool {
    let lower = s.to_lowercase();
    let simple_terms = [
        "fix typo",
        "typo",
        "rename",
        "update",
        "fix ",
        "single file",
        "one file",
        "change the",
        "correct the",
    ];
    simple_terms.iter().any(|kw| lower.contains(kw))
}

/// Count distinct file references (tokens that look like `something.ext`).
///
/// Trailing punctuation (`,`, `.`, `;`, `)`) is stripped before matching so
/// that tokens like `"parser.rs,"` are correctly recognised as `.rs` files.
fn count_file_references(s: &str) -> usize {
    let known_extensions = [
        ".rs", ".toml", ".json", ".yaml", ".yml", ".ts", ".js", ".py",
        ".go", ".cpp", ".c", ".h", ".md", ".txt", ".html", ".css",
    ];

    s.split_whitespace()
        .map(|token| token.trim_end_matches([',', '.', ';', ')', '!']))
        .filter(|token| known_extensions.iter().any(|ext| token.ends_with(ext)))
        .count()
}
