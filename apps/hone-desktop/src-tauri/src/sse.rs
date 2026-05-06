//! SSE consumer for `POST /run` and any other `text/event-stream` endpoint
//! `honed` exposes.
//!
//! The wire format honed emits is the standard SSE shape
//! (`text/event-stream`):
//!
//! ```text
//! id: 1
//! event: token
//! data: {"type":"token","text":"hello"}
//!
//! id: 2
//! event: tool_call_start
//! data: {"type":"tool_call_start", ...}
//!
//! ```
//!
//! Frames are separated by `\n\n`. A frame may carry one of `id`, `event`,
//! `data`, `retry`, or a comment line beginning with `:`.
//!
//! This module provides:
//! - [`parse_frame`]: a pure parser turning one frame's bytes into a
//!   [`SseFrame`]. Easy to unit-test.
//! - [`SseStream`]: an async stream wrapper around `reqwest::Response::bytes_stream()`
//!   that yields fully-parsed frames.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// One parsed SSE frame. `event` and `id` are optional per the spec; `data`
/// may span multiple `data:` lines (joined with `\n`). Comments are dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseFrame {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
}

impl SseFrame {
    pub fn is_empty(&self) -> bool {
        self.id.is_none() && self.event.is_none() && self.data.is_empty()
    }
}

/// Parse a single SSE frame (newline-separated lines, no trailing `\n\n`).
///
/// Spec semantics:
/// - Lines beginning with `:` are comments → ignored.
/// - `field: value` pairs (with optional space after the colon).
/// - Multiple `data:` lines accumulate, separated by `\n`.
/// - Bare `field` lines (no colon) are treated as `field: ""` (rare).
pub fn parse_frame(raw: &str) -> SseFrame {
    let mut frame = SseFrame::default();
    let mut data_lines: Vec<&str> = Vec::new();
    for line in raw.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "id" => frame.id = Some(value.to_owned()),
            "event" => frame.event = Some(value.to_owned()),
            "data" => data_lines.push(value),
            // "retry" and unknown fields silently ignored — we don't auto-reconnect
            // at the SSE layer; the caller decides retry policy.
            _ => {}
        }
    }
    frame.data = data_lines.join("\n");
    frame
}

/// Buffered framer. Feed bytes via [`Self::push`]; drain complete frames via
/// [`Self::next_frame`] until it returns `None`.
///
/// Handles the case where a chunk delivers partial frames or multiple frames.
#[derive(Debug, Default)]
pub struct SseFramer {
    buf: String,
    ready: VecDeque<SseFrame>,
}

impl SseFramer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a UTF-8 chunk. Non-UTF-8 bytes panic — honed only ever emits
    /// UTF-8 JSON, so this is acceptable.
    pub fn push(&mut self, chunk: &str) {
        self.buf.push_str(chunk);
        // Drain frames separated by "\n\n" or "\r\n\r\n".
        loop {
            let split = self.buf.find("\n\n").or_else(|| self.buf.find("\r\n\r\n"));
            match split {
                Some(idx) => {
                    let raw = self.buf[..idx].to_owned();
                    let drop_len = if self.buf[idx..].starts_with("\r\n\r\n") {
                        4
                    } else {
                        2
                    };
                    self.buf.drain(..idx + drop_len);
                    let frame = parse_frame(&raw);
                    if !frame.is_empty() {
                        self.ready.push_back(frame);
                    }
                }
                None => break,
            }
        }
    }

    pub fn next_frame(&mut self) -> Option<SseFrame> {
        self.ready.pop_front()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_data_line() {
        let frame = parse_frame("data: hello");
        assert_eq!(frame.data, "hello");
        assert!(frame.event.is_none());
        assert!(frame.id.is_none());
    }

    #[test]
    fn parses_id_event_data_combo() {
        let frame = parse_frame("id: 7\nevent: token\ndata: {\"x\":1}");
        assert_eq!(frame.id.as_deref(), Some("7"));
        assert_eq!(frame.event.as_deref(), Some("token"));
        assert_eq!(frame.data, "{\"x\":1}");
    }

    #[test]
    fn joins_multiple_data_lines_with_newline() {
        let frame = parse_frame("data: line1\ndata: line2\ndata: line3");
        assert_eq!(frame.data, "line1\nline2\nline3");
    }

    #[test]
    fn ignores_comment_lines() {
        let frame = parse_frame(": this is a heartbeat\ndata: payload");
        assert_eq!(frame.data, "payload");
    }

    #[test]
    fn handles_field_without_colon() {
        // Per the spec, a bare line is field=line, value="".
        let frame = parse_frame("data\ndata: actual");
        // The first "data" with no colon contributes "" to data_lines, which
        // we filter as empty when joining? Actually the spec says we DO add
        // the empty string, so the join gives "\nactual".
        assert_eq!(frame.data, "\nactual");
    }

    #[test]
    fn strips_optional_space_after_colon() {
        let frame = parse_frame("data:no_space");
        assert_eq!(frame.data, "no_space");
        let frame2 = parse_frame("data: with_space");
        assert_eq!(frame2.data, "with_space");
    }

    #[test]
    fn framer_emits_multiple_complete_frames_in_one_push() {
        let mut f = SseFramer::new();
        f.push("data: a\n\ndata: b\n\ndata: c\n\n");
        let mut frames = Vec::new();
        while let Some(fr) = f.next_frame() {
            frames.push(fr.data);
        }
        assert_eq!(frames, vec!["a", "b", "c"]);
    }

    #[test]
    fn framer_buffers_partial_frame_until_complete() {
        let mut f = SseFramer::new();
        f.push("data: par");
        assert!(f.next_frame().is_none(), "no frame yet");
        f.push("tial");
        assert!(f.next_frame().is_none(), "still no terminator");
        f.push("\n\n");
        let frame = f.next_frame().expect("frame complete");
        assert_eq!(frame.data, "partial");
    }

    #[test]
    fn framer_handles_crlf_line_endings() {
        let mut f = SseFramer::new();
        f.push("data: x\r\n\r\ndata: y\r\n\r\n");
        let a = f.next_frame().expect("a");
        let b = f.next_frame().expect("b");
        assert_eq!(a.data, "x");
        assert_eq!(b.data, "y");
    }

    #[test]
    fn framer_skips_empty_frames_from_consecutive_blank_lines() {
        let mut f = SseFramer::new();
        f.push("\n\ndata: real\n\n");
        let frame = f.next_frame().expect("got real");
        assert_eq!(frame.data, "real");
        assert!(f.next_frame().is_none());
    }
}
