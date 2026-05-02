use std::io::Write;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use hone_memory::{MemoryFilter, MemoryKind, MemoryQuery, MemoryStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

// ---------------------------------------------------------------------------
// Public enum — the contract pinned by `bins/hone/tests/memory_cmd_test.rs`.
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
pub enum MemoryCmd {
    /// List memories (most-relevant first).
    Ls {
        /// Maximum number of memories to list (default: 50).
        #[arg(long)]
        limit: Option<usize>,
        /// Filter by kind (conversation, user, feedback, project, reference, outcome).
        #[arg(long)]
        kind: Option<String>,
    },
    /// Show one memory in full.
    Show {
        /// Memory id (or an 8-char prefix).
        id: String,
    },
    /// Bulk delete memories matching exactly one filter.
    Forget {
        /// Delete all memories with this tag.
        #[arg(long)]
        tag: Option<String>,
        /// Delete all memories of this kind.
        #[arg(long)]
        kind: Option<String>,
        /// Delete all memories created before this RFC3339 timestamp.
        #[arg(long)]
        before: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// MemoryKind from string (case-insensitive)
// ---------------------------------------------------------------------------

fn kind_from_str(s: &str) -> Result<MemoryKind> {
    match s.to_lowercase().as_str() {
        "conversation" => Ok(MemoryKind::Conversation),
        "user" => Ok(MemoryKind::User),
        "feedback" => Ok(MemoryKind::Feedback),
        "project" => Ok(MemoryKind::Project),
        "reference" => Ok(MemoryKind::Reference),
        "outcome" => Ok(MemoryKind::Outcome),
        other => Err(anyhow!("unknown memory kind: {other}")),
    }
}

// ---------------------------------------------------------------------------
// RFC3339 timestamp parsing and formatting via the `time` crate.
//
// The `time` crate correctly handles non-UTC timezone offsets (converting them
// to UTC before producing unix_timestamp) and validates day-of-month against
// the actual month and year (including leap-year rules).
// ---------------------------------------------------------------------------

fn parse_rfc3339_to_epoch_secs(s: &str) -> Result<u64> {
    let dt = OffsetDateTime::parse(s, &Rfc3339)
        .map_err(|e| anyhow!("invalid RFC3339 timestamp '{s}': {e}"))?;
    let secs = dt.unix_timestamp();
    if secs < 0 {
        return Err(anyhow!("timestamps before 1970 are not supported"));
    }
    Ok(secs as u64)
}

fn epoch_to_rfc3339(secs: u64) -> String {
    let dt = OffsetDateTime::from_unix_timestamp(secs as i64).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    dt.format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

// ---------------------------------------------------------------------------
// Truncate a string to `max_chars` chars, appending "…" if truncated.
// ---------------------------------------------------------------------------

fn truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = chars[..max_chars].iter().collect();
        format!("{truncated}\u{2026}")
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub async fn run(
    cmd: MemoryCmd,
    store: Arc<dyn MemoryStore>,
    out: &mut dyn Write,
) -> Result<()> {
    match cmd {
        MemoryCmd::Ls { limit, kind } => {
            let effective_limit = limit.unwrap_or(50);
            let target_kind = kind.as_deref().map(kind_from_str).transpose()?;
            let memories = store
                .recall(&MemoryQuery {
                    text: String::new(),
                    limit: effective_limit,
                    kind: target_kind,
                })
                .await?;

            for m in &memories {
                let kind_label = kind_label(m.kind);
                let timestamp = epoch_to_rfc3339(m.created_at);
                let id_prefix = if m.id.len() > 8 { &m.id[..8] } else { &m.id };
                let description_text = m.description.as_deref().unwrap_or(&m.content);
                let snippet = truncate(description_text, 60);
                writeln!(out, "{id_prefix}  {kind_label}  {timestamp}  {snippet}")?;
            }

            Ok(())
        }

        MemoryCmd::Show { id } => {
            // Try exact match via recall_by_id (O(log n)).  If that misses,
            // fall back to a bounded prefix scan — handles 8-char prefixes
            // shown in `memory ls` as well as any future non-uuid id format.
            let memory = if let Some(m) = store.recall_by_id(&id).await? {
                m
            } else {
                let all = store
                    .recall(&MemoryQuery {
                        text: String::new(),
                        limit: 1000,
                        kind: None,
                    })
                    .await?;
                all.into_iter()
                    .find(|m| m.id.starts_with(&id as &str))
                    .ok_or_else(|| anyhow!("memory not found: {id}"))?
            };

            let timestamp = epoch_to_rfc3339(memory.created_at);
            let tags = if memory.tags.is_empty() {
                "(none)".to_string()
            } else {
                memory.tags.join(", ")
            };

            writeln!(out, "id:          {}", memory.id)?;
            writeln!(out, "kind:        {}", kind_label(memory.kind))?;
            writeln!(out, "created_at:  {timestamp}")?;
            writeln!(out, "tags:        {tags}")?;
            writeln!(out, "weight:      {:.4}", memory.weight)?;
            if let Some(desc) = &memory.description {
                writeln!(out, "description: {desc}")?;
            }
            writeln!(out)?;
            writeln!(out, "{}", memory.content)?;

            Ok(())
        }

        MemoryCmd::Forget { tag, kind, before } => {
            // Validate: exactly one filter must be set.
            let filter_count = [tag.is_some(), kind.is_some(), before.is_some()]
                .iter()
                .filter(|&&b| b)
                .count();
            if filter_count != 1 {
                return Err(anyhow!("specify exactly one of --tag, --kind, --before"));
            }

            let filter = if let Some(tag_val) = tag {
                MemoryFilter::Tag(tag_val)
            } else if let Some(kind_val) = kind {
                let k = kind_from_str(&kind_val)?;
                MemoryFilter::Kind(k)
            } else {
                // before must be Some — we checked filter_count == 1 above.
                let before_str = before.unwrap();
                let epoch_secs = parse_rfc3339_to_epoch_secs(&before_str)?;
                MemoryFilter::Before(epoch_secs)
            };

            let n = store.forget(&filter).await?;
            writeln!(out, "Removed {n}")?;

            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn kind_label(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Conversation => "conversation",
        MemoryKind::User => "user",
        MemoryKind::Feedback => "feedback",
        MemoryKind::Project => "project",
        MemoryKind::Reference => "reference",
        MemoryKind::Outcome => "outcome",
    }
}
