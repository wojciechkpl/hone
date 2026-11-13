use kvparse::parse_kv;

#[test]
fn simple_pair() {
    assert_eq!(parse_kv("name = hone"), Some(("name".into(), "hone".into())));
}

#[test]
fn value_may_contain_equals_signs() {
    assert_eq!(
        parse_kv("url=https://x.test/?a=1&b=2"),
        Some(("url".into(), "https://x.test/?a=1&b=2".into()))
    );
}

#[test]
fn empty_value_is_allowed() {
    assert_eq!(parse_kv("flag="), Some(("flag".into(), "".into())));
}

#[test]
fn lines_without_a_key_are_rejected() {
    assert_eq!(parse_kv("=value"), None);
    assert_eq!(parse_kv("no separator"), None);
}
