//! RED-PHASE regression tests for **HIGH-1**: Seatbelt profile emits a
//! global `(allow file-read*)` rule, voiding read isolation on macOS.
//!
//! Audit finding (c):
//!     `crates/hone-sandbox/src/seatbelt.rs:95-100`
//!     The profile builder always appends `(allow file-read*)` after the
//!     per-path read rules, so a sandboxed `cat ~/.ssh/id_rsa` succeeds.
//!     The comment in the source acknowledges this; the security value of
//!     Seatbelt is reduced to writes / network / process operations.
//!
//! Contract these tests pin:
//!     The profile MUST NOT contain a global `(allow file-read*)` rule.
//!     Required system reads (dyld cache, /dev/urandom, etc.) MUST be
//!     enumerated as `(allow file-read* (subpath "..."))` or `(literal ...)`
//!     rules so a child process cannot read arbitrary user files.
//!
//! These tests will FAIL against current `main` because the literal string
//! `(allow file-read*)` (without a `(subpath ...)` or `(literal ...)`
//! qualifier) appears in every profile.
//!
//! Implementer's freedom (green phase):
//!     - Replace the unqualified rule with explicit allows for the dyld
//!       shared cache (`/usr/lib`, `/System/Library/dyld`,
//!       `/private/var/db/dyld`), `/dev/urandom`, `/dev/null`, and the
//!       running binary's path.
//!     - Document the chosen path set in the source comment.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(target_os = "macos")]
mod macos_tests {
    use hone_sandbox::{SandboxMode, SandboxPolicyBuilder, SeatbeltProfile};

    /// Strip Scheme-style `;` line-comments and split on whitespace-trimmed
    /// lines so we can inspect each rule independently of formatting.
    fn rules(profile: &str) -> Vec<String> {
        profile
            .lines()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty() && !l.starts_with(';'))
            .collect()
    }

    #[test]
    fn profile_does_not_globally_allow_file_read() {
        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .build()
            .expect("policy");

        let profile = SeatbeltProfile::from_policy(&policy);
        let rules = rules(&profile);

        // The forbidden pattern is the literal global allow with NO
        // qualifier — i.e. the rule body is exactly `(allow file-read*)` with
        // optional surrounding whitespace and no `(subpath ...)` /
        // `(literal ...)` / `(regex ...)` selector.
        let global_allow = rules.iter().find(|r| {
            let trimmed = r.trim_end_matches(')').trim_end();
            trimmed == "(allow file-read*"
                || r.as_str() == "(allow file-read*)"
                || r.starts_with("(allow file-read*)")
        });

        assert!(
            global_allow.is_none(),
            "AUDIT-HIGH-1: Seatbelt profile must NOT contain an unqualified \
             `(allow file-read*)` rule.  This single line voids read \
             isolation — a sandboxed process can read ~/.ssh, .env, and any \
             other user-readable file.  Found: {global_allow:?}\n\
             Full profile:\n{profile}"
        );
    }

    #[test]
    fn profile_with_explicit_read_path_does_not_also_emit_global_read() {
        // Even when callers supply per-path reads, the profile must not
        // additionally append a global allow.  This pins that the fix
        // actually removes the global rule rather than emitting both.
        use std::path::PathBuf;
        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_read(PathBuf::from("/tmp/workspace"))
            .build()
            .expect("policy");

        let profile = SeatbeltProfile::from_policy(&policy);

        // The per-path rule MUST still be present.
        assert!(
            profile.contains("/tmp/workspace"),
            "regression: per-path read for /tmp/workspace must remain in the profile"
        );

        // But the global one MUST NOT appear.
        for line in profile.lines() {
            let trimmed = line.trim();
            assert!(
                trimmed != "(allow file-read*)",
                "AUDIT-HIGH-1: profile contains unqualified `(allow file-read*)` \
                 even when explicit allow_read paths are provided.  Full profile:\n{profile}"
            );
        }
    }

    #[test]
    fn profile_still_allows_required_system_reads() {
        // Positive contract: the fix must not break sandboxed children.
        // macOS child processes need access to the dyld shared cache and
        // /dev/urandom or they SIGABRT during dynamic linking.  After the
        // fix, the profile must explicitly allow at least one of:
        //   - /usr/lib            (dyld shared cache via mmap)
        //   - /System/Library     (system frameworks)
        //   - /dev/urandom        (CSPRNG seeding for libc / TLS)
        //   - /private/var/db/dyld (dyld cache backing files)
        //
        // We accept any of these as evidence the implementer enumerated
        // system paths instead of just deleting the global rule.
        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .build()
            .expect("policy");

        let profile = SeatbeltProfile::from_policy(&policy);

        let has_required = profile.contains("/usr/lib")
            || profile.contains("/System/Library")
            || profile.contains("/dev/urandom")
            || profile.contains("/private/var/db/dyld");

        assert!(
            has_required,
            "AUDIT-HIGH-1: after removing the global file-read* rule, the \
             profile must enumerate at least one required system path \
             (/usr/lib, /System/Library, /dev/urandom, /private/var/db/dyld) \
             so child processes can dyld-link and access the CSPRNG.  \
             Otherwise every sandboxed binary will SIGABRT.\n\
             Full profile:\n{profile}"
        );
    }
}

// On non-macOS hosts the seatbelt module is `cfg`'d out entirely, so this
// file becomes an empty crate and `cargo test --workspace` continues to pass
// without scope creep.
#[cfg(not(target_os = "macos"))]
#[test]
fn seatbelt_tests_skipped_on_non_macos() {
    // No-op marker so the test binary is not entirely empty.
}
