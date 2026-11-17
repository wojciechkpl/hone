//! Shared helper for model ids that carry a release-date suffix.

/// `id` without a trailing `-YYYY-MM-DD` or `-YYYYMMDD`, when it has one.
pub(crate) fn strip_date_suffix(id: &str) -> Option<&str> {
    let bytes = id.as_bytes();
    let n = bytes.len();
    let digits = |r: &[u8]| r.iter().all(u8::is_ascii_digit);
    if n >= 11 {
        let t = &bytes[n - 11..];
        if t[0] == b'-'
            && t[5] == b'-'
            && t[8] == b'-'
            && digits(&t[1..5])
            && digits(&t[6..8])
            && digits(&t[9..11])
        {
            return Some(&id[..n - 11]);
        }
    }
    if n >= 9 {
        let t = &bytes[n - 9..];
        // Exactly eight digits after the dash (not nine).
        if t[0] == b'-' && digits(&t[1..]) {
            return Some(&id[..n - 9]);
        }
    }
    None
}
