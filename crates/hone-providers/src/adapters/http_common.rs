//! Helpers shared by the HTTP adapters (Anthropic, OpenAI-compatible).

/// Normalise a provider base URL: trim surrounding whitespace, drop trailing
/// `/`, then one trailing whole `/v1` segment, then trailing `/` again. Request
/// sites append `/v1/...` themselves, so `https://openrouter.ai/api/v1` must not
/// become `.../v1/v1/...`. `/apiv1`, `/v1beta` and `/v10` are left alone.
pub(crate) fn normalize_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let without_v1 = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    without_v1.trim_end_matches('/').to_owned()
}

/// Incremental UTF-8 decoder for network chunks.
///
/// Chunk boundaries are set by the network, not by characters, so a multi-byte
/// character can be split across two chunks. `push` appends the longest valid
/// prefix, keeps an incomplete trailing sequence (at most 3 bytes) for the next
/// chunk, and replaces invalid sequences with U+FFFD exactly as
/// [`String::from_utf8_lossy`] does on the whole body. Call `finish` at end of
/// stream to flush a carried incomplete sequence as U+FFFD.
#[derive(Debug, Default)]
pub(crate) struct Utf8Decoder {
    carry: Vec<u8>,
}

impl Utf8Decoder {
    pub(crate) fn push(&mut self, chunk: &[u8], out: &mut String) {
        let joined: Vec<u8>;
        let mut bytes: &[u8] = if self.carry.is_empty() {
            chunk
        } else {
            joined = [self.carry.as_slice(), chunk].concat();
            self.carry.clear();
            &joined
        };
        loop {
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    out.push_str(text);
                    return;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if let Ok(text) = std::str::from_utf8(&bytes[..valid]) {
                        out.push_str(text);
                    }
                    match e.error_len() {
                        Some(bad) => {
                            tracing::debug!(
                                "invalid UTF-8 in a provider stream replaced with U+FFFD"
                            );
                            out.push('\u{FFFD}');
                            bytes = &bytes[valid + bad..];
                        }
                        // Truncated sequence at the end: wait for the next chunk.
                        None => {
                            self.carry.extend_from_slice(&bytes[valid..]);
                            return;
                        }
                    }
                }
            }
        }
    }

    /// End of stream: an incomplete trailing sequence becomes U+FFFD.
    pub(crate) fn finish(&mut self, out: &mut String) {
        if !self.carry.is_empty() {
            out.push('\u{FFFD}');
            self.carry.clear();
        }
    }
}

/// Idle limit for a streamed response: how long the body may stay silent before
/// the read fails with a timeout. It is reset by every received frame, so a
/// long answer that keeps streaming is never cut off.
pub(crate) const HTTP_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
pub(crate) const HTTP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Build the shared HTTP client: connect timeout plus a per-read idle timeout
/// (not a total deadline).
#[allow(clippy::expect_used)] // the builder only fails if the TLS backend cannot initialise
pub(crate) fn build_client(idle: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .read_timeout(idle)
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .build()
        .expect("reqwest builder cannot fail with valid timeout values")
}

/// An error with its `source()` chain joined by `: `, so a timeout deep in
/// hyper reads "timed out" in the message.
pub(crate) fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(src) = cur {
        let msg = src.to_string();
        if !out.contains(&msg) {
            out.push_str(": ");
            out.push_str(&msg);
        }
        cur = src.source();
    }
    out
}

/// The adapters that stream over HTTP (their names appear in error messages).
const HTTP_ADAPTERS: [&str; 3] = ["openai", "anthropic", "ollama"];

/// Message for a response that went silent for longer than the idle limit.
/// `is_idle_timeout_message` recognises exactly this shape.
pub(crate) fn timed_out_message(provider: &str, cause: &str) -> String {
    format!("{provider} response timed out: {cause}")
}

/// True for a message produced by [`timed_out_message`] for one of our HTTP
/// adapters. Only the exact prefix counts, so a provider's own in-band text
/// that merely mentions a timeout is never taken for an idle timeout.
pub(crate) fn is_idle_timeout_message(message: &str) -> bool {
    HTTP_ADAPTERS
        .iter()
        .any(|p| message.starts_with(&timed_out_message(p, "")))
}

/// Tracks how a provider's event stream ends. A stream is *complete* once a
/// `Done` was parsed (each adapter emits `Done` at its completion marker) and
/// *failed* once an `Error` was emitted. Completion wins over a later transport
/// error; nothing follows an `Error`.
#[derive(Debug, Default)]
pub(crate) struct StreamProgress {
    complete: bool,
    failed: bool,
    /// The end-of-stream arm already ran: the byte stream must not be polled again.
    ended: bool,
}

impl StreamProgress {
    /// Record freshly parsed events. Everything after the first `Error` is
    /// dropped, so an in-band error can never be followed by `Done`.
    pub(crate) fn observe(&mut self, events: &mut Vec<hone_proto::ProviderEvent>) {
        use hone_proto::ProviderEvent;
        if let Some(pos) = events
            .iter()
            .position(|e| matches!(e, ProviderEvent::Error { .. }))
        {
            events.truncate(pos + 1);
            self.failed = true;
        }
        if !self.failed && events.iter().any(|e| matches!(e, ProviderEvent::Done)) {
            self.complete = true;
        }
    }

    /// Mark the response complete without a `Done` event (a completion marker
    /// that the parser reports differently, e.g. Anthropic's `stop_reason`).
    pub(crate) fn mark_complete(&mut self) {
        self.complete = true;
    }

    /// True once an `Error` was emitted or the end of the stream was handled:
    /// the reader must stop and never poll the byte stream again.
    pub(crate) fn should_stop(&self) -> bool {
        self.failed || self.ended
    }

    /// At end of stream: the one `Error` for a response that did not complete
    /// (`cause` is the transport error, `None` for a clean end).
    pub(crate) fn end_events(
        &mut self,
        provider: &str,
        cause: Option<&reqwest::Error>,
    ) -> Vec<hone_proto::ProviderEvent> {
        self.ended = true;
        if self.complete || self.failed {
            return Vec::new();
        }
        self.failed = true;
        let message = match cause {
            Some(e) if e.is_timeout() => timed_out_message(provider, &error_chain(e)),
            Some(e) => format!("{provider} response interrupted: {}", error_chain(e)),
            None => format!("{provider} response ended before it was complete"),
        };
        vec![hone_proto::ProviderEvent::Error {
            message,
            kind: hone_proto::StreamErrorKind::Unclassified,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_in_pieces(body: &[u8], sizes: &[usize]) -> String {
        let mut d = Utf8Decoder::default();
        let mut out = String::new();
        let mut rest = body;
        for &n in sizes {
            let n = n.min(rest.len());
            d.push(&rest[..n], &mut out);
            rest = &rest[n..];
        }
        d.push(rest, &mut out);
        d.finish(&mut out);
        out
    }

    #[test]
    fn test_decoder_any_two_way_split_equals_lossy_of_the_whole() {
        let bodies: [&[u8]; 4] = [
            "Zażółć €中😀 end".as_bytes(),
            b"ab\xffcd",
            b"tail \xf0\x9f\x98",
            b"\xe2\x82\xe2\x82\xac\xc3",
        ];
        for body in bodies {
            let want = String::from_utf8_lossy(body).into_owned();
            for cut in 0..=body.len() {
                assert_eq!(decode_in_pieces(body, &[cut]), want, "{body:?} cut {cut}");
            }
        }
    }

    #[test]
    fn test_decoder_one_byte_at_a_time_equals_lossy() {
        let body = "Zażółć €中😀 \u{FFFD}".as_bytes();
        let sizes = vec![1; body.len()];
        assert_eq!(
            decode_in_pieces(body, &sizes),
            String::from_utf8_lossy(body)
        );
        let bad = b"x\xf0\x9f\x98y\xffz\xc3";
        let sizes = vec![1; bad.len()];
        assert_eq!(decode_in_pieces(bad, &sizes), String::from_utf8_lossy(bad));
    }

    #[test]
    fn test_decoder_carries_at_most_three_bytes() {
        let mut d = Utf8Decoder::default();
        let mut out = String::new();
        d.push(&[0xf0, 0x9f, 0x98], &mut out);
        assert!(out.is_empty());
        assert_eq!(d.carry.len(), 3);
    }
}
