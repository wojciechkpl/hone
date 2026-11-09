#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 section 15.1: every value-capturing redaction kind checks plausibility.
//!
//! The cases are the round-2 reviewer's reproducers (`rc1.txt`, `rc2.txt`, `rc3.txt`) plus the
//! spec's own "must stay unchanged" / "must still be redacted" lists. Code, templates, prose,
//! URLs and paths are never secrets; real-looking values still are.

use hone_tools::redact::redact_secrets;

fn red(s: &str) -> (String, usize) {
    let r = redact_secrets(s);
    (r.text.into_owned(), r.count)
}

fn assert_unchanged(input: &str) {
    let (out, n) = red(input);
    assert_eq!(out, input, "must stay unredacted");
    assert_eq!(n, 0, "{input:?}");
}

fn assert_redacted(input: &str, secret: &str) {
    let (out, n) = red(input);
    assert!(!out.contains(secret), "secret leaked: {input:?} -> {out:?}");
    assert!(n >= 1, "{input:?} -> {out:?}");
}

// -- templates and references (15.1 rule 2) ---------------------------------------------------

#[test]
fn url_password_templates_are_not_secrets() {
    for s in [
        "url = f\"https://{user}:{password}@{host}/db\"",
        "DATABASE_URL: postgres://${POSTGRES_USER}:${POSTGRES_PASSWORD}@db:5432/app",
        "const url = `postgres://${user}:${pass}@${host}`;",
        "dsn = \"mysql://%(user)s:%(password)s@%(host)s/db\"",
        "conn_str = \"postgresql://{}:{}@{}/{}\".format(u, p, h, d)",
        "postgres://${A}:${B}@h",
        "postgres://{user}:{password}@h",
        "postgres://{}:{}@h",
    ] {
        assert_unchanged(s);
    }
}

#[test]
fn auth_header_templates_are_not_secrets() {
    for s in [
        "curl -H \"Authorization: Bearer ${{ secrets.GITHUB_TOKEN }}\" https://api.github.com",
        "curl -H \"Authorization: Bearer $TOKEN\" https://x",
        "header = fmt.Sprintf(\"Authorization: Bearer %s\", token)",
        "h = f\"Authorization: Bearer {token}\"",
        "req.headers['Authorization: Bearer ' + tok]",
        "# Authorization: Bearer <token>",
        "Authorization: Bearer ${TOKEN}",
        "Authorization: Bearer %(token)s",
    ] {
        assert_unchanged(s);
    }
}

#[test]
fn ci_and_vault_templates_are_not_secrets() {
    for s in [
        "token: ${{ secrets.NPM_TOKEN }}",
        "env:\n  API_KEY: ${{ secrets.API_KEY }}",
        "secret: !vault |\n          $ANSIBLE_VAULT;1.1;AES256",
        "--password=$(cat pw.txt)",
        "password2 = password_v2",
    ] {
        assert_unchanged(s);
    }
}

// -- prose, URLs, paths, URNs, word-like identifiers (rules 4 and 5) -------------------------------

#[test]
fn prose_and_ui_strings_are_not_secrets() {
    for s in [
        "\"password_too_short\": \"Password must be at least 8 characters\",",
        "\"token_expired\": \"Your session expired after 30 minutes\",",
        "  passwordHint: 'Use 12 or more characters',",
        "password_min_length = 12",
        "<input type=\"password\" placeholder=\"Password (min 8 chars)\">",
        "# login with the password stored in vault",
        "Enter your login and password below to continue.",
        "The default password is changed on first login.",
        "login(user, password) -> session",
        "if not login: raise ValueError(\"password missing\")",
        "Enter your login and password below",
    ] {
        assert_unchanged(s);
    }
}

#[test]
fn urls_paths_and_urns_are_not_secrets() {
    for s in [
        "TOKEN_URL = \"https://oauth2.googleapis.com/token\"",
        "token_endpoint: https://login.example.com/oauth2/v2.0/token",
        "secret_path: /var/run/secrets/kubernetes.io/serviceaccount/token",
        "credentials_file: ~/.aws/credentials2",
        "password_file = \"/etc/app/db_password_v2.txt\"",
        "secret_path: ./secrets/token.txt",
        "secret_path: ../secrets/token.txt",
        "token_type: urn:ietf:params:oauth:token-type:access_token",
        "token_type: urn:ietf:params:oauth:token-type:jwt",
        "SECRET_KEY_FALLBACKS = [\"old-key-1\"]",
    ] {
        assert_unchanged(s);
    }
}

#[test]
fn words_that_only_look_like_identifiers_are_not_secrets() {
    for s in [
        "AUTH_TOKEN_HEADER: \"X-Auth-Token-V2\"",
        "api_key_env_var: \"OPENAI_API_KEY_2\"",
        "token_name = db_password_v2",
        "token_name: X-Auth-Token-V2",
    ] {
        assert_unchanged(s);
    }
}

#[test]
fn values_with_non_word_segments_still_qualify() {
    let (out, n) = red("aws_secret_access_key = HC-CANARY-AWS-91c2-benchmark-canary");
    assert_eq!(out, "aws_secret_access_key = [REDACTED:secret-assignment]");
    assert_eq!(n, 1);
    assert_redacted("password: s3cr3t-Passw0rd", "s3cr3t-Passw0rd");
}

// -- real values are still redacted (and delimiters kept) -----------------------------------------------

#[test]
fn trailing_comments_and_closing_quotes_are_kept() {
    let cases = [
        (
            "token = \"abcdefgh12\" # placeholder",
            "token = \"[REDACTED:secret-assignment]\" # placeholder",
        ),
        (
            "password: hunter2hunter2 # comment",
            "password: [REDACTED:secret-assignment] # comment",
        ),
        (
            "curl -H \"Authorization: Bearer abc123def456ghi789\" https://x",
            "curl -H \"Authorization: Bearer [REDACTED:auth-header]\" https://x",
        ),
        (
            "Authorization: Bearer abc123def456ghi789",
            "Authorization: Bearer [REDACTED:auth-header]",
        ),
    ];
    for (input, want) in cases {
        assert_eq!(red(input), (want.to_owned(), 1), "{input}");
    }
}

#[test]
fn json_and_dict_style_auth_headers_are_redacted() {
    let cases = [
        (
            "{\"Authorization\": \"Bearer abc123def456ghi789\"}",
            "{\"Authorization\": \"Bearer [REDACTED:auth-header]\"}",
        ),
        (
            "{'Authorization': 'Bearer abc123def456ghi789'}",
            "{'Authorization': 'Bearer [REDACTED:auth-header]'}",
        ),
    ];
    for (input, want) in cases {
        assert_eq!(red(input), (want.to_owned(), 1), "{input}");
    }
    // the token must pass the same checks: templates stay
    assert_unchanged("{\"Authorization\": \"Bearer ${TOKEN}\"}");
    assert_unchanged("{'Authorization': 'Bearer {token}'}");
}

#[test]
fn secrets_inside_quotes_urls_and_keys_are_redacted() {
    assert_redacted("api_key = \"AKIAIOSFODNN7EXAMPLE\"", "AKIAIOSFODNN7EXAMPLE");
    let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
    let (out, n) = red(&format!("x = \"{jwt}\""));
    assert_eq!(out, "x = \"[REDACTED:jwt]\"");
    assert_eq!(n, 1);
    assert_redacted("X-Api-Key: abc123def456ghi789", "abc123def456ghi789");
    assert_redacted(
        "aws_session_token = IQoJb3JpZ2luX2VjEHcaCXVzLWVhc3QtMSJHMEUCIQ",
        "IQoJb3JpZ2luX2VjEHcaCXVzLWVhc3QtMSJHMEUCIQ",
    );
    assert_redacted("API_KEY=abc123xyz789", "abc123xyz789");
}

// -- docker auth (decodes to user:pass) -----------------------------------------------------------------------

#[test]
fn docker_auth_needs_base64_of_user_colon_pass() {
    // base64("user:password")
    let (out, n) = red("{\"auth\": \"dXNlcjpwYXNzd29yZA==\"}");
    assert_eq!(out, "{\"auth\": \"[REDACTED:docker-auth]\"}");
    assert_eq!(n, 1);
    // base64("justonetoken12"): printable but no ':'
    assert_unchanged("{\"auth\": \"anVzdG9uZXRva2VuMTI=\"}");
    assert_unchanged("{\"auth\": \"required\"}");
    assert_unchanged("{\"auth\": \"required\", \"mode\": \"strict\"}");
    assert_unchanged("{\"auth\": \"disabled\"}");
    assert_unchanged("\"identitytoken\": \"something-long\"");
}

// -- netrc grammar across lines ----------------------------------------------------------------------------------

#[test]
fn multi_line_netrc_is_redacted() {
    let input = "machine api.heroku.com\n  login me@example.com\n  password 01234567-89ab-cdef-0123-456789abcdef";
    let (out, n) = red(input);
    assert_eq!(
        out,
        "machine api.heroku.com\n  login me@example.com\n  password [REDACTED:netrc-password]"
    );
    assert_eq!(n, 1);
}

#[test]
fn netrc_value_belongs_to_the_nearest_machine_or_default() {
    let input = "machine a.example login x\nmachine b.example\n  password s3cr3t-pa55w0rd\n";
    let (out, n) = red(input);
    assert!(!out.contains("s3cr3t-pa55w0rd"), "{out:?}");
    assert!(out.contains("[REDACTED:netrc-password]"), "{out:?}");
    assert_eq!(n, 1);
    let (out, _) = red("default\n  login anon\n  password guest-p4ss-w0rd\n");
    assert!(!out.contains("guest-p4ss-w0rd"), "{out:?}");
}

#[test]
fn a_blank_line_ends_a_netrc_entry() {
    assert_unchanged("machine a.example\n  login me\n\nthe password hunter2hunter2 is prose");
}

#[test]
fn a_line_that_merely_mentions_login_or_default_is_not_netrc() {
    assert_unchanged("Enter your login and password hunter2hunter2 below");
    assert_unchanged("if not login: raise ValueError(\"password missing\")");
}

// -- kube-key-data wrapped at 76 columns ----------------------------------------------------------------------

const B64: &str = "LS0tLS1CRUdJTiBPUEVOU1NIIFBSSVZBVEUgS0VZLS0tLS0KSXBIWXpjTVFRUjUrd25ONHBtSEpOUmg4QitUVlkyNmJ3OFFBc25KRXVNMDZsL0VhNWxFSEJRYW1pZ0x3NFdHdk4vaHN1UQplSE9NTnc4SDZOTzFnN3JUakNkZk5LN1FWcTF1cU83S1FaTDZIK3VkeExIcjVWNWJqNXRvRHY5MnlCMU9tck1FMUlsdm5oCmY5andnV1NXMmdoNlByN01aMnFxTEYyTTRiUEdyTHhmRm5DcGdodkhLWVhYWkY1OXV3ZDRDMDYwMmZ1ZGw1UmtwU3NyZ0QKcjdBOFV6aXV2Y2pEdG5nMWp6MkpOYWRlaEVxSXliOWJvQllzamIwdlRpOEwyRHp5R0V4NDgwYmZNT2U5NWRrWTB6OElGcApmTkJiYWxnQWlZcWZ5WnhVZFprSHpUcWlMWXlWTHR3WHpJM00yZEh1UVFqWDhhd1NGZDRFY3dQQndVYy9SQnpNbnk5WVNoCkVxS0VHSDh5dW9SYVcyUzNTelVuOTVIUVpQWWxkcnl6QkNHMERtdW9MNk5mZWJidEg1QlRrRVpTVUp1UFVwY3JTQnJXMkwKMVRqNithSE1zWVJ6T1lhbUIyV3NrODFTcUtGdEQ3eE1JUGMyNEF4T0V0c1RUK3J3VEw0b2FwQkFJUUtQNE5rSmw5RTM5dQphUmRTdlQzdDc1eDdTZmdnbGdNMWdaTkpLczVXNlhNWDRhOEtwalM0Ri9CQT09Ci0tLS0tRU5EIE9QRU5TU0ggUFJJVkFURSBLRVktLS0tLQo=";

fn wrap76(s: &str) -> Vec<String> {
    s.as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8(c.to_vec()).unwrap())
        .collect()
}

#[test]
fn a_76_column_wrapped_base64_pem_is_redacted_whole() {
    let lines = wrap76(B64);
    assert!(lines.len() > 3);
    let input = format!("before\n{}\nafter: not base64 here", lines.join("\n"));
    let (out, _) = red(&input);
    for l in &lines {
        assert!(!out.contains(l.as_str()), "line leaked: {l}");
    }
    assert!(out.contains("[REDACTED:kube-key-data]"), "{out:?}");
    assert!(out.starts_with("before\n"), "{out:?}");
    assert!(out.ends_with("after: not base64 here"), "{out:?}");
}

#[test]
fn an_unwrapped_base64_pem_is_still_redacted() {
    let (out, n) = red(&format!("blob {B64} end"));
    assert_eq!(out, "blob [REDACTED:kube-key-data] end");
    assert_eq!(n, 1);
}

#[test]
fn client_key_data_with_continuation_lines_is_redacted_whole() {
    let lines = wrap76(B64);
    let input = format!(
        "  client-key-data: {}\n{}\n  next-key: plain value",
        lines[0],
        lines[1..].join("\n")
    );
    let (out, _) = red(&input);
    for l in &lines {
        assert!(!out.contains(l.as_str()), "line leaked: {l}");
    }
    assert!(
        out.contains("client-key-data: [REDACTED:kube-key-data]"),
        "{out:?}"
    );
    assert!(out.ends_with("  next-key: plain value"), "{out:?}");
}

#[test]
fn a_non_base64_line_ends_the_block() {
    let lines = wrap76(B64);
    let input = format!("{}\nthis line has spaces\n{}", lines[0], lines[1]);
    let (out, _) = red(&input);
    assert!(out.contains("this line has spaces"), "{out:?}");
    // the second chunk is not preceded by the marker line, so it is left alone
    assert!(out.contains(lines[1].as_str()), "{out:?}");
}

#[test]
fn key_data_look_alikes_in_code_are_unchanged() {
    assert_unchanged("const KEY_DATA_RE = /-key-data: (.*)/");
}
