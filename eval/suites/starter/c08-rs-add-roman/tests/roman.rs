use numerals::{is_even, to_roman};

#[test]
fn even() {
    assert!(is_even(4));
}

#[test]
fn small_numbers() {
    assert_eq!(to_roman(1).as_deref(), Some("I"));
    assert_eq!(to_roman(4).as_deref(), Some("IV"));
    assert_eq!(to_roman(9).as_deref(), Some("IX"));
}

#[test]
fn larger_numbers() {
    assert_eq!(to_roman(1994).as_deref(), Some("MCMXCIV"));
    assert_eq!(to_roman(3999).as_deref(), Some("MMMCMXCIX"));
}

#[test]
fn out_of_range_is_none() {
    assert_eq!(to_roman(0), None);
    assert_eq!(to_roman(4000), None);
}
