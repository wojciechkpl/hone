/// Parse a `key=value` line. Keys and values are trimmed; the value is
/// everything after the first `=`. Returns None for lines without a key.
pub fn parse_kv(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.split('=').collect();
    if parts.len() < 2 {
        return None;
    }
    let key = parts[0].trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), parts[1].trim().to_string()))
}
