#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T1 of `docs/plans/audit-timestamps-spec.md`: audit-log timestamps are correct
//! RFC 3339 UTC. API (new): `hone_core::egress_log::format_rfc3339_utc(secs: u64) -> String`
//! (`YYYY-MM-DDTHH:MM:SSZ`, proleptic Gregorian); `now_rfc3339()` uses it.

use chrono::{DateTime, Datelike, Utc};
use hone_core::egress_log::{format_rfc3339_utc, now_rfc3339};

fn chrono_format(secs: u64) -> String {
    DateTime::<Utc>::from_timestamp(secs as i64, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

#[test]
fn test_t1_known_values_should_format_on_the_real_calendar() {
    for (secs, expected) in [
        (0, "1970-01-01T00:00:00Z"),
        (951_782_400, "2000-02-29T00:00:00Z"),
        (1_767_225_599, "2025-12-31T23:59:59Z"),
        (1_794_441_600, "2026-11-12T00:00:00Z"),
        (1_798_761_599, "2026-12-31T23:59:59Z"),
    ] {
        assert_eq!(format_rfc3339_utc(secs), expected, "{secs}");
    }
}

#[test]
fn test_t1_every_day_of_a_leap_and_a_common_year_should_match_chrono() {
    // Noon of every day of 2024 (leap) and 2026 (common).
    for year in [2024_i32, 2026] {
        let mut day = chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
        while day.year() == year {
            let secs = day.and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp() as u64;
            assert_eq!(format_rfc3339_utc(secs), chrono_format(secs), "{day}");
            day = day.succ_opt().unwrap();
        }
    }
}

#[test]
fn test_t1_a_spread_of_timestamps_should_match_chrono() {
    // An arbitrary stride across 1970..2100, plus day boundaries and the leap-day edge.
    let mut secs = 0_u64;
    while secs < 4_102_444_800 {
        assert_eq!(format_rfc3339_utc(secs), chrono_format(secs), "{secs}");
        secs += 86_399 * 37 + 12_345;
    }
    for secs in [86_399, 86_400, 951_868_799, 951_868_800, 4_102_444_799] {
        assert_eq!(format_rfc3339_utc(secs), chrono_format(secs), "{secs}");
    }
}

#[test]
fn test_t1_now_should_use_the_format_and_be_close_to_the_real_time() {
    let s = now_rfc3339();

    let b = s.as_bytes();
    assert_eq!(b.len(), 20, "{s}");
    for (i, c) in b.iter().enumerate() {
        match i {
            4 | 7 => assert_eq!(*c, b'-', "{s}"),
            10 => assert_eq!(*c, b'T', "{s}"),
            13 | 16 => assert_eq!(*c, b':', "{s}"),
            19 => assert_eq!(*c, b'Z', "{s}"),
            _ => assert!(c.is_ascii_digit(), "{s}"),
        }
    }
    let parsed = DateTime::parse_from_rfc3339(&s).unwrap_or_else(|e| panic!("{s}: {e}"));
    let drift = (Utc::now() - parsed.with_timezone(&Utc)).num_seconds().abs();
    assert!(drift <= 5, "{s} is {drift}s from now");
}
