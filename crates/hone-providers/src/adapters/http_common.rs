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
