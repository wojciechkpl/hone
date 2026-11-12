#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R1, R2 and R3 of `docs/plans/doc-retrieval-spec.md`: the document types, the traits
//! and the two chunkers, as pure functions.
//!
//! API required (new, re-exported at the `hone_index` crate root):
//!
//! ```ignore
//! pub struct Document { pub uri: String, pub text: String, pub sha256: String }
//! pub struct Chunk {
//!     pub id: String,            // "<doc_uri>#<start_line>", stable
//!     pub doc_uri: String,
//!     pub start_line: u32,       // 1-based, inclusive
//!     pub end_line: u32,         // 1-based, inclusive; trailing blank lines are not part of a chunk
//!     pub heading: Option<String>, // heading path, " > " separated ("Setup > Linux")
//!     pub text: String,          // the chunk's lines joined with "\n" (no "\r"); for markdown it
//!                                // includes the heading line itself
//!     pub sha256: String,        // sha256_hex of `text`
//! }
//! pub struct Hit { pub source: hone_proto::SourceRef, pub text: String, pub score: f32 }
//! pub trait Chunker: Send + Sync { fn id(&self) -> &hone_proto::StrategyId;
//!                                  fn chunk(&self, doc: &Document) -> Vec<Chunk>; }
//! #[async_trait] pub trait Retriever: Send + Sync { fn id(&self) -> &hone_proto::StrategyId;
//!     async fn retrieve(&self, query: &str, k: usize) -> Result<Vec<Hit>, IndexError>; }
//!
//! pub struct MarkdownChunker;                         // MarkdownChunker::new(), strategy name "markdown"
//! pub struct FixedChunker { pub lines: usize, pub overlap: usize }  // Default: 40 / 5; name "fixed"
//! ```
//!
//! Markdown rules (R2): ATX headings `#`..`######` (a space after the hashes) outside fenced
//! code blocks (``` or ~~~); each section (heading line to the line before the next heading)
//! is one chunk with the heading path of the headings above it (a heading of the same or a
//! higher level pops the path); a section with no non-blank body line after its heading
//! still gets a chunk only if it has text besides the heading; front matter (`---` ... `---`
//! at the very start) is skipped; a section longer than 60 lines is split at blank lines into
//! pieces of at most 60 lines (a hard split at 60 when there is no blank line); lines before
//! the first heading form a chunk with `heading: None`.
//!
//! Fixed rules (R3): windows of `lines` lines starting every `max(lines - overlap, 1)` lines,
//! the last window ends at the last line; no extra window that lies entirely inside the previous
//! one; heading `None`.

use hone_index::{Chunk, Chunker, Document, FixedChunker, Hit, MarkdownChunker, Retriever};
use hone_proto::{SourceRef, sha256_hex};

fn doc(uri: &str, text: &str) -> Document {
    Document {
        uri: uri.to_owned(),
        text: text.to_owned(),
        sha256: sha256_hex(text.as_bytes()),
    }
}

fn md(uri: &str, text: &str) -> Vec<Chunk> {
    MarkdownChunker::new().chunk(&doc(uri, text))
}

fn shape(chunks: &[Chunk]) -> Vec<(u32, u32, Option<&str>)> {
    chunks.iter().map(|c| (c.start_line, c.end_line, c.heading.as_deref())).collect()
}

const GUIDE: &str = "# Guide\nIntro line.\n\n## Setup\nInstall it.\n### Linux\napt install x\n### macOS\nbrew install x\n## Usage\nRun it.\n";

// ---- R1: the types and traits ----------------------------------------------------------------

#[test]
fn test_r1_chunk_and_hit_should_be_plain_data() {
    let c = Chunk {
        id: "a.md#1".into(),
        doc_uri: "a.md".into(),
        start_line: 1,
        end_line: 2,
        heading: Some("A".into()),
        text: "# A\nx".into(),
        sha256: sha256_hex(b"# A\nx"),
    };
    let h = Hit {
        source: SourceRef::Chunk { index: "docs".into(), chunk_id: c.id.clone(), sha256: c.sha256.clone() },
        text: c.text.clone(),
        score: 1.5,
    };
    assert_eq!(c.clone(), c);
    assert_eq!(h.score, 1.5);
}

#[test]
fn test_r1_a_custom_chunker_should_fit_the_trait() {
    struct Whole(hone_proto::StrategyId);
    impl Chunker for Whole {
        fn id(&self) -> &hone_proto::StrategyId {
            &self.0
        }
        fn chunk(&self, d: &Document) -> Vec<Chunk> {
            vec![Chunk {
                id: format!("{}#1", d.uri),
                doc_uri: d.uri.clone(),
                start_line: 1,
                end_line: 1,
                heading: None,
                text: d.text.clone(),
                sha256: d.sha256.clone(),
            }]
        }
    }
    let w = Whole(hone_proto::StrategyId {
        slot: hone_proto::Slot::Retrieval,
        name: "whole".into(),
        version: "1".into(),
        params_sha256: String::new(),
    });
    assert_eq!(w.chunk(&doc("x.txt", "hi")).len(), 1);
    let _: Option<&dyn Retriever> = None; // the trait exists and is object safe
}

#[test]
fn test_r1_the_builtin_chunkers_should_name_their_strategies() {
    let m = MarkdownChunker::new();
    assert_eq!(m.id().name, "markdown");
    assert_eq!(m.id().slot, hone_proto::Slot::Retrieval);
    assert_eq!(FixedChunker::default().id().name, "fixed");
    let f = FixedChunker::default();
    assert_eq!((f.lines, f.overlap), (40, 5));
}

// ---- R2: markdown --------------------------------------------------------------------------------

#[test]
fn test_r2_headings_should_start_chunks_with_their_heading_path() {
    let chunks = md("guide.md", GUIDE);
    assert_eq!(
        shape(&chunks),
        [
            (1, 2, Some("Guide")),
            (4, 5, Some("Guide > Setup")),
            (6, 7, Some("Guide > Setup > Linux")),
            (8, 9, Some("Guide > Setup > macOS")),
            (10, 11, Some("Guide > Usage")),
        ]
    );
    assert_eq!(chunks[0].text, "# Guide\nIntro line.");
    assert_eq!(chunks[2].text, "### Linux\napt install x");
}

#[test]
fn test_r2_chunk_ids_should_be_path_and_start_line_and_hashes_should_cover_the_text() {
    let chunks = md("docs/guide.md", GUIDE);
    let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["docs/guide.md#1", "docs/guide.md#4", "docs/guide.md#6", "docs/guide.md#8", "docs/guide.md#10"]);
    for c in &chunks {
        assert_eq!(c.doc_uri, "docs/guide.md");
        assert_eq!(c.sha256, sha256_hex(c.text.as_bytes()));
    }
}

#[test]
fn test_r2_editing_a_later_section_should_not_change_earlier_ids() {
    let before = md("g.md", GUIDE);
    let after = md("g.md", &GUIDE.replace("Run it.", "Run it twice.\nThen stop."));
    assert_eq!(before[..4].iter().map(|c| &c.id).collect::<Vec<_>>(), after[..4].iter().map(|c| &c.id).collect::<Vec<_>>());
    assert_eq!(before[..4], after[..4]);
    assert_ne!(before[4].sha256, after[4].sha256);
}

#[test]
fn test_r2_headings_inside_fenced_code_blocks_are_not_headings() {
    let text = "# Real\nbefore\n```sh\n# not a heading\n## neither\n```\nafter\n~~~\n# nor this\n~~~\n## Next\nbody\n";
    let chunks = md("f.md", text);
    assert_eq!(shape(&chunks), [(1, 10, Some("Real")), (11, 12, Some("Real > Next"))]);
    assert!(chunks[0].text.contains("# not a heading") && chunks[0].text.contains("# nor this"));
}

#[test]
fn test_r2_front_matter_should_be_skipped_but_line_numbers_stay_true() {
    let text = "---\ntitle: Secret front matter\ntags: [a]\n---\n# Real\ntext\n";
    let chunks = md("f.md", text);
    assert_eq!(shape(&chunks), [(5, 6, Some("Real"))]);
    assert!(chunks.iter().all(|c| !c.text.contains("Secret front matter")));
    assert_eq!(chunks[0].id, "f.md#5");
}

#[test]
fn test_r2_a_dashed_line_that_is_not_front_matter_is_ordinary_text() {
    let chunks = md("f.md", "# Title\ntext\n---\nmore\n");
    assert_eq!(shape(&chunks), [(1, 4, Some("Title"))]);
}

#[test]
fn test_r2_text_before_the_first_heading_should_be_a_chunk_without_heading() {
    let chunks = md("f.md", "Preamble line.\n\n# First\nbody\n");
    assert_eq!(shape(&chunks), [(1, 1, None), (3, 4, Some("First"))]);
}

#[test]
fn test_r2_a_document_without_headings_should_be_one_chunk() {
    assert_eq!(shape(&md("f.md", "just\nsome\ntext\n")), [(1, 3, None)]);
}

#[test]
fn test_r2_empty_and_blank_documents_should_have_no_chunks() {
    assert!(md("f.md", "").is_empty());
    assert!(md("f.md", "\n\n  \n").is_empty());
}

#[test]
fn test_r2_a_heading_without_body_should_not_make_an_empty_chunk() {
    let chunks = md("f.md", "# Title\n## Sub\ntext\n");
    assert_eq!(shape(&chunks), [(2, 3, Some("Title > Sub"))], "the bare `# Title` section has no text of its own");
}

#[test]
fn test_r2_headings_need_a_space_and_at_most_six_hashes() {
    let chunks = md("f.md", "#hashtag not a heading\n####### seven\n# Real\nbody\n");
    assert_eq!(shape(&chunks), [(1, 2, None), (3, 4, Some("Real"))]);
}

#[test]
fn test_r2_long_sections_should_split_at_blank_lines_into_pieces_of_at_most_sixty_lines() {
    let mut text = String::from("# Big\n");
    for n in 1..=130 {
        text.push_str(&format!("line {n}\n"));
        if n % 25 == 0 {
            text.push('\n');
        }
    }
    let chunks = md("big.md", &text);

    assert!(chunks.len() >= 3, "{}", chunks.len());
    for c in &chunks {
        assert!(c.end_line - c.start_line + 1 <= 60, "{} has {} lines", c.id, c.end_line - c.start_line + 1);
        assert_eq!(c.heading.as_deref(), Some("Big"));
        assert_eq!(c.id, format!("big.md#{}", c.start_line));
    }
    assert_eq!(chunks[0].start_line, 1, "the first piece starts at the heading");
    for n in 1..=130 {
        let hits = chunks.iter().filter(|c| c.text.lines().any(|l| l == format!("line {n}"))).count();
        assert_eq!(hits, 1, "line {n} must be in exactly one piece");
    }
    // Pieces start right after a blank line (the split points).
    for c in &chunks[1..] {
        assert!(c.text.starts_with("line "), "{:?}", c.text.lines().next());
    }
}

#[test]
fn test_r2_a_long_section_without_blank_lines_should_split_hard_at_sixty() {
    let mut text = String::from("# Wall\n");
    for n in 1..=100 {
        text.push_str(&format!("row {n}\n"));
    }
    let chunks = md("wall.md", &text);
    assert!(chunks.len() >= 2);
    for c in &chunks {
        assert!(c.end_line - c.start_line + 1 <= 60);
    }
    let covered: usize = (1..=100)
        .filter(|n| chunks.iter().any(|c| c.text.lines().any(|l| l == format!("row {n}"))))
        .count();
    assert_eq!(covered, 100);
}

#[test]
fn test_r2_a_section_of_exactly_sixty_lines_should_stay_whole() {
    let mut text = String::from("# Exact\n");
    for n in 1..=59 {
        text.push_str(&format!("x {n}\n"));
    }
    assert_eq!(shape(&md("e.md", &text)), [(1, 60, Some("Exact"))]);
}

#[test]
fn test_r2_crlf_documents_should_chunk_like_lf_documents_without_carriage_returns() {
    let crlf = GUIDE.replace('\n', "\r\n");
    let a = md("g.md", GUIDE);
    let b = md("g.md", &crlf);
    assert_eq!(shape(&a), shape(&b));
    assert!(b.iter().all(|c| !c.text.contains('\r')), "no carriage returns in chunk text");
    assert_eq!(a.iter().map(|c| &c.text).collect::<Vec<_>>(), b.iter().map(|c| &c.text).collect::<Vec<_>>());
}

// ---- R3: fixed windows -------------------------------------------------------------------------------

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("l{i}\n")).collect()
}

fn fixed(lines: usize, overlap: usize, n: usize) -> Vec<Chunk> {
    FixedChunker { lines, overlap }.chunk(&doc("notes.txt", &numbered(n)))
}

#[test]
fn test_r3_windows_should_overlap_and_end_at_the_last_line() {
    let chunks = fixed(4, 1, 10);
    assert_eq!(shape(&chunks), [(1, 4, None), (4, 7, None), (7, 10, None)]);
    assert_eq!(chunks[1].text, "l4\nl5\nl6\nl7");
    let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["notes.txt#1", "notes.txt#4", "notes.txt#7"]);
    assert!(chunks.iter().all(|c| c.sha256 == sha256_hex(c.text.as_bytes()) && c.doc_uri == "notes.txt"));
}

#[test]
fn test_r3_defaults_should_be_forty_lines_with_an_overlap_of_five() {
    let chunks = FixedChunker::default().chunk(&doc("n.txt", &numbered(100)));
    assert_eq!(shape(&chunks), [(1, 40, None), (36, 75, None), (71, 100, None)]);
}

#[test]
fn test_r3_a_short_document_is_one_window_and_an_empty_one_has_none() {
    assert_eq!(shape(&fixed(40, 5, 3)), [(1, 3, None)]);
    assert!(fixed(40, 5, 0).is_empty());
    assert_eq!(shape(&fixed(4, 1, 4)), [(1, 4, None)], "no tiny extra window inside the previous one");
}

#[test]
fn test_r3_an_overlap_not_smaller_than_the_window_should_still_terminate_and_cover() {
    let chunks = fixed(3, 5, 7);
    assert!(!chunks.is_empty() && chunks.len() <= 7);
    for n in 1..=7 {
        assert!(chunks.iter().any(|c| c.text.lines().any(|l| l == format!("l{n}"))), "l{n}");
    }
}

#[test]
fn test_r3_crlf_should_chunk_like_lf() {
    let a = FixedChunker { lines: 4, overlap: 1 }.chunk(&doc("n.txt", &numbered(10)));
    let b = FixedChunker { lines: 4, overlap: 1 }.chunk(&doc("n.txt", &numbered(10).replace('\n', "\r\n")));
    assert_eq!(a, b);
}

// ---- R13 (review round 1): markdown structure ----------------------------------------------------------------------------

#[test]
fn test_r13_a_fence_should_close_only_with_the_same_character_and_at_least_the_same_length() {
    // 4 backticks open; a 3-backtick line does not close; the heading inside stays text.
    let longer = "# Top\n````\n```\n# not a heading\n````\n# Real\nbody\n";
    assert_eq!(shape(&md("f.md", longer)), [(1, 5, Some("Top")), (6, 7, Some("Real"))]);

    // A tilde fence is not closed by backticks (and vice versa).
    let mixed = "# Top\n~~~\n```\n# inside\n~~~\n# Real\nbody\n";
    assert_eq!(shape(&md("f.md", mixed)), [(1, 5, Some("Top")), (6, 7, Some("Real"))]);
    let mixed = "# Top\n```\n~~~\n# inside\n```\n# Real\nbody\n";
    assert_eq!(shape(&md("f.md", mixed)), [(1, 5, Some("Top")), (6, 7, Some("Real"))]);

    // A longer closing fence is fine.
    let ok = "# Top\n```\n# inside\n`````\n# Real\nbody\n";
    assert_eq!(shape(&md("f.md", ok)), [(1, 4, Some("Top")), (5, 6, Some("Real"))]);
}

#[test]
fn test_r13_a_leading_thematic_break_should_not_swallow_the_first_section() {
    // No closing `---` within 100 lines: this is a thematic break, not front matter.
    let text = format!("---\n# Intro\nbody line\n{}---\ntail\n", "filler\n".repeat(120));

    let chunks = md("f.md", &text);

    assert_eq!(chunks[0].heading.as_deref(), Some("Intro"), "{:?}", shape(&chunks));
    assert_eq!(chunks[0].start_line, 2);
}

#[test]
fn test_r13_front_matter_should_close_within_one_hundred_lines_in_yaml_or_toml() {
    // YAML closing at line 50 is front matter; closing at line 150 is not.
    let yaml = |n: usize| format!("---\n{}---\n# Real\ntext\n", "k: v\n".repeat(n));
    let chunks = md("f.md", &yaml(48)); // closing `---` is line 50
    assert_eq!(shape(&chunks), [(51, 52, Some("Real"))], "closed within 100 lines: skipped");
    let chunks = md("f.md", &yaml(148)); // closing `---` is line 150
    assert!(
        chunks.iter().any(|c| c.text.contains("k: v")),
        "no close within 100 lines: not front matter, its lines are text: {:?}",
        shape(&chunks)
    );

    // TOML front matter.
    let toml = "+++\ntitle = \"Hidden\"\n+++\n# Real\ntext\n";
    let chunks = md("f.md", toml);
    assert_eq!(shape(&chunks), [(4, 5, Some("Real"))]);
    assert!(chunks.iter().all(|c| !c.text.contains("Hidden")));

    // `+++` anywhere but the first line is ordinary text.
    assert_eq!(shape(&md("f.md", "# T\ntext\n+++\nmore\n")), [(1, 4, Some("T"))]);
}

#[test]
fn test_r13_atx_headings_may_be_indented_up_to_three_spaces() {
    assert_eq!(shape(&md("f.md", "   ## Indented\nbody\n")), [(1, 2, Some("Indented"))]);
    assert_eq!(shape(&md("f.md", "  # One\ntext\n ## Two\nmore\n")), [(1, 2, Some("One")), (3, 4, Some("One > Two"))]);
    // Four spaces is an indented code block, not a heading.
    assert_eq!(shape(&md("f.md", "    ## not a heading\nbody\n")), [(1, 2, None)]);
}

#[test]
fn test_r13_setext_headings_should_start_sections() {
    let text = "Title\n=====\nbody\n\nSub\n-----\ntext\n";

    let chunks = md("f.md", text);

    assert_eq!(shape(&chunks), [(1, 3, Some("Title")), (5, 7, Some("Title > Sub"))]);
    assert!(chunks[0].text.starts_with("Title\n====="), "the section includes the heading lines: {:?}", chunks[0].text);

    // After a blank line `---` is a thematic break, and inside a fence nothing is a heading.
    assert_eq!(shape(&md("f.md", "# T\ntext\n\n---\nmore\n")), [(1, 5, Some("T"))]);
    assert_eq!(shape(&md("f.md", "```\nTitle\n=====\n```\nbody\n")), [(1, 5, None)]);
}
