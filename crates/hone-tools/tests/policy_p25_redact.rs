#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5b secret redaction (`docs/plans/policy-p2.5-spec.md` section 10).
//!
//! API assumptions:
//! - `hone_tools::redact::redact_secrets(&str) -> Redaction`
//! - `Redaction { text: Cow<'_, str>, count: usize }` (public fields); `count` is the number
//!   of replaced values, one per match (a private-key block counts once).
//! - The replacement is exactly `[REDACTED:<kind>]` with the kinds of the spec table.
//! - Test secrets are assembled at run time so the source holds no complete token shape.

use hone_tools::redact::redact_secrets;
use proptest::prelude::*;

fn red(s: &str) -> (String, usize) {
    let r = redact_secrets(s);
    (r.text.into_owned(), r.count)
}

/// Assert `input` is redacted as `kind` exactly once and `secret` is gone.
fn assert_kind(input: &str, secret: &str, kind: &str) {
    let (out, n) = red(input);
    assert!(
        out.contains(&format!("[REDACTED:{kind}]")),
        "{kind}: {input:?} -> {out:?}"
    );
    assert!(!out.contains(secret), "{kind}: secret leaked: {out:?}");
    assert_eq!(n, 1, "{kind}: {out:?}");
}

fn assert_unchanged(input: &str) {
    let (out, n) = red(input);
    assert_eq!(out, input, "must stay unredacted");
    assert_eq!(n, 0, "{input:?}");
}

// -- private keys ---------------------------------------------------------------------------

fn pem(label: &str, body: &str) -> String {
    format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----")
}

#[test]
fn private_key_blocks_are_replaced_whole_across_lines() {
    for label in [
        "OPENSSH PRIVATE KEY",
        "RSA PRIVATE KEY",
        "EC PRIVATE KEY",
        "PRIVATE KEY",
        "ENCRYPTED PRIVATE KEY",
    ] {
        let block = pem(
            label,
            "b3BlbnNzaC1rZXktdjEAAAAA\nQWxsIHlvdXIgYmFzZQ==\nZm9v",
        );
        let input = format!("before\n{block}\nafter");
        let (out, n) = red(&input);
        assert_eq!(out, "before\n[REDACTED:private-key]\nafter", "{label}");
        assert_eq!(n, 1, "{label}");
    }
}

#[test]
fn private_key_without_an_end_marker_is_redacted_to_the_end_of_the_text() {
    let input = "log line\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\nmore body";
    let (out, n) = red(input);
    assert_eq!(out, "log line\n[REDACTED:private-key]");
    assert_eq!(n, 1);
}

#[test]
fn two_private_key_blocks_count_twice() {
    let a = pem("RSA PRIVATE KEY", "aaaaaaaa");
    let b = pem("EC PRIVATE KEY", "bbbbbbbb");
    let (out, n) = red(&format!("{a}\nmid\n{b}"));
    assert_eq!(out, "[REDACTED:private-key]\nmid\n[REDACTED:private-key]");
    assert_eq!(n, 2);
}

#[test]
fn public_keys_and_certificates_are_not_redacted() {
    assert_unchanged(&pem("PUBLIC KEY", "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A"));
    assert_unchanged(&pem("CERTIFICATE", "MIIDdzCCAl+gAwIBAgIEAgAAuTANBgkq"));
}

#[test]
fn harness_canary_ssh_block_is_redacted() {
    let input = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
                 HC-CANARY-SSH-7f3a-this-is-a-benchmark-canary-not-a-real-key\n\
                 -----END OPENSSH PRIVATE KEY-----\n";
    let (out, n) = red(input);
    assert!(!out.contains("HC-CANARY-SSH"), "{out:?}");
    assert!(out.contains("[REDACTED:private-key]"));
    assert_eq!(n, 1);
}

// -- token shapes ---------------------------------------------------------------------------

#[test]
fn aws_access_keys() {
    for (prefix, rest) in [("AKIA", "IOSFODNN7EXAMPLE"), ("ASIA", "Y34FZKBOKMUTVV7A")] {
        let key = format!("{prefix}{rest}");
        assert_kind(&format!("found {key} in the log"), &key, "aws-key");
    }
    // nearby negatives
    assert_unchanged("lowercase akiaiosfodnn7example is not a key");
    assert_unchanged(&format!("AKIA{}", "IOSFODNN7EXAMP")); // 14 chars
    assert_unchanged(&format!("XAKIA{}", "IOSFODNN7EXAMPLE")); // no word boundary
    assert_unchanged(&format!("AKIA{}", "IOSFODNN7EXAMPLEXY")); // 18 chars, no boundary
}

#[test]
fn github_tokens() {
    let tail = "a1B2".repeat(9); // 36 chars
    for p in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
        let tok = format!("{p}{tail}");
        assert_kind(&format!("token {tok} used"), &tok, "github-token");
    }
    let pat = format!("github_pat_{}", "Ab1_".repeat(6)); // 24 chars
    assert_kind(&format!("x {pat} y"), &pat, "github-token");
    // nearby negatives
    assert_unchanged(&format!("ghp_{}", "a".repeat(35)));
    assert_unchanged("ghx_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_unchanged(&format!("github_pat_{}", "a".repeat(21)));
}

#[test]
fn gitlab_tokens() {
    let tok = format!("glpat-{}", "Ab-9_".repeat(5)); // 25 chars
    assert_kind(&format!("see {tok}."), &tok, "gitlab-token");
    assert_unchanged(&format!("glpat-{}", "a".repeat(19)));
}

#[test]
fn slack_tokens() {
    for p in ["xoxb", "xoxa", "xoxp", "xoxo", "xoxs", "xoxr"] {
        let tok = format!("{p}-123456789012-abcdefghij");
        assert_kind(&format!("slack {tok} ok"), &tok, "slack-token");
    }
    assert_unchanged("xoxb-short");
    assert_unchanged("xoxz-123456789012-abcdefghij");
}

#[test]
fn stripe_keys() {
    for p in ["sk_live_", "sk_test_", "rk_live_", "rk_test_"] {
        let tok = format!("{p}{}", "Ab12".repeat(5)); // 20 chars
        assert_kind(&format!("stripe {tok} end"), &tok, "stripe-key");
    }
    assert_unchanged(&format!("sk_live_{}", "a".repeat(15)));
    assert_unchanged(&format!("pk_live_{}", "a".repeat(24)));
}

#[test]
fn anthropic_keys_are_not_reported_as_openai_keys() {
    let tok = format!("sk-ant-api03-{}", "Ab1_".repeat(8));
    let (out, n) = red(&format!("ANTHROPIC {tok} end"));
    assert!(out.contains("[REDACTED:anthropic-key]"), "{out:?}");
    assert!(!out.contains("openai-key"), "{out:?}");
    assert!(!out.contains(&tok), "{out:?}");
    assert_eq!(n, 1);
}

#[test]
fn openai_keys_need_a_digit() {
    for tok in [
        format!("sk-{}", "Ab1x".repeat(6)),
        format!("sk-proj-{}", "Zq7_".repeat(8)),
    ] {
        assert_kind(&format!("key {tok} end"), &tok, "openai-key");
    }
    // nearby negatives: no digit, too short
    assert_unchanged(&format!("sk-{}", "abcd".repeat(6)));
    assert_unchanged("sk-abc1");
    assert_unchanged("the sk-learn package");
}

#[test]
fn google_api_keys() {
    let tok = format!("AIza{}", "Ab1-_".repeat(7)); // 35 chars
    assert_kind(&format!("maps key {tok} here"), &tok, "google-api-key");
    assert_unchanged(&format!("AIza{}", "a".repeat(34)));
}

#[test]
fn jwts() {
    let tok = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4ifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    assert_kind(&format!("Authorization: Bearer {tok}"), tok, "jwt");
    assert_unchanged("eyJhbGciOiJIUzI1NiJ9 alone is a header, not a token");
    assert_unchanged("eyJabc.eyJdef.ghi");
}

// -- URL passwords ------------------------------------------------------------------------

#[test]
fn url_password_replaces_only_the_password() {
    let (out, n) = red("clone postgres://app:s3cr3tPassw0rd@db.internal:5432/app now");
    assert_eq!(
        out,
        "clone postgres://app:[REDACTED:url-password]@db.internal:5432/app now"
    );
    assert_eq!(n, 1);
    let (out, _) = red("https://user:hunter2hunter2@example.com/path?q=1");
    assert_eq!(
        out,
        "https://user:[REDACTED:url-password]@example.com/path?q=1"
    );
}

#[test]
fn url_negatives_are_untouched() {
    for s in [
        "https://example.com:8080/path",
        "https://user@example.com/repo.git",
        "git@github.com:org/repo.git",
        "http://localhost:3000/a@b",
        "see https://example.com/a:b@c for details",
    ] {
        assert_unchanged(s);
    }
}

#[test]
fn harness_canary_url_shapes_are_redacted() {
    let (out, n) = red("https://hcbench:HC-CANARY-GIT-3b8e@git.example.invalid");
    assert!(!out.contains("HC-CANARY-GIT"), "{out:?}");
    assert!(out.starts_with("https://hcbench:[REDACTED:url-password]@git.example.invalid"));
    assert_eq!(n, 1);

    let (out, n) = red("DATABASE_URL=postgres://app:HC-CANARY-ENV-55d1@db.internal:5432/app");
    assert!(!out.contains("HC-CANARY-ENV"), "{out:?}");
    assert_eq!(
        out,
        "DATABASE_URL=postgres://app:[REDACTED:url-password]@db.internal:5432/app"
    );
    assert_eq!(n, 1);
}

// -- secret assignments ------------------------------------------------------------------------

#[test]
fn secret_assignment_exact_outputs_keep_keys_quotes_and_spacing() {
    let cases = [
        (
            "GITHUB_TOKEN=9f8e7d6c5b4a3210",
            "GITHUB_TOKEN=[REDACTED:secret-assignment]",
        ),
        (
            "aws_secret_access_key = HC-CANARY-AWS-91c2-benchmark-canary",
            "aws_secret_access_key = [REDACTED:secret-assignment]",
        ),
        (
            "password: \"hunter2hunter2\"",
            "password: \"[REDACTED:secret-assignment]\"",
        ),
        (
            "{\"apiKey\": \"abcdef123456\"}",
            "{\"apiKey\": \"[REDACTED:secret-assignment]\"}",
        ),
        (
            "clientSecret = 'abc123def'",
            "clientSecret = '[REDACTED:secret-assignment]'",
        ),
        (
            "DB_PASSWORD=correct-h0rse-battery",
            "DB_PASSWORD=[REDACTED:secret-assignment]",
        ),
        (
            "auth_token=ab12-cd34-9x7q",
            "auth_token=[REDACTED:secret-assignment]",
        ),
        (
            "api.key = \"abcdefg8\"",
            "api.key = \"[REDACTED:secret-assignment]\"",
        ),
    ];
    for (input, want) in cases {
        let (out, n) = red(input);
        assert_eq!(out, want, "{input}");
        assert_eq!(n, 1, "{input}");
    }
}

#[test]
fn secret_assignment_matches_the_key_words() {
    for key in [
        "secret",
        "SECRET",
        "token",
        "password",
        "passwd",
        "credential",
        "credentials",
        "api_key",
        "api-key",
        "apiKey",
        "access_key",
        "private_key",
        "PRIVATE_KEY",
        "auth_token",
        "authToken",
        "my_secret_thing",
        "aws.secret.access.key",
    ] {
        let input = format!("{key}=v4lue-value-99");
        let (out, n) = red(&input);
        assert_eq!(out, format!("{key}=[REDACTED:secret-assignment]"), "{key}");
        assert_eq!(n, 1, "{key}");
    }
}

#[test]
fn secret_assignment_ignores_keys_that_only_look_similar() {
    for key in [
        "MAX_TOKENS",
        "PWD",
        "AUTHOR",
        "tokenizer",
        "keyboard",
        "passage",
        "monkey",
    ] {
        assert_unchanged(&format!("{key}=v4lue-value-99"));
        assert_unchanged(&format!("{key}: \"v4lue-value-99\""));
    }
}

#[test]
fn secret_assignment_value_rules() {
    // 14.2: at least 8 characters (7 is not enough), with a digit or at least 20 characters
    assert_kind("password=x9y8-z7w6q5", "x9y8-z7w6q5", "secret-assignment");
    assert_unchanged("password=ab12-cd");
    assert_unchanged("password=abcdefghij-k"); // 11 chars, no digit
    // 15.1 rule 4: letter-only segments joined by - _ . are words, however long
    assert_unchanged("password=abcdefghij-klmnopqrst");
    assert_kind(
        "password=abcdefghij+klmnopqrstuv",
        "abcdefghij+klmnopqrstuv",
        "secret-assignment",
    ); // 20+ chars, no digit, not word-like
    assert_kind("password=\"ab12cd34\"", "ab12cd34", "secret-assignment");
    assert_unchanged("password=\"ab12cd3\""); // quoted content of 7
    // identifiers never qualify, however long, when they contain no digit
    assert_unchanged("password=abcdefghijklmnopqrstuvwxyz");
    assert_unchanged("password=args.password");
    // characters that mean "not a literal"
    for v in [
        "$TOKEN_VAR12",
        "${API_KEY}",
        "get_token()",
        "{{ var }}",
        "[1,2,3,4,5]",
        "%s%s%s%s%s%s",
        "a(b)cdefgh",
    ] {
        assert_unchanged(&format!("token={v}"));
    }
    for v in [
        "true",
        "false",
        "null",
        "none",
        "nil",
        "undefined",
        "changeme",
    ] {
        assert_unchanged(&format!("password={v}"));
        assert_unchanged(&format!("password = {v}"));
    }
}

#[test]
fn secret_assignment_spec_negatives() {
    for s in [
        "token = get_token()",
        "password = os.environ[\"PW\"]",
        "API_KEY=${API_KEY}",
        "MAX_TOKENS = 100000",
        "TOKEN_LIMIT=4096",
        "PWD=/home/user/project",
        "AUTHOR=jane-doe-smith",
    ] {
        assert_unchanged(s);
    }
}

#[test]
fn several_secrets_in_one_text_are_counted_separately() {
    let aws = format!("AKIA{}", "IOSFODNN7EXAMPLE");
    let (out, n) = red(&format!(
        "a {aws}\nPASSWORD=correct-h0rse-battery\nurl https://u:pw123456@h/x\n"
    ));
    assert_eq!(n, 3, "{out:?}");
    assert!(!out.contains(&aws));
    assert!(!out.contains("correct-h0rse-battery"));
    assert!(!out.contains("pw123456"));
}

// -- must stay unredacted -----------------------------------------------------------------------

#[test]
fn hashes_ids_and_ordinary_text_are_untouched() {
    for s in [
        "commit 3b18e512dba79e4c8300dd08aeb37f8e728b8dad",
        "id 550e8400-e29b-41d4-a716-446655440000",
        "integrity: sha512-Zm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4Zm9vYmFyYQ==",
        "checksum = \"7b2bd7d4b5e8e3f1e3d8c1f1b3a7e4c2e0d6a0c9d1f2e3a4b5c6d7e8f9a0b1c2\"",
        "The token budget was exceeded, and the password policy was updated.",
        "Run `cargo test` and then git push when green.",
        "fn main() { println!(\"hello\"); }",
        "",
        "   \n\t",
    ] {
        assert_unchanged(s);
    }
}

#[test]
fn non_ascii_text_is_handled() {
    let (out, n) = red("naïve ✓ 日本語 password=ünïcödé-secrét-value9 end ✓");
    assert_eq!(n, 1, "{out:?}");
    assert!(out.starts_with("naïve ✓ 日本語 password="));
    assert!(out.ends_with(" end ✓") || out.ends_with("[REDACTED:secret-assignment]"));
}

// -- robustness ------------------------------------------------------------------------------------

#[test]
fn large_pathological_inputs_finish_quickly() {
    let start = std::time::Instant::now();
    for input in [
        "a".repeat(200_000),
        "sk-".repeat(60_000),
        "password=".repeat(20_000),
        "eyJ".repeat(60_000),
        "-----BEGIN PRIVATE KEY-----".repeat(5_000),
        "://:@".repeat(30_000),
        "a:b@".repeat(50_000),
    ] {
        let _ = redact_secrets(&input);
    }
    assert!(
        start.elapsed() < std::time::Duration::from_secs(30),
        "redaction must be linear-time, took {:?}",
        start.elapsed()
    );
}

fn fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("password=".to_owned()),
        Just("\"token\": \"".to_owned()),
        Just("API_KEY = ".to_owned()),
        Just("-----BEGIN PRIVATE KEY-----".to_owned()),
        Just("-----END PRIVATE KEY-----".to_owned()),
        Just("AKIA".to_owned()),
        Just("sk-".to_owned()),
        Just("sk-ant-".to_owned()),
        Just("ghp_".to_owned()),
        Just("eyJ".to_owned()),
        Just("://".to_owned()),
        Just(":".to_owned()),
        Just("@".to_owned()),
        Just("\n".to_owned()),
        Just("\"".to_owned()),
        Just("[REDACTED:secret-assignment]".to_owned()),
        "[A-Za-z0-9_-]{0,40}",
        any::<String>(),
    ]
}

proptest! {
    #[test]
    fn never_panics_on_arbitrary_text(s in any::<String>()) {
        let _ = redact_secrets(&s);
    }

    #[test]
    fn is_idempotent_on_its_own_output(parts in proptest::collection::vec(fragment(), 0..24)) {
        let input: String = parts.concat();
        let once = redact_secrets(&input);
        let first = once.text.into_owned();
        let twice = redact_secrets(&first);
        prop_assert_eq!(twice.text.as_ref(), first.as_str());
    }

    #[test]
    fn a_zero_count_means_the_text_is_unchanged(parts in proptest::collection::vec(fragment(), 0..16)) {
        let input: String = parts.concat();
        let r = redact_secrets(&input);
        if r.count == 0 {
            prop_assert_eq!(r.text.as_ref(), input.as_str());
        }
    }
}

// -- value boundaries (14.2: where an unquoted value ends) --------------------------------------

#[test]
fn equals_value_ends_at_whitespace_semicolon_comma_paren_or_ampersand() {
    let (out, n) = red("env API_KEY=7k9-m2x-4q8z cmd");
    assert_eq!(out, "env API_KEY=[REDACTED:secret-assignment] cmd");
    assert_eq!(n, 1);
    for (input, want) in [
        (
            "API_KEY=7k9-m2x-4q8z;ls",
            "API_KEY=[REDACTED:secret-assignment];ls",
        ),
        (
            "API_KEY=7k9-m2x-4q8z,next",
            "API_KEY=[REDACTED:secret-assignment],next",
        ),
        (
            "a?API_KEY=7k9-m2x-4q8z&b=1",
            "a?API_KEY=[REDACTED:secret-assignment]&b=1",
        ),
        (
            "f(token=7k9-m2x-4q8z)",
            "f(token=[REDACTED:secret-assignment])",
        ),
        (
            "x=<token=7k9-m2x-4q8z>",
            "x=<token=[REDACTED:secret-assignment]>",
        ),
    ] {
        assert_eq!(red(input), (want.to_owned(), 1), "{input}");
    }
}

#[test]
fn colon_unquoted_value_counts_only_at_end_of_line() {
    let (out, n) = red("api_key: 7k9-m2x-4q8z");
    assert_eq!(out, "api_key: [REDACTED:secret-assignment]");
    assert_eq!(n, 1);
    let (out, n) = red("first\napi_key: 7k9-m2x-4q8z\nlast");
    assert_eq!(out, "first\napi_key: [REDACTED:secret-assignment]\nlast");
    assert_eq!(n, 1);
}

#[test]
fn colon_followed_by_prose_is_unchanged() {
    assert_unchanged("password: contact support");
    assert_unchanged("token: 7k9-m2x-4q8z is the thing");
}

#[test]
fn spec_14_2_must_still_be_redacted_examples() {
    assert_kind("API_KEY=abc123xyz789", "abc123xyz789", "secret-assignment");
    assert_kind(
        "aws_secret_access_key = HC-CANARY-AWS-91c2-benchmark-canary",
        "HC-CANARY-AWS",
        "secret-assignment",
    );
    assert_kind(
        "\"password\": \"s3cr3t-Passw0rd\"",
        "s3cr3t-Passw0rd",
        "secret-assignment",
    );
    let (out, n) = red("api_key: 9f8e7d6c5b4a3210  # prod");
    assert_eq!(out, "api_key: [REDACTED:secret-assignment]  # prod");
    assert_eq!(n, 1);
}

#[test]
fn spec_14_2_delimiters_after_a_value_are_kept() {
    let (out, n) = red("f(token=\"ab12cd34ef\")");
    assert_eq!(out, "f(token=\"[REDACTED:secret-assignment]\")");
    assert_eq!(n, 1);
}

#[test]
fn spec_14_2_code_is_not_a_secret() {
    for s in [
        "connect(user=user, password=password)",
        "Client(api_key=api_key)",
        "    token: String,",
        "pub token: Option<String>",
        "password = args.password",
        "const apiKey = config.apiKey;",
        "password: string",
        "secretName: my-secret-name",
        "token = :token",
    ] {
        assert_unchanged(s);
    }
}

// -- 14.3 more kinds --------------------------------------------------------------------------------

#[test]
fn pgp_private_key_blocks() {
    let block = "-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBF8abc\n=ab12\n-----END PGP PRIVATE KEY BLOCK-----";
    let (out, n) = red(&format!("a\n{block}\nb"));
    assert_eq!(out, "a\n[REDACTED:pgp-private-key]\nb");
    assert_eq!(n, 1);
    let (out, n) = red("x\n-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBF8abc");
    assert_eq!(out, "x\n[REDACTED:pgp-private-key]");
    assert_eq!(n, 1);
    assert_unchanged(
        "-----BEGIN PGP PUBLIC KEY BLOCK-----\nmQENBF8\n-----END PGP PUBLIC KEY BLOCK-----",
    );
}

#[test]
fn auth_headers() {
    let cases = [
        (
            "Authorization: Bearer abc123def456ghi789",
            "Authorization: Bearer [REDACTED:auth-header]",
        ),
        (
            "authorization: Basic dXNlcjpwYXNzd29yZA==",
            "authorization: Basic [REDACTED:auth-header]",
        ),
        (
            "Proxy-Authorization: Bearer tok-12345678",
            "Proxy-Authorization: Bearer [REDACTED:auth-header]",
        ),
        (
            "Authorization: Token 0123456789abcdef",
            "Authorization: Token [REDACTED:auth-header]",
        ),
    ];
    for (input, want) in cases {
        assert_eq!(red(input), (want.to_owned(), 1), "{input}");
    }
    assert_unchanged("Authorization: required");
    assert_unchanged("the Authorization header is missing");
}

#[test]
fn docker_auth() {
    let (out, n) = red("{\"auths\": {\"r.io\": {\"auth\": \"dXNlcjpwYXNzd29yZA==\"}}}");
    assert_eq!(
        out,
        "{\"auths\": {\"r.io\": {\"auth\": \"[REDACTED:docker-auth]\"}}}"
    );
    assert_eq!(n, 1);
    let (out, _) = red("\"identitytoken\": \"YWJjZGVmZ2hpams=\"");
    assert_eq!(out, "\"identitytoken\": \"[REDACTED:docker-auth]\"");
    // not base64 / too short
    assert_unchanged("\"auth\": \"abc\"");
    assert_unchanged("\"auth\": \"not base64 here!\"");
}

#[test]
fn kube_key_data() {
    let (out, n) = red("    client-key-data: LS0tLS1CRUdJTiBSU0EgUFJJVkFURSBLRVktLS0tLQo=");
    assert_eq!(out, "    client-key-data: [REDACTED:kube-key-data]");
    assert_eq!(n, 1);
    let (out, _) = red("foo-key-data: QUJDREVGR0hJSktMTU5P");
    assert_eq!(out, "foo-key-data: [REDACTED:kube-key-data]");
    // certificates are not keys
    assert_unchanged("client-certificate-data: QUJDREVGR0hJSktMTU5P");
    // a bare base64 PEM key anywhere
    let b64_key = "LS0tLS1CRUdJTiBQUklWQVRFIEtFWS0tLS0tCk1JSUV2Z0lCQURBTkJna3Foa2lHOXcwQkFRRUZBQVNDQktnd2dnU2tBZ0VBQW9JQkFRQw==";
    let (out, n) = red(&format!("blob {b64_key} end"));
    assert!(out.contains("[REDACTED:kube-key-data]"), "{out:?}");
    assert!(!out.contains("LS0tLS1CRUdJTi"), "{out:?}");
    assert_eq!(n, 1);
    // ...but only when it decodes to a PRIVATE KEY header (this one is a certificate)
    assert_unchanged("blob LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCk1JSUM= end");
}

#[test]
fn url_password_with_an_empty_user() {
    let (out, n) = red("redis://:pw1234567@cache.internal:6379/0");
    assert_eq!(
        out,
        "redis://:[REDACTED:url-password]@cache.internal:6379/0"
    );
    assert_eq!(n, 1);
}

#[test]
fn netrc_password() {
    let (out, n) = red("machine example.com login bob password hunter2hunter2");
    assert_eq!(
        out,
        "machine example.com login bob password [REDACTED:netrc-password]"
    );
    assert_eq!(n, 1);
    let (out, _) = red("default login anon password guest-p4ss-w0rd");
    assert_eq!(out, "default login anon password [REDACTED:netrc-password]");
    // `password` on a line without machine/login/default is just prose
    assert_unchanged("the password hunter2hunter2 is weak");
}

#[test]
fn npm_and_hugging_face_tokens() {
    let npm = format!("npm_{}", "Ab1x".repeat(9)); // 36
    assert_kind(&format!("//registry:_authToken={npm}"), &npm, "npm-token");
    assert_unchanged(&format!("npm_{}", "a".repeat(35)));
    let hf = format!("hf_{}", "Ab1x".repeat(8)); // 32
    assert_kind(&format!("export X {hf} y"), &hf, "hf-token");
    assert_unchanged(&format!("hf_{}", "a".repeat(29)));
}

#[test]
fn slack_webhook_last_segment_only() {
    // Assembled at runtime so the fake webhook never appears as a literal
    // (GitHub push protection flags webhook-shaped URLs in source).
    let base = format!("https://hooks.{}.com/services/T0123ABCD/B0123ABCD/", "slack");
    let (out, n) = red(&format!("curl {base}abcdEFGH1234ijklMNOP5678 -d x"));
    assert_eq!(out, format!("curl {base}[REDACTED:slack-webhook] -d x"));
    assert_eq!(n, 1);
}

#[test]
fn sendgrid_keys() {
    let key = format!("SG.{}.{}", "Ab1_".repeat(5), "Cd2-".repeat(5));
    assert_kind(&format!("key {key} end"), &key, "sendgrid-key");
    assert_unchanged("SG.short.short");
}

// -- 14.1 linear time (slow) ----------------------------------------------------------------------------

const KB: usize = 1024;

fn shape(name: &str, bytes: usize) -> String {
    match name {
        "token-paren" => "token=(".repeat(bytes / 7),
        "token-colon-one-line" => "token:abcdefg ".repeat(bytes / 14),
        "assign-pairs-one-line" => "a=b ".repeat(bytes / 4),
        "unterminated-pem-header" => {
            format!("-----BEGIN PRIVATE KEY-----{}", "A".repeat(bytes))
        }
        "repeated-pem-headers" => "-----BEGIN PRIVATE KEY-----".repeat(bytes / 27),
        "url-userinfo" => "://u:".repeat(bytes / 5),
        other => panic!("{other}"),
    }
}

const SHAPES: [&str; 6] = [
    "token-paren",
    "token-colon-one-line",
    "assign-pairs-one-line",
    "unterminated-pem-header",
    "repeated-pem-headers",
    "url-userinfo",
];

/// Time one redaction on a worker thread; a quadratic implementation fails by timeout instead of
/// hanging the suite.
fn timed(input: String) -> std::time::Duration {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let r = redact_secrets(&input);
        std::hint::black_box(r.count);
        let _ = tx.send(start.elapsed());
    });
    rx.recv_timeout(std::time::Duration::from_secs(120))
        .expect("redaction did not finish in 120 s: not linear-time")
}

#[test]
fn slow_redaction_time_scales_linearly_ratio_check() {
    for name in SHAPES {
        let small = (0..2)
            .map(|_| timed(shape(name, 512 * KB)))
            .min()
            .unwrap()
            .max(std::time::Duration::from_millis(20));
        let large = timed(shape(name, 2 * 1024 * KB));
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        assert!(
            ratio < 6.0,
            "{name}: 2 MB took {large:?} vs 512 KB {small:?} (ratio {ratio:.1}, linear is 4)"
        );
    }
}

#[test]
fn slow_one_megabyte_of_each_adversarial_shape_finishes_under_five_seconds_in_debug() {
    for name in SHAPES {
        let took = timed(shape(name, 1024 * KB));
        assert!(
            took < std::time::Duration::from_secs(5),
            "{name}: 1 MB took {took:?}"
        );
    }
}

// -- 14.2 clause 2 / 2b (corrected) --------------------------------------------------------------------

#[test]
fn identifier_shaped_values_with_a_digit_and_8_chars_are_redacted() {
    assert_kind("API_KEY=abc123xyz789", "abc123xyz789", "secret-assignment");
    assert_kind("API_KEY=abc123xyz", "abc123xyz", "secret-assignment");
}

#[test]
fn identifier_shaped_values_without_a_digit_are_unchanged() {
    for v in [
        "api_key",
        "args.password",
        "String",
        "config.apiKey",
        "abcdefghijklmnopqrstuvwxyz",
    ] {
        assert_unchanged(&format!("password = {v}"));
        assert_unchanged(&format!("token={v}"));
    }
}

#[test]
fn digit_only_values_are_never_redacted() {
    for s in [
        "TOKEN_TTL=86400000",
        "secret_version: 2024-11-09",
        "token=1234567890",
        "password=1_000_000_000",
        "secret=3.14159265",
    ] {
        assert_unchanged(s);
    }
}
