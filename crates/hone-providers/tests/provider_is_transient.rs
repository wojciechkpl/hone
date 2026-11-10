#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R10 (`docs/plans/stream-retry-fallback-spec.md`): `ProviderError::is_transient()`.
//!
//! Kept in its own file because the method does not exist yet: the compile error stays here
//! and does not hide the other new tests.

use hone_providers::ProviderError;

fn http(status: u16) -> ProviderError {
    ProviderError::Http {
        provider: "p".to_owned(),
        status,
        body: "b".to_owned(),
    }
}

#[test]
fn test_r10_http_status_boundaries() {
    for (status, expected) in [
        (0, false),
        (400, false),
        (401, false),
        (404, false),
        (428, false),
        (429, true),
        (430, false),
        (499, false),
        (500, true),
        (502, true),
        (503, true),
        (504, true),
        (529, true),
        (599, true),
        (600, false),
    ] {
        assert_eq!(
            http(status).is_transient(),
            expected,
            "HTTP status {status}"
        );
    }
}

#[test]
fn test_r10_other_variants() {
    assert!(
        ProviderError::RateLimit {
            provider: "p".to_owned(),
            retry_after_secs: 1
        }
        .is_transient()
    );
    assert!(
        ProviderError::Timeout {
            provider: "p".to_owned()
        }
        .is_transient()
    );
    assert!(
        ProviderError::Stream {
            provider: "p".to_owned(),
            source: std::io::Error::other("reset"),
        }
        .is_transient()
    );
    assert!(
        !ProviderError::ModelNotFound {
            model: "m".to_owned(),
            provider: "p".to_owned()
        }
        .is_transient()
    );
    let ser = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert!(!ProviderError::Serialization(ser).is_transient());
}
