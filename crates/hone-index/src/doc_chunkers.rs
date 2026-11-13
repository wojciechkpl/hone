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

/// `(level, title)` of an ATX heading line (indented at most 3 spaces).
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let line = &line[indent..];
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

/// `(character, length)` of a code fence line (3+ backticks or tildes, indented
/// at most 3 spaces).
fn fence_of(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let t = &line[indent..];
    let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = t.chars().take_while(|x| *x == c).count();
    (n >= 3).then_some((c, n))
}

/// Whether `line` closes a fence of `(c, len)`: the same character, at least as
/// long, nothing else.
fn closes_fence(line: &str, c: char, len: usize) -> bool {
    match fence_of(line) {
        Some((fc, n)) if fc == c && n >= len => line.trim().chars().all(|x| x == c),
        _ => false,
    }
}

/// The level of a setext underline (`===` is 1, `---` is 2).
fn setext_level(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let t = line.trim();
    if indent > 3 || t.len() < 3 {
        return None;
    }
    if t.chars().all(|c| c == '=') {
        Some(1)
    } else if t.chars().all(|c| c == '-') {
        Some(2)
    } else {
        None
    }
}

/// A thematic break (`---`, `***`, `___`).
fn is_thematic_break(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|m| t.chars().all(|c| c.to_string() == *m))
}

/// The line index where the body starts after front matter: `---` (YAML) or
/// `+++` (TOML) on the first line, closed within 100 lines.
fn front_matter_end(lines: &[&str]) -> usize {
    let Some(first) = lines.first().map(|l| l.trim_end()) else {
        return 0;
    };
    match first {
        "+++" => lines
            .iter()
            .take(100)
            .skip(1)
            .position(|l| l.trim_end() == "+++")
            .map_or(0, |close| close + 2),
        // YAML: a horizontal rule unless a closing `---` or `...` follows within
        // 50 lines with only blank, comment and `key: value` lines between.
        "---" => {
            for (i, l) in lines.iter().enumerate().take(50).skip(1) {
                let t = l.trim_end();
                if t == "---" || t == "..." {
                    return i + 1;
                }
                if !yaml_shaped(l) {
                    return 0;
                }
            }
            0
        }
        _ => 0,
    }
}

/// A line that can sit inside YAML front matter: blank, a comment, a list item
/// or indented continuation, or `key: value`.
fn yaml_shaped(line: &str) -> bool {
    let t = line.trim_end();
    if t.trim().is_empty()
        || t.starts_with('#')
        || t.starts_with("- ")
        || t.starts_with(char::is_whitespace)
    {
        return true;
    }
    match t.split_once(':') {
        Some((key, rest)) => {
            !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '))
                && (rest.is_empty() || rest.starts_with(' '))
        }
        None => false,
    }
}

impl Chunker for MarkdownChunker {
    fn id(&self) -> &StrategyId {
        &self.id
    }

    fn chunk(&self, doc: &Document) -> Vec<Chunk> {
        let lines: Vec<&str> = doc.text.lines().collect();
        let first = front_matter_end(&lines);

        // Section boundaries: (start line index, heading (level, title) and the
        // number of lines the heading takes).
        type Start = (usize, Option<(usize, String, usize)>);
        let mut starts: Vec<Start> = vec![(first, None)];
        let mut fence: Option<(char, usize)> = None;
        let mut i = first;
        while i < lines.len() {
            let line = lines[i];
            if let Some((c, len)) = fence {
                if closes_fence(line, c, len) {
                    fence = None;
                }
                i += 1;
                continue;
            }
            if let Some(f) = fence_of(line) {
                fence = Some(f);
                i += 1;
                continue;
            }
            if let Some((level, title)) = heading_of(line) {
                starts.push((i, Some((level, title.to_owned(), 1))));
                i += 1;
                continue;
            }
            // Setext: a text line starting a paragraph, then `===` or `---`.
            let paragraph_start = i == first || is_blank(lines[i - 1]);
            if paragraph_start
                && !is_blank(line)
                && line.len() - line.trim_start_matches(' ').len() < 4
            {
                if let Some(level) = lines.get(i + 1).and_then(|l| setext_level(l)) {
                    starts.push((i, Some((level, line.trim().to_owned(), 2))));
                    i += 2;
                    continue;
                }
            }
            i += 1;
        }

        let mut path: Vec<(usize, String)> = Vec::new();
        let mut chunks = Vec::new();
        for (n, (start, head)) in starts.iter().enumerate() {
            let end = starts.get(n + 1).map_or(lines.len(), |(s, _)| *s);
            if let Some((level, title, _)) = head {
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
            // A bare heading has no text of its own (nor does a preamble of
            // only thematic breaks).
            let body_from = match head {
                Some((_, _, heading_lines)) => s + heading_lines,
                None => s,
            };
            let body = &lines[body_from.min(e)..e];
            if body
                .iter()
                .all(|l| is_blank(l) || (head.is_none() && is_thematic_break(l)))
            {
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
