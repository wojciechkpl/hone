//! The built-in chunkers: `markdown` (sections by heading) and `fixed`
//! (overlapping windows of lines).

use hone_proto::{Slot, StrategyId, canonical_json_sha256, sha256_hex};

use crate::docs::{Chunk, Chunker, Document};

/// The longest piece of a markdown section, in lines.
const MAX_SECTION_LINES: usize = 60;

fn strategy(name: &str, params: &serde_json::Value) -> StrategyId {
    StrategyId {
        slot: Slot::Retrieval,
        name: name.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        params_sha256: canonical_json_sha256(params),
    }
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn make_chunk(
    doc: &Document,
    lines: &[&str],
    start: usize,
    end: usize,
    heading: Option<String>,
) -> Chunk {
    let text = lines[start..end].join("\n");
    Chunk {
        id: format!("{}#{}", doc.uri, start + 1),
        doc_uri: doc.uri.clone(),
        start_line: (start + 1) as u32,
        end_line: end as u32,
        heading,
        sha256: sha256_hex(text.as_bytes()),
        text,
    }
}

/// Splits at ATX headings outside fenced code; each chunk carries its heading path.
pub struct MarkdownChunker {
    id: StrategyId,
}

impl MarkdownChunker {
    pub fn new() -> Self {
        Self {
            id: strategy("markdown", &serde_json::json!({})),
        }
    }
}

impl Default for MarkdownChunker {
    fn default() -> Self {
        Self::new()
    }
}

/// `(level, title)` of an ATX heading line.
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim();
    if title.is_empty() {
        None
    } else {
        Some((hashes, title))
    }
}

impl Chunker for MarkdownChunker {
    fn id(&self) -> &StrategyId {
        &self.id
    }

    fn chunk(&self, doc: &Document) -> Vec<Chunk> {
        let lines: Vec<&str> = doc.text.lines().collect();

        // Front matter: `---` on the first line up to the next `---` line.
        let mut first = 0;
        if lines.first().is_some_and(|l| l.trim_end() == "---") {
            if let Some(close) = lines.iter().skip(1).position(|l| l.trim_end() == "---") {
                first = close + 2;
            }
        }

        // Section boundaries: (start line index, heading level and title).
        let mut starts: Vec<(usize, Option<(usize, String)>)> = vec![(first, None)];
        let mut fence: Option<char> = None;
        for (i, line) in lines.iter().enumerate().skip(first) {
            let t = line.trim_start();
            let marker = if t.starts_with("```") {
                Some('`')
            } else if t.starts_with("~~~") {
                Some('~')
            } else {
                None
            };
            if let Some(m) = marker {
                match fence {
                    None => fence = Some(m),
                    Some(open) if open == m => fence = None,
                    Some(_) => {}
                }
                continue;
            }
            if fence.is_some() {
                continue;
            }
            if let Some((level, title)) = heading_of(line) {
                starts.push((i, Some((level, title.to_owned()))));
            }
        }

        let mut path: Vec<(usize, String)> = Vec::new();
        let mut chunks = Vec::new();
        for (n, (start, head)) in starts.iter().enumerate() {
            let end = starts.get(n + 1).map_or(lines.len(), |(s, _)| *s);
            if let Some((level, title)) = head {
                while path.last().is_some_and(|(l, _)| l >= level) {
                    path.pop();
                }
                path.push((*level, title.clone()));
            }
            let heading = if path.is_empty() {
                None
            } else {
                Some(
                    path.iter()
                        .map(|(_, t)| t.as_str())
                        .collect::<Vec<_>>()
                        .join(" > "),
                )
            };
            // The text of the section: no leading (preamble) or trailing blank lines.
            let mut s = *start;
            if head.is_none() {
                while s < end && is_blank(lines[s]) {
                    s += 1;
                }
            }
            let mut e = end;
            while e > s && is_blank(lines[e - 1]) {
                e -= 1;
            }
            // A bare heading has no text of its own.
            let body_from = if head.is_some() { s + 1 } else { s };
            if lines[body_from.min(e)..e].iter().all(|l| is_blank(l)) {
                continue;
            }
            // Pieces of at most 60 lines, split at blank lines where possible.
            let mut p = s;
            while p < e {
                let limit = p + MAX_SECTION_LINES;
                if e <= limit {
                    chunks.push(make_chunk(doc, &lines, p, e, heading.clone()));
                    break;
                }
                let split = (p + 1..=limit).rev().find(|&b| is_blank(lines[b]));
                let (piece_end, next) = match split {
                    Some(b) => (b, b + 1),
                    None => (limit, limit),
                };
                let mut pe = piece_end;
                while pe > p && is_blank(lines[pe - 1]) {
                    pe -= 1;
                }
                if pe > p {
                    chunks.push(make_chunk(doc, &lines, p, pe, heading.clone()));
                }
                p = next;
                while p < e && is_blank(lines[p]) {
                    p += 1;
                }
            }
        }
        chunks
    }
}

/// Windows of `lines` lines overlapping by `overlap`.
pub struct FixedChunker {
    pub lines: usize,
    pub overlap: usize,
}

impl Default for FixedChunker {
    fn default() -> Self {
        Self {
            lines: 40,
            overlap: 5,
        }
    }
}

impl Chunker for FixedChunker {
    fn id(&self) -> &StrategyId {
        // The window parameters are public fields, so the id names the strategy
        // only; the registry reports the parameters.
        static ID: std::sync::OnceLock<StrategyId> = std::sync::OnceLock::new();
        ID.get_or_init(|| strategy("fixed", &serde_json::json!({})))
    }

    fn chunk(&self, doc: &Document) -> Vec<Chunk> {
        let lines: Vec<&str> = doc.text.lines().collect();
        let n = lines.len();
        let window = self.lines.max(1);
        let step = window.saturating_sub(self.overlap).max(1);
        let mut chunks = Vec::new();
        let mut s = 0;
        while s < n {
            let e = (s + window).min(n);
            if lines[s..e].iter().any(|l| !is_blank(l)) {
                chunks.push(make_chunk(doc, &lines, s, e, None));
            }
            if e == n {
                break;
            }
            s += step;
        }
        chunks
    }
}
