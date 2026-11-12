// Held-out inputs, not in the task's tests. Added to the fresh directory as tests/heldout.rs.
use kvparse::parse_kv;

fn pair(k: &str, v: &str) -> Option<(String, String)> {
    Some((k.to_string(), v.to_string()))
}

#[test]
fn heldout_values_keep_everything_after_the_first_equals() {
    assert_eq!(parse_kv("a=b=c=d"), pair("a", "b=c=d"));
    assert_eq!(parse_kv("k==v"), pair("k", "=v"));
    assert_eq!(parse_kv("  key  =  a = b  "), pair("key", "a = b"));
    assert_eq!(parse_kv("path=/usr/bin:/bin?x=1"), pair("path", "/usr/bin:/bin?x=1"));
}

#[test]
fn heldout_keys_and_edge_cases() {
    assert_eq!(parse_kv("   =x"), None);
    assert_eq!(parse_kv(""), None);
    assert_eq!(parse_kv("x="), pair("x", ""));
}
