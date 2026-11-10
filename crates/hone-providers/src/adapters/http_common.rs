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

/// Message for a response that went silent for longer than the idle limit.
pub(crate) fn timed_out_message(provider: &str, cause: &str) -> String {
    format!("{provider} response timed out: {cause}")
}

/// Status implied by a provider's error name (`error.type` / a named `error.code`).
/// Exact and case-sensitive.
const ERROR_NAMES: [(&str, u16); 11] = [
    ("invalid_request_error", 400),
    ("authentication_error", 401),
    ("permission_error", 403),
    ("not_found_error", 404),
    ("request_too_large", 413),
    ("rate_limit_error", 429),
    ("api_error", 500),
    ("overloaded_error", 529),
    ("server_error", 500),
    ("context_length_exceeded", 400),
    ("rate_limit_exceeded", 429),
];

fn status_for_name(name: &str) -> Option<u16> {
    ERROR_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, status)| *status)
}

fn valid_status(n: u64) -> Option<u16> {
    u16::try_from(n).ok().filter(|s| (100..=599).contains(s))
}

/// Status of an OpenAI-compatible in-band `error` object, first match wins:
/// `code` an integer in 100-599; `code` a digit string in 100-599; `code` a
/// known name; `type` a known name; otherwise none.
pub(crate) fn openai_error_status(err: &serde_json::Value) -> Option<u16> {
    let code = &err["code"];
    code.as_u64()
        .and_then(valid_status)
        .or_else(|| {
            code.as_str()
                .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|s| s.parse::<u64>().ok())
                .and_then(valid_status)
        })
        .or_else(|| code.as_str().and_then(status_for_name))
        .or_else(|| err["type"].as_str().and_then(status_for_name))
}

/// Status of an Anthropic `error` event's `error.type`, if it is a known name.
pub(crate) fn anthropic_error_status(err: &serde_json::Value) -> Option<u16> {
    err["type"].as_str().and_then(status_for_name)
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
        let (message, kind) = match cause {
            Some(e) if e.is_timeout() => (
                timed_out_message(provider, &error_chain(e)),
                hone_proto::StreamErrorKind::IdleTimeout,
            ),
            Some(e) => (
                format!("{provider} response interrupted: {}", error_chain(e)),
                hone_proto::StreamErrorKind::Interrupted,
            ),
            None => (
                format!("{provider} response ended before it was complete"),
                hone_proto::StreamErrorKind::Interrupted,
            ),
        };
        vec![hone_proto::ProviderEvent::Error { message, kind }]
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
