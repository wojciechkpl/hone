//! Memory index rendering for the agent context window.
//!
//! This module provides two public async functions: `render_index`, which
//! produces a compact Markdown-formatted summary of all live memories (grouped
//! by kind, sorted by relevance, and truncated to a configurable budget), and
//! `read_memory`, which fetches the full body of a single memory by its id.
//! Both functions treat the `MemoryStore` as the authority on which memories
//! are live: decayed or forgotten memories will not appear because the store
//! filters them before `recall` returns.

use hone_memory::{Memory, MemoryError, MemoryKind, MemoryQuery, MemoryStore};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Budget constraints for `render_index`.
#[derive(Debug, Clone, Copy)]
pub struct IndexBudget {
    /// Maximum number of entry lines to emit.
    pub max_entries: usize,
    /// Maximum total length of the rendered string in characters.
    pub max_chars: usize,
}

impl Default for IndexBudget {
    fn default() -> Self {
        Self {
            max_entries: 50,
            max_chars: 5000,
        }
    }
}

// ---------------------------------------------------------------------------
// Kind ordering
// ---------------------------------------------------------------------------

/// Fixed display order for memory kinds.
const KIND_ORDER: &[MemoryKind] = &[
    MemoryKind::User,
    MemoryKind::Feedback,
    MemoryKind::Project,
    MemoryKind::Reference,
    MemoryKind::Outcome,
    MemoryKind::Conversation,
];

fn kind_header(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::User => "### User",
        MemoryKind::Feedback => "### Feedback",
        MemoryKind::Project => "### Project",
        MemoryKind::Reference => "### Reference",
        MemoryKind::Outcome => "### Outcome",
        MemoryKind::Conversation => "### Conversation",
    }
}

fn kind_name(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::User => "user",
        MemoryKind::Feedback => "feedback",
        MemoryKind::Project => "project",
        MemoryKind::Reference => "reference",
        MemoryKind::Outcome => "outcome",
        MemoryKind::Conversation => "conversation",
    }
}

// ---------------------------------------------------------------------------
// Entry rendering helpers
// ---------------------------------------------------------------------------

/// Compute the display label for a memory entry.
///
/// Preference order:
/// 1. `description` if `Some`.
/// 2. Full `content` when ≤ 80 chars.
/// 3. First 80 chars of `content` + `"..."`.
fn entry_label(memory: &Memory) -> String {
    if let Some(desc) = &memory.description {
        return desc.clone();
    }
    let chars: Vec<char> = memory.content.chars().collect();
    if chars.len() <= 80 {
        memory.content.clone()
    } else {
        let truncated: String = chars.into_iter().take(80).collect();
        format!("{truncated}...")
    }
}

/// Render a single entry line in the format:
/// `- [<id>] <label> (kind=<kind>, w=<weight>)`
fn render_entry(memory: &Memory) -> String {
    let label = entry_label(memory);
    let kind = kind_name(memory.kind);
    // Rust's default Display for f64 prints "1" for 1.0, "1.5" for 1.5, etc.
    // when using the `{}` formatter — exactly the format the tests expect.
    let weight = memory.weight;
    format!("- [{}] {} (kind={kind}, w={weight})", memory.id, label)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a Markdown index of all live memories in the store.
///
/// Returns `""` when the store is empty.  Memories are grouped by kind in the
/// fixed order `User → Feedback → Project → Reference → Outcome → Conversation`
/// and within each group sorted by `weight` DESC, then `created_at` DESC.
/// Output is capped to `budget.max_entries` entry lines and `budget.max_chars`
/// total characters; overflow is indicated by a trailing truncation message.
pub async fn render_index(
    store: &dyn MemoryStore,
    budget: IndexBudget,
) -> Result<String, MemoryError> {
    // Fetch all live memories directly — no outer lock needed.
    let memories = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: usize::MAX,
            kind: None,
        })
        .await?;

    if memories.is_empty() {
        return Ok(String::new());
    }

    // Build sections in fixed kind order, filtering and sorting per kind.
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for &kind in KIND_ORDER {
        let mut group: Vec<&Memory> = memories.iter().filter(|m| m.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        group.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        let header = kind_header(kind).to_string();
        let lines: Vec<String> = group.iter().map(|m| render_entry(m)).collect();
        sections.push((header, lines));
    }

    // Count total available entries to know how many we will drop.
    let total_entries: usize = sections.iter().map(|(_, lines)| lines.len()).sum();

    // Pre-compute the worst-case truncation message to reserve budget for it.
    // 50 chars is a conservative reserve for any plausible count in the message
    // "_(... N more memories truncated)_".
    const TRUNCATION_RESERVE: usize = 50;

    // Assemble output while respecting both budget limits.
    // We append lines to `output_parts` and track char count and entry count.
    let mut output_parts: Vec<String> = Vec::new();
    let mut total_chars: usize = 0;
    let mut entries_emitted: usize = 0;
    let mut truncated = false;

    'outer: for (header, entry_lines) in &sections {
        // We need to know if any entries from this section will fit before
        // committing to the header.  Tentatively check.
        let header_cost = header.len() + 1; // +1 for the trailing newline

        // Check if we can fit at least the header + one entry.
        // We'll add the header speculatively and remove it if no entries fit.
        let available = budget
            .max_chars
            .saturating_sub(total_chars + TRUNCATION_RESERVE);

        if available < header_cost {
            // Not even the header fits — all remaining entries are truncated.
            truncated = true;
            break;
        }

        // Track how much header cost remains to be charged for this section.
        // It is charged once — when the first entry of the section is emitted.
        let mut remaining_header_cost = header_cost;

        for (section_idx, entry) in entry_lines.iter().enumerate() {
            if entries_emitted >= budget.max_entries {
                truncated = true;
                break 'outer;
            }

            let entry_cost = entry.len() + 1; // +1 for newline

            // For section_idx == 0 the header has not been emitted yet, so we
            // must reserve its cost.  For section_idx > 0 the header is already
            // in total_chars and remaining_header_cost has been zeroed.
            let available_now = budget
                .max_chars
                .saturating_sub(total_chars + remaining_header_cost + TRUNCATION_RESERVE);

            if available_now < entry_cost {
                truncated = true;
                break 'outer;
            }

            if section_idx == 0 {
                // Emit the header now that we know at least one entry fits.
                output_parts.push(header.clone());
                total_chars += header_cost;
                remaining_header_cost = 0;
            }

            output_parts.push(entry.clone());
            total_chars += entry_cost;
            entries_emitted += 1;
        }
    }

    if truncated {
        let remaining = total_entries - entries_emitted;
        let truncation_msg = format!("_(... {remaining} more memories truncated)_");
        output_parts.push(truncation_msg);
    }

    Ok(output_parts.join("\n"))
}

/// Fetch the full body of a single memory by its id.
///
/// Returns `Ok(Some(memory))` if the id is found, `Ok(None)` if the memory
/// does not exist or has been decayed/forgotten.
pub async fn read_memory(
    store: &dyn MemoryStore,
    id: &str,
) -> Result<Option<Memory>, MemoryError> {
    let memories = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: usize::MAX,
            kind: None,
        })
        .await?;

    Ok(memories.into_iter().find(|m| m.id == id))
}
