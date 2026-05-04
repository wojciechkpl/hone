use std::io::{BufRead, BufReader, Read, Write};

use crate::builder::BuilderError;
use crate::example::TrainingExample;

/// Write examples one per line to `writer`, terminated by `\n`. Empty input
/// produces empty output (no header). Preserves insertion order.
pub fn write_jsonl<W: Write>(
    examples: &[TrainingExample],
    writer: &mut W,
) -> Result<(), BuilderError> {
    for example in examples {
        let line = serde_json::to_string(example)?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

/// Parse JSONL from `reader`. Skips blank lines silently. Returns Err on
/// the FIRST line that fails to parse, with a 1-based line number embedded
/// in the error.
pub fn read_jsonl<R: Read>(reader: R) -> Result<Vec<TrainingExample>, JsonlError> {
    let mut examples = Vec::new();
    for (line_number, line) in BufReader::new(reader).lines().enumerate() {
        let raw = line?;
        let line_number = line_number + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let example: TrainingExample =
            serde_json::from_str(trimmed).map_err(|source| JsonlError::Parse {
                line: line_number,
                source,
            })?;
        examples.push(example);
    }
    Ok(examples)
}

#[derive(Debug, thiserror::Error)]
pub enum JsonlError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("line {line}: {source}")]
    Parse {
        line: usize,
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(prompt: &str, response: &str) -> TrainingExample {
        TrainingExample::new(prompt, response)
    }

    fn ex_scored(prompt: &str, response: &str, score: f32) -> TrainingExample {
        TrainingExample::new(prompt, response).with_score(score)
    }

    fn round_trip(examples: &[TrainingExample]) -> Vec<TrainingExample> {
        let mut buf = Vec::<u8>::new();
        write_jsonl(examples, &mut buf).expect("write_jsonl");
        read_jsonl(buf.as_slice()).expect("read_jsonl")
    }

    #[test]
    fn write_jsonl_empty_produces_empty_output() {
        let mut buf = Vec::<u8>::new();
        write_jsonl(&[], &mut buf).expect("write_jsonl");
        assert!(buf.is_empty());
    }

    #[test]
    fn write_jsonl_emits_one_line_per_example() {
        let examples = vec![ex("p1", "r1"), ex("p2", "r2"), ex("p3", "r3")];
        let mut buf = Vec::<u8>::new();
        write_jsonl(&examples, &mut buf).expect("write_jsonl");
        let text = String::from_utf8(buf).expect("utf8");
        let non_empty_lines: Vec<_> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(non_empty_lines.len(), 3);
    }

    #[test]
    fn write_jsonl_preserves_insertion_order() {
        let examples = vec![
            ex_scored("first", "r1", 0.9),
            ex_scored("second", "r2", 0.5),
            ex_scored("third", "r3", 0.1),
        ];
        let mut buf = Vec::<u8>::new();
        write_jsonl(&examples, &mut buf).expect("write_jsonl");
        let text = String::from_utf8(buf).expect("utf8");
        let lines: Vec<_> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3);
        // Each line must contain the expected prompt in insertion order.
        assert!(lines[0].contains("\"first\""));
        assert!(lines[1].contains("\"second\""));
        assert!(lines[2].contains("\"third\""));
    }

    #[test]
    fn read_jsonl_parses_round_trip() {
        let examples = vec![
            ex_scored("q1", "a1", 0.8).with_metadata("model", "llama3"),
            ex("q2", "a2"),
        ];
        let parsed = round_trip(&examples);
        assert_eq!(parsed, examples);
    }

    #[test]
    fn read_jsonl_skips_blank_lines() {
        // Manually craft JSONL with blank lines interspersed.
        let ex1 = ex("p1", "r1");
        let ex2 = ex("p2", "r2");
        let line1 = serde_json::to_string(&ex1).expect("ser");
        let line2 = serde_json::to_string(&ex2).expect("ser");
        let input = format!("{line1}\n\n{line2}\n");
        let parsed = read_jsonl(input.as_bytes()).expect("read_jsonl");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], ex1);
        assert_eq!(parsed[1], ex2);
    }

    #[test]
    fn read_jsonl_reports_line_number_on_parse_error() {
        let ex1 = ex("p1", "r1");
        let line1 = serde_json::to_string(&ex1).expect("ser");
        // Second line (line number 2) is invalid JSON.
        let input = format!("{line1}\nnot valid json\n");
        let err = read_jsonl(input.as_bytes()).expect_err("should fail");
        match err {
            JsonlError::Parse { line, .. } => {
                assert_eq!(line, 2, "error must report 1-based line number 2");
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn read_jsonl_handles_trailing_newline() {
        let examples = vec![ex("p1", "r1"), ex("p2", "r2")];
        let mut buf = Vec::<u8>::new();
        write_jsonl(&examples, &mut buf).expect("write_jsonl");
        // Ensure there's a trailing newline (write_jsonl should already add one,
        // but we force it for explicitness).
        if !buf.ends_with(b"\n") {
            buf.push(b'\n');
        }
        let parsed = read_jsonl(buf.as_slice()).expect("read_jsonl");
        assert_eq!(parsed.len(), 2);
    }
}
