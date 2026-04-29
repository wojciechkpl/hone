//! Helper for parsing the `Retry-After` response header.
//!
//! Per RFC 9110 §10.2.3 the field value is either:
//! - An integer: delay in seconds.
//! - An HTTP-date: absolute point in time after which the client may retry.
//!
//! We try the integer form first; HTTP-date parsing requires the `httpdate`
//! crate which is not currently a workspace dependency, so that branch is left
//! as a TODO and the fallback is a 60-second default.

use reqwest::header::HeaderMap;

/// Default retry-after delay when the header is absent or unparseable.
pub const DEFAULT_RETRY_AFTER_SECS: u64 = 60;

/// Parse the `Retry-After` header from `headers`.
///
/// Returns the number of seconds the caller should wait before retrying.
/// Falls back to [`DEFAULT_RETRY_AFTER_SECS`] when:
/// - the header is absent,
/// - the value is not valid UTF-8,
/// - the value cannot be parsed as an integer, or
/// - (TODO) the value is an HTTP-date — add `httpdate` to workspace deps and
///   implement `parse_http_date_secs` to handle that branch.
pub fn parse_retry_after(headers: &HeaderMap) -> u64 {
    let Some(value) = headers.get("retry-after") else {
        return DEFAULT_RETRY_AFTER_SECS;
    };

    let Ok(text) = value.to_str() else {
        return DEFAULT_RETRY_AFTER_SECS;
    };

    let trimmed = text.trim();

    // Try integer seconds first (the common case for all major LLM APIs).
    if let Ok(secs) = trimmed.parse::<u64>() {
        return secs;
    }

    // TODO: try HTTP-date via `httpdate::parse_http_date` and compute
    // seconds-from-now.  Requires adding `httpdate` to workspace Cargo.toml:
    //   httpdate = "1"
    // Then:
    //   if let Ok(date) = httpdate::parse_http_date(trimmed) {
    //       if let Ok(duration) = date.duration_since(std::time::SystemTime::now()) {
    //           return duration.as_secs();
    //       }
    //   }

    DEFAULT_RETRY_AFTER_SECS
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use reqwest::header::{HeaderMap, HeaderValue};

    use super::*;

    fn headers_with(value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(
            "retry-after",
            HeaderValue::from_str(value).expect("valid header value"),
        );
        map
    }

    #[test]
    fn integer_header_returns_exact_value() {
        let headers = headers_with("120");
        assert_eq!(parse_retry_after(&headers), 120);
    }

    #[test]
    fn integer_header_zero_returns_zero() {
        let headers = headers_with("0");
        assert_eq!(parse_retry_after(&headers), 0);
    }

    #[test]
    fn missing_header_returns_default() {
        let headers = HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), DEFAULT_RETRY_AFTER_SECS);
    }

    #[test]
    fn garbage_header_returns_default() {
        let headers = headers_with("not-a-number");
        assert_eq!(parse_retry_after(&headers), DEFAULT_RETRY_AFTER_SECS);
    }

    #[test]
    fn whitespace_trimmed_integer() {
        let headers = headers_with("  30  ");
        assert_eq!(parse_retry_after(&headers), 30);
    }
}
