//! `hone context show`: render the context receipts stored with a session.

use std::fmt::Write as _;

use anyhow::{Result, bail};
use hone_proto::{ContextReceipt, SourceRef};

use crate::session::StoredReceipt;

/// Render the stored receipts for `hone context show`.
///
/// `run` and `turn` are the `--run` and `--turn` filters (`turn` without `run`
/// means the latest run); `json` prints a JSON array of
/// `{ "run", "seq", "receipt" }`. An unknown run, or a turn not in the selected
/// run, is an error naming it. With no receipts at all the result is one line
/// saying so (an empty array with `json`).
pub fn render_context_show(
    stored: &[StoredReceipt],
    run: Option<u32>,
    turn: Option<u32>,
    json: bool,
) -> Result<String> {
    if stored.is_empty() {
        return Ok(if json {
            "[]\n".to_owned()
        } else {
            "No context receipts stored for this session.\n".to_owned()
        });
    }

    let mut sorted: Vec<&StoredReceipt> = stored.iter().collect();
    sorted.sort_by_key(|s| (s.run, s.seq));

    let selected_run = match (run, turn) {
        (Some(r), _) => Some(r),
        (None, Some(_)) => sorted.last().map(|s| s.run),
        (None, None) => None,
    };
    if let Some(r) = selected_run {
        sorted.retain(|s| s.run == r);
        if sorted.is_empty() {
            bail!("no run {r} in this session");
        }
    }
    if let Some(t) = turn {
        sorted.retain(|s| s.receipt.turn == t);
        if sorted.is_empty() {
            match selected_run {
                Some(r) => bail!("no turn {t} in run {r}"),
                None => bail!("no turn {t} in this session"),
            }
        }
    }

    if json {
        let items: Vec<serde_json::Value> = sorted
            .iter()
            .map(|s| serde_json::json!({ "run": s.run, "seq": s.seq, "receipt": s.receipt }))
            .collect();
        return Ok(format!("{}\n", serde_json::to_string_pretty(&items)?));
    }

    let mut out = String::new();
    for s in sorted {
        render_receipt(&mut out, s.run, &s.receipt);
    }
    Ok(out)
}

fn render_receipt(out: &mut String, run: u32, r: &ContextReceipt) {
    let hash: String = r.request_sha256.chars().take(12).collect();
    let _ = writeln!(
        out,
        "run {run} turn {} call {}  {}  request {hash}",
        r.turn,
        r.call,
        esc(&r.model)
    );
    let listed: usize = r
        .blocks
        .iter()
        .filter(|b| b.slot == hone_proto::Slot::History)
        .map(|b| b.sources.len())
        .sum();
    if r.history_len > 0 || listed > 0 {
        let root: String = r.history_root.chars().take(12).collect();
        let _ = writeln!(
            out,
            "  history: {} messages ({listed} new listed), root {root}",
            r.history_len
        );
    }
    for b in &r.blocks {
        let _ = writeln!(
            out,
            "  {} [{}] {} tokens",
            slot_name(b.slot),
            esc(&b.strategy.name),
            b.tokens
        );
        for s in &b.sources {
            match s.score {
                Some(score) => {
                    let _ = writeln!(out, "    {} (score {score})", source_label(&s.source));
                }
                None => {
                    let _ = writeln!(out, "    {}", source_label(&s.source));
                }
            }
        }
    }
    if !r.dropped.is_empty() {
        let _ = writeln!(out, "  dropped:");
        for d in &r.dropped {
            let _ = writeln!(out, "    {}", source_label(d));
        }
    }
    if !r.compaction.is_empty() {
        let _ = writeln!(out, "  compaction:");
        for c in &r.compaction {
            let replaced: Vec<String> = c.replaced.iter().map(source_label).collect();
            let derived = c
                .derived
                .as_ref()
                .map_or_else(|| "removed".to_owned(), source_label);
            let _ = writeln!(
                out,
                "    {}: {} -> {derived}",
                esc(&c.strategy.name),
                replaced.join(", ")
            );
        }
    }
    out.push('\n');
}

/// `text` with control characters (C0, DEL, C1) shown as `\u{1b}`-style escapes,
/// so a receipt string can't move the cursor, retitle the terminal or forge lines.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

fn slot_name(slot: hone_proto::Slot) -> String {
    serde_json::to_value(slot)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{slot:?}"))
}

/// A short label for a source: ids, paths and names; hashes cut to 12 digits.
fn source_label(source: &SourceRef) -> String {
    let short = |h: &str| h.chars().take(12).collect::<String>();
    match source {
        SourceRef::Memory { id, version } => match version {
            Some(v) => format!("memory {} v{v}", esc(id)),
            None => format!("memory {}", esc(id)),
        },
        SourceRef::File { path, .. } => format!("file {}", esc(&path.display().to_string())),
        SourceRef::Message { index, sha256 } => format!("message #{index} {}", short(sha256)),
        SourceRef::ToolResult { call_id, sha256 } => {
            format!("tool_result {} {}", esc(call_id), short(sha256))
        }
        SourceRef::Skill { name } => format!("skill {}", esc(name)),
        SourceRef::Command { argv, .. } => format!(
            "command {}",
            argv.iter().map(|a| esc(a)).collect::<Vec<_>>().join(" ")
        ),
        SourceRef::Chunk {
            index, chunk_id, ..
        } => format!("chunk {}/{}", esc(index), esc(chunk_id)),
        SourceRef::Opaque { plugin, note } => format!("{}: {}", esc(plugin), esc(note)),
    }
}
