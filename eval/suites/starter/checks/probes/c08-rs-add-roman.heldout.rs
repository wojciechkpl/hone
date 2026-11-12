// Held-out inputs, not in the task's tests. Added to the fresh directory as tests/heldout.rs.
use numerals::to_roman;

#[test]
fn heldout_numerals() {
    for (n, want) in [
        (2, "II"),
        (3, "III"),
        (14, "XIV"),
        (40, "XL"),
        (58, "LVIII"),
        (90, "XC"),
        (400, "CD"),
        (444, "CDXLIV"),
        (2024, "MMXXIV"),
        (3888, "MMMDCCCLXXXVIII"),
    ] {
        assert_eq!(to_roman(n).as_deref(), Some(want), "{n}");
    }
    assert_eq!(to_roman(4000), None);
    assert_eq!(to_roman(u32::MAX), None);
}
