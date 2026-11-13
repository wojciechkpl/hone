/// Parse a `key=value` line. Keys and values are trimmed; the value is
/// everything after the first `=`. Returns None for lines without a key.
pub fn parse_kv(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), value.trim().to_string()))
}
