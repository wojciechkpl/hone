#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E5 (`docs/plans/provider-error-kind-spec.md`): one rule for a transient status.
//!
//! Own file because neither item exists yet: the compile error stays here.
//! PINNED API: `hone_proto::is_transient_status(u16) -> bool` (crate root) and
//! `StreamErrorKind::is_retryable(&self) -> bool`.

use hone_proto::{StreamErrorKind, is_transient_status};

#[test]
fn test_e5_is_transient_status_table() {
    for (status, expected) in [
        (0, false),
        (100, false),
        (200, false),
        (400, false),
        (401, false),
        (428, false),
        (429, true),
        (430, false),
        (499, false),
        (500, true),
        (502, true),
        (503, true),
        (529, true),
        (599, true),
        (600, false),
        (u16::MAX, false),
    ] {
        assert_eq!(is_transient_status(status), expected, "status {status}");
    }
}

#[test]
fn test_e5_is_retryable_for_the_fixed_kinds() {
    assert!(StreamErrorKind::Unclassified.is_retryable());
    assert!(StreamErrorKind::Interrupted.is_retryable());
    assert!(!StreamErrorKind::IdleTimeout.is_retryable());
    assert!(StreamErrorKind::Provider { status: None }.is_retryable());
}

#[test]
fn test_e5_is_retryable_for_provider_statuses_follows_is_transient_status() {
    for status in [
        0u16, 99, 400, 401, 403, 404, 413, 428, 429, 430, 499, 500, 529, 599, 600,
    ] {
        assert_eq!(
            StreamErrorKind::Provider {
                status: Some(status)
            }
            .is_retryable(),
            is_transient_status(status),
            "status {status}"
        );
    }
    assert!(!StreamErrorKind::Provider { status: Some(400) }.is_retryable());
    assert!(StreamErrorKind::Provider { status: Some(529) }.is_retryable());
}
