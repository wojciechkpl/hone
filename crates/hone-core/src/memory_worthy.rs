use hone_memory::MemoryKind;

use crate::memory_format::StructuredBody;

/// A candidate memory entry produced by [`classify_turn`].
#[derive(Debug)]
pub struct MemoryCandidate {
    pub kind: MemoryKind,
    pub description: Option<String>,
    pub body: StructuredBody,
}

// ---------------------------------------------------------------------------
// Trigger tables
// ---------------------------------------------------------------------------

/// Correction phrases that indicate the user is correcting the assistant.
///
/// These are intentionally narrow: they must appear as direct imperatives
/// targeting the assistant's behaviour ("stop doing X", "don't use Y"),
/// not as general advice or temporal statements ("stop the migration").
const CORRECTION_TRIGGERS: &[&str] = &[
    "don't do ",
    "don't use ",
    "don't add ",
    "don't mock ",
    "stop doing ",
    "stop using ",
    "stop summarizing",
    "no, not",
    "no — ",
    "no -- ",
];

/// Validation phrases that indicate the user is confirming a non-obvious approach.
const VALIDATION_TRIGGERS: &[&str] = &[
    "yes, exactly",
    "perfect, keep",
    "that's exactly right",
];

/// External-system link patterns that indicate a reference memory.
///
/// `github.com` only fires when the URL points to a specific issue or PR
/// (contains `/issues/` or `/pull/`), not a bare repository link.
const REFERENCE_TRIGGERS: &[&str] = &[
    "linear.app",
    "slack.com",
    "grafana",
    "jira",
];

/// Project state markers.
///
/// `sprint` and `release` are restricted to Agile/milestone phrasing to avoid
/// matching the plain verb forms ("I sprinted", "we should release today").
const PROJECT_TRIGGERS: &[&str] = &[
    "freezing",
    "freeze",
    "deadline",
    "by thursday",
    "by friday",
    "by monday",
    "by tuesday",
    "by wednesday",
    "q1 ",
    "q2 ",
    "q3 ",
    "q4 ",
    " q1",
    " q2",
    " q3",
    " q4",
    "sprint planning",
    "the sprint",
    "release branch",
    "cutting a release",
];

/// Role / self-introduction patterns.
const USER_TRIGGERS: &[&str] = &[
    "i'm a ",
    "i am a ",
    "i work as",
    "my role is",
    "first time",
];

// ---------------------------------------------------------------------------
// Slack channel pattern helper
// ---------------------------------------------------------------------------

/// Returns `true` if the text contains a `#word` pattern that looks like a
/// Slack channel mention (e.g., `#ops`, `#general`).
fn has_slack_channel(text: &str) -> bool {
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' {
            // There must be a letter or digit after the `#`.
            if let Some(&next) = bytes.get(i + 1) {
                if next.is_ascii_alphanumeric() {
                    return true;
                }
            }
        }
    }
    false
}

/// Returns `true` if `lower` contains a `github.com` URL that points to a
/// specific issue or pull request (i.e., contains `/issues/` or `/pull/`).
fn has_github_issue_or_pr(lower: &str) -> bool {
    if !lower.contains("github.com") {
        return false;
    }
    lower.contains("/issues/") || lower.contains("/pull/")
}

// ---------------------------------------------------------------------------
// Main classifier
// ---------------------------------------------------------------------------

/// Decide whether the just-completed turn produced anything worth keeping as a
/// long-term memory.
///
/// Returns `None` for noisy turns — the caller should then write nothing.
///
/// This is a pure heuristic: no LLM call, no external I/O.  False negatives
/// are cheaper than false positives.
pub fn classify_turn(user_input: &str, assistant_reply: &str) -> Option<MemoryCandidate> {
    if user_input.is_empty() && assistant_reply.is_empty() {
        return None;
    }

    let lower = user_input.to_lowercase();

    // 1. Correction patterns → Feedback
    for &trigger in CORRECTION_TRIGGERS {
        if lower.contains(trigger) {
            return Some(make_feedback(user_input, "correction"));
        }
    }

    // 2. Validation patterns → Feedback
    for &trigger in VALIDATION_TRIGGERS {
        if lower.contains(trigger) {
            return Some(make_feedback(user_input, "validation"));
        }
    }

    // 3. External links → Reference  (check user input)
    for &trigger in REFERENCE_TRIGGERS {
        if lower.contains(trigger) {
            return Some(make_reference(user_input));
        }
    }

    // GitHub: only fire for issue/PR URLs, not bare repo links.
    if has_github_issue_or_pr(&lower) {
        return Some(make_reference(user_input));
    }

    // Slack channel-style `#word` in user input.
    if has_slack_channel(user_input) {
        return Some(make_reference(user_input));
    }

    // 4. Project state → Project
    for &trigger in PROJECT_TRIGGERS {
        if lower.contains(trigger) {
            return Some(make_project(user_input));
        }
    }

    // 5. Role declarations → User
    for &trigger in USER_TRIGGERS {
        if lower.contains(trigger) {
            return Some(make_user(user_input));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Candidate constructors
// ---------------------------------------------------------------------------

/// Trim `input` to at most `max_chars` characters for use as a rule or
/// description.  Adds "…" suffix if truncated.
fn trim_to(input: &str, max_chars: usize) -> String {
    let trimmed = input.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_owned()
    } else {
        let truncated: String = trimmed.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

fn make_feedback(user_input: &str, kind: &str) -> MemoryCandidate {
    let description = Some(format!(
        "feedback {kind}: {}",
        trim_to(user_input, 80)
    ));
    MemoryCandidate {
        kind: MemoryKind::Feedback,
        description,
        body: StructuredBody {
            rule: trim_to(user_input, 200),
            why: None,
            how: None,
        },
    }
}

fn make_reference(user_input: &str) -> MemoryCandidate {
    let description = Some(format!(
        "external reference: {}",
        trim_to(user_input, 80)
    ));
    MemoryCandidate {
        kind: MemoryKind::Reference,
        description,
        body: StructuredBody {
            rule: trim_to(user_input, 200),
            why: None,
            how: None,
        },
    }
}

fn make_project(user_input: &str) -> MemoryCandidate {
    let description = Some(format!(
        "project state: {}",
        trim_to(user_input, 80)
    ));
    MemoryCandidate {
        kind: MemoryKind::Project,
        description,
        body: StructuredBody {
            rule: trim_to(user_input, 200),
            why: None,
            how: None,
        },
    }
}

fn make_user(user_input: &str) -> MemoryCandidate {
    let description = Some(format!(
        "user info: {}",
        trim_to(user_input, 80)
    ));
    MemoryCandidate {
        kind: MemoryKind::User,
        description,
        body: StructuredBody {
            rule: trim_to(user_input, 200),
            why: None,
            how: None,
        },
    }
}
