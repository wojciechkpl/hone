//! Secret redaction for tool output (P2.5b, `docs/plans/policy-p2.5-spec.md` §10, §14).
//!
//! [`redact_secrets`] replaces secret-shaped values with `[REDACTED:<kind>]`. It is
//! pure and linear-time: the patterns run on the `regex` crate (no backtracking) and
//! are compiled once, and the hand-written assignment scanner only moves forward and
//! never re-scans text it has already passed. It is pattern-based, so an encoded or
//! unusually shaped secret still passes through.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::LazyLock;

use base64::Engine as _;
use regex::{Captures, Regex};

/// Result of [`redact_secrets`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redaction<'a> {
    /// The text with secret-shaped values replaced; borrowed when nothing matched.
    pub text: Cow<'a, str>,
    /// Number of replaced values (one per match; a key block counts once).
    pub count: usize,
}

const MARKER_PREFIX: &str = "[REDACTED:";

fn marker(kind: &str) -> String {
    format!("{MARKER_PREFIX}{kind}]")
}

/// Compile a constant pattern. The patterns are fixed, so a failure is a programming
/// error caught by the unit tests, and the redactor then degrades to doing nothing.
fn compile(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

/// A pattern whose whole match (or one group) is replaced by `[REDACTED:kind]`.
struct Shape {
    kind: &'static str,
    re: Option<Regex>,
    /// Which capture group to replace (0 = the whole match).
    group: usize,
    /// Extra check on the matched text.
    accept: fn(&str) -> bool,
}

fn always(_: &str) -> bool {
    true
}

fn openai_accept(m: &str) -> bool {
    !m.starts_with("sk-ant-") && m.bytes().any(|b| b.is_ascii_digit())
}

/// A bare base64 token that decodes to a PEM private-key header.
fn base64_private_key(m: &str) -> bool {
    let prefix: String = m.chars().take(48).collect();
    if prefix.len() < 48 {
        return false;
    }
    base64::engine::general_purpose::STANDARD
        .decode(prefix.as_bytes())
        .ok()
        .is_some_and(|b| String::from_utf8_lossy(&b).contains("PRIVATE KEY"))
}

static PRIVATE_KEYS: LazyLock<Vec<Shape>> = LazyLock::new(|| {
    let block = |kind, label: &str| Shape {
        kind,
        re: compile(&format!(
            r"(?s)-----BEGIN {label}-----(?:.*?-----END {label}-----|.*)"
        )),
        group: 0,
        accept: always,
    };
    vec![
        block("pgp-private-key", "PGP PRIVATE KEY BLOCK"),
        block("private-key", "[A-Z0-9 ]*PRIVATE KEY"),
    ]
});

static SHAPES: LazyLock<Vec<Shape>> = LazyLock::new(|| {
    let shape = |kind, pattern: &str, group, accept: fn(&str) -> bool| Shape {
        kind,
        re: compile(pattern),
        group,
        accept,
    };
    vec![
        shape("aws-key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", 0, always),
        shape(
            "github-token",
            r"\bgh[pousr]_[A-Za-z0-9]{36,}|\bgithub_pat_[A-Za-z0-9_]{22,}",
            0,
            always,
        ),
        shape("gitlab-token", r"\bglpat-[A-Za-z0-9_-]{20,}", 0, always),
        shape("slack-token", r"\bxox[abposr]-[A-Za-z0-9-]{10,}", 0, always),
        shape(
            "stripe-key",
            r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}",
            0,
            always,
        ),
        shape("anthropic-key", r"\bsk-ant-[A-Za-z0-9_-]{20,}", 0, always),
        shape(
            "openai-key",
            r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}",
            0,
            openai_accept,
        ),
        shape("google-api-key", r"\bAIza[0-9A-Za-z_-]{35}", 0, always),
        shape(
            "jwt",
            r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            0,
            always,
        ),
        shape("npm-token", r"\bnpm_[A-Za-z0-9]{36}\b", 0, always),
        shape("hf-token", r"\bhf_[A-Za-z0-9]{30,}\b", 0, always),
        shape(
            "sendgrid-key",
            r"\bSG\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}",
            0,
            always,
        ),
        shape(
            "slack-webhook",
            r"https://hooks\.slack\.com/services/T[A-Za-z0-9]+/B[A-Za-z0-9]+/([A-Za-z0-9]+)",
            1,
            always,
        ),
    ]
});

/// `Authorization:` (or `Proxy-Authorization:`), a scheme and the blank after it; JSON and
/// dict spellings (`"Authorization": "Bearer …"`) included. The credential follows.
static AUTH_HEAD: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compile(
        r#"(?i)\b(?:proxy-)?authorization["']?[ \t]*:[ \t]*["']?(?:bearer|basic|token|digest)[ \t]+"#,
    )
});

/// `"auth": "<base64>"` / `"identitytoken": "<base64>"` in Docker config.
static DOCKER_AUTH: LazyLock<Option<Regex>> =
    LazyLock::new(|| compile(r#"(?i)"(auth|identitytoken)"[ \t]*:[ \t]*"([A-Za-z0-9+/=_-]{8,})""#));

/// kubeconfig `*-key-data:` heads; the base64 value follows.
static KEY_DATA_HEAD: LazyLock<Option<Regex>> =
    LazyLock::new(|| compile(r#"[A-Za-z0-9_]-key-data"?[ \t]*:[ \t]*"?"#));

/// `scheme://user:PASSWORD@` (the user may be empty); group 2 is the password.
static URL_PASSWORD: LazyLock<Option<Regex>> =
    LazyLock::new(|| compile(r#"://([^\s/@:\[\]"'<>]*):([^\s/@"'<>]+)@"#));

/// Replace secret-shaped values in `text`.
pub fn redact_secrets(text: &str) -> Redaction<'_> {
    let mut count = 0usize;
    let mut cur: Cow<'_, str> = Cow::Borrowed(text);

    for shape in PRIVATE_KEYS.iter().chain(SHAPES.iter()) {
        let next = replace_regex(shape.re.as_ref(), &cur, &mut count, |c| {
            let m = c.get(shape.group)?;
            (shape.accept)(m.as_str()).then(|| (m.range(), shape.kind))
        });
        step(&mut cur, next);
    }
    let next = redact_auth_headers(&cur, &mut count);
    step(&mut cur, next);
    let next = redact_docker_auth(&cur, &mut count);
    step(&mut cur, next);
    let next = redact_key_data(&cur, &mut count);
    step(&mut cur, next);
    let next = redact_bare_pem_base64(&cur, &mut count);
    step(&mut cur, next);
    let next = redact_url_passwords(&cur, &mut count);
    step(&mut cur, next);
    let next = redact_netrc(&cur, &mut count);
    step(&mut cur, next);
    let next = redact_assignments(&cur, &mut count);
    step(&mut cur, next);

    Redaction { text: cur, count }
}

fn step(cur: &mut Cow<'_, str>, next: Option<String>) {
    if let Some(n) = next {
        *cur = Cow::Owned(n);
    }
}

/// Replace the range chosen by `pick` for each match of `re`. `None` when nothing
/// was replaced.
fn replace_regex<F>(
    re: Option<&Regex>,
    text: &str,
    count: &mut usize,
    mut pick: F,
) -> Option<String>
where
    F: FnMut(&Captures<'_>) -> Option<(Range<usize>, &'static str)>,
{
    let re = re?;
    let mut out: Option<String> = None;
    let mut last = 0usize;
    for caps in re.captures_iter(text) {
        let Some((range, kind)) = pick(&caps) else {
            continue;
        };
        if range.start < last {
            continue;
        }
        let buf = out.get_or_insert_with(|| String::with_capacity(text.len()));
        buf.push_str(&text[last..range.start]);
        buf.push_str(&marker(kind));
        last = range.end;
        *count += 1;
    }
    out.map(|mut buf| {
        buf.push_str(&text[last..]);
        buf
    })
}

/// Redact the JSON string leaves of a value, returning the new value and the number
/// of replacements. Keys, shape and non-string values are kept.
pub fn redact_json(value: &serde_json::Value) -> (serde_json::Value, usize) {
    let mut count = 0usize;
    let v = redact_json_inner(value, &mut count);
    (v, count)
}

fn redact_json_inner(value: &serde_json::Value, count: &mut usize) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) => {
            let r = redact_secrets(s);
            *count += r.count;
            if r.count == 0 {
                value.clone()
            } else {
                Value::String(r.text.into_owned())
            }
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| redact_json_inner(v, count)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), redact_json_inner(v, count)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// -- plausibility (spec §15.1) ---------------------------------------------------------------

/// Whether `c` ends an unquoted value: whitespace, a quote, `,`, `)`, `;`, `<`, `>`, `&`
/// or `#`. Delimiters and quotes are never part of a value.
fn ends_value(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ')' | ';' | '<' | '>' | '&' | '#')
}

/// Byte length of the unquoted value at the start of `s`.
fn cut_len(s: &str) -> usize {
    s.find(ends_value).unwrap_or(s.len())
}

/// A reference or template, never a secret: `$TOKEN`, `${{ secrets.X }}`, `%s`, `{token}`,
/// `%(password)s`, `{}`.
fn is_template(v: &str) -> bool {
    v.starts_with(['$', '{', '%', '<', '`', '"', '\''])
        || ["${", "{{", "%(", "%s", "{}", "}}"]
            .iter()
            .any(|t| v.contains(t))
}

/// One segment of a word-like value: letters then up to four digits, or one to four digits.
fn is_wordlike_segment(seg: &str) -> bool {
    let letters = seg.bytes().take_while(u8::is_ascii_alphabetic).count();
    let rest = &seg[letters..];
    if letters == 0 {
        (1..=4).contains(&seg.len()) && seg.bytes().all(|b| b.is_ascii_digit())
    } else {
        rest.len() <= 4 && rest.bytes().all(|b| b.is_ascii_digit())
    }
}

/// Two or more word-like segments joined by `_`, `-` or `.` (`X-Auth-Token-V2`).
fn is_wordlike(v: &str) -> bool {
    let mut n = 0;
    for seg in v.split(['_', '-', '.']) {
        if seg.is_empty() || !is_wordlike_segment(seg) {
            return false;
        }
        n += 1;
    }
    n >= 2
}

/// Prose, a URL without user info, a path or a URN.
fn is_prose_url_or_path(v: &str, quoted: bool) -> bool {
    if quoted && v.contains(' ') {
        return true;
    }
    if let Some(i) = v.find("://") {
        let authority = v[i + 3..].split('/').next().unwrap_or("");
        if !authority.contains('@') {
            return true;
        }
    }
    v.starts_with("/")
        || v.starts_with("~/")
        || v.starts_with("./")
        || v.starts_with("../")
        || v.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("urn:"))
}

/// The one check every value-capturing kind applies before redacting. `prose` adds the
/// prose, URL, path and URN rejection (secret assignments and auth headers).
fn plausible_value(v: &str, quoted: bool, prose: bool) -> bool {
    !v.is_empty()
        && !is_template(v)
        && plausible_secret(v, quoted)
        && !is_wordlike(v)
        && !(prose && is_prose_url_or_path(v, quoted))
}

// -- ranges ------------------------------------------------------------------------------------

/// Replace the (sorted, non-overlapping) ranges with markers.
fn apply_ranges(
    text: &str,
    ranges: Vec<(Range<usize>, &'static str)>,
    count: &mut usize,
) -> Option<String> {
    if ranges.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for (r, kind) in ranges {
        if r.start < last {
            continue;
        }
        out.push_str(&text[last..r.start]);
        out.push_str(&marker(kind));
        last = r.end;
        *count += 1;
    }
    out.push_str(&text[last..]);
    Some(out)
}

fn redact_auth_headers(text: &str, count: &mut usize) -> Option<String> {
    let head = AUTH_HEAD.as_ref()?;
    let mut ranges = Vec::new();
    for m in head.find_iter(text) {
        let start = m.end();
        let len = cut_len(&text[start..]);
        if len > 0 && plausible_value(&text[start..start + len], false, true) {
            ranges.push((start..start + len, "auth-header"));
        }
    }
    apply_ranges(text, ranges, count)
}

/// Base64 of printable `user:pass` (not a bare word that merely looks like base64).
fn decodes_to_user_pass(v: &str) -> bool {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
    let bytes = STANDARD
        .decode(v)
        .or_else(|_| STANDARD_NO_PAD.decode(v.trim_end_matches('=')))
        .or_else(|_| URL_SAFE_NO_PAD.decode(v.trim_end_matches('=')));
    let Ok(bytes) = bytes else { return false };
    bytes.len() >= 3
        && bytes.iter().all(|b| (0x20..0x7f).contains(b))
        && bytes
            .iter()
            .position(|b| *b == b':')
            .is_some_and(|i| i > 0 && i + 1 < bytes.len())
}

fn is_standard_base64(v: &str) -> bool {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    STANDARD.decode(v).is_ok() || STANDARD_NO_PAD.decode(v.trim_end_matches('=')).is_ok()
}

fn redact_docker_auth(text: &str, count: &mut usize) -> Option<String> {
    let re = DOCKER_AUTH.as_ref()?;
    let mut ranges = Vec::new();
    for caps in re.captures_iter(text) {
        let (Some(key), Some(g)) = (caps.get(1), caps.get(2)) else {
            continue;
        };
        // `auth` is base64 of `user:pass`; `identitytoken` is an opaque base64 token.
        let ok = if key.as_str().eq_ignore_ascii_case("auth") {
            decodes_to_user_pass(g.as_str())
        } else {
            is_standard_base64(g.as_str())
        };
        if ok {
            ranges.push((g.range(), "docker-auth"));
        }
    }
    apply_ranges(text, ranges, count)
}

fn is_b64_line(l: &str) -> bool {
    !l.is_empty()
        && l.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// If a base64 token of `len` bytes ends `end` and the line ends there, extend over the
/// following 76-column continuation lines of pure base64 (the last one is shorter).
fn extend_wrapped_base64(text: &str, end: usize, len: usize) -> usize {
    let mut end = end;
    if len < 60 || !text[end..].starts_with('\n') {
        return end;
    }
    loop {
        let rest = &text[end + 1..];
        let line = rest.split('\n').next().unwrap_or("");
        if !is_b64_line(line) {
            return end;
        }
        end += 1 + line.len();
        if line.len() < 60 || !text[end..].starts_with('\n') {
            return end;
        }
    }
}

/// kubeconfig `*-key-data:` values (and their wrapped continuation lines).
fn redact_key_data(text: &str, count: &mut usize) -> Option<String> {
    let head = KEY_DATA_HEAD.as_ref()?;
    let mut ranges: Vec<(Range<usize>, &'static str)> = Vec::new();
    let mut floor = 0usize;
    for m in head.find_iter(text) {
        let start = m.end();
        if start < floor {
            continue;
        }
        let len = text[start..]
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
            .count();
        if len < 8 || text[start..start + len].contains(MARKER_PREFIX) {
            continue;
        }
        let end = extend_wrapped_base64(text, start + len, len);
        ranges.push((start..end, "kube-key-data"));
        floor = end;
    }
    apply_ranges(text, ranges, count)
}

/// A bare base64 PEM key (`LS0tLS1CRUdJTi…`), possibly wrapped at 76 columns.
fn redact_bare_pem_base64(text: &str, count: &mut usize) -> Option<String> {
    const NEEDLE: &str = "LS0tLS1CRUdJTi";
    let mut ranges: Vec<(Range<usize>, &'static str)> = Vec::new();
    let mut from = 0usize;
    while let Some(i) = text[from..].find(NEEDLE) {
        let start = from + i;
        let token_len = text[start..]
            .bytes()
            .take_while(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-')
            })
            .count();
        let end = start + token_len;
        from = end.max(start + 1);
        let boundary = start == 0 || !text.as_bytes()[start - 1].is_ascii_alphanumeric();
        if !boundary || !base64_private_key(&text[start..end]) {
            continue;
        }
        let full = extend_wrapped_base64(text, end, token_len);
        ranges.push((start..full, "kube-key-data"));
        from = full.max(from);
    }
    apply_ranges(text, ranges, count)
}

fn redact_url_passwords(text: &str, count: &mut usize) -> Option<String> {
    let re = URL_PASSWORD.as_ref()?;
    let mut ranges = Vec::new();
    for caps in re.captures_iter(text) {
        if let Some(g) = caps.get(2)
            && plausible_value(g.as_str(), false, false)
        {
            ranges.push((g.range(), "url-password"));
        }
    }
    apply_ranges(text, ranges, count)
}

// -- netrc ---------------------------------------------------------------------------------------

/// netrc grammar across lines: after `machine HOST` or `default`, the value after the next
/// `password` token is the secret, until the next `machine`/`default` or a blank line.
fn redact_netrc(text: &str, count: &mut usize) -> Option<String> {
    let mut ranges: Vec<(Range<usize>, &'static str)> = Vec::new();
    let mut in_entry = false;
    let mut expect_value = false;
    let mut line_start = 0usize;
    for line in text.split('\n') {
        if line.trim().is_empty() {
            in_entry = false;
            expect_value = false;
        } else {
            let mut off = 0usize;
            for tok in line.split_inclusive(char::is_whitespace) {
                let word = tok.trim_end();
                let tok_start = line_start + off;
                off += tok.len();
                if word.is_empty() {
                    continue;
                }
                if expect_value {
                    expect_value = false;
                    let len = cut_len(word);
                    if len > 0 && plausible_value(&word[..len], false, false) {
                        ranges.push((tok_start..tok_start + len, "netrc-password"));
                    }
                    continue;
                }
                match word {
                    "machine" | "default" => in_entry = true,
                    "password" if in_entry => expect_value = true,
                    _ => {}
                }
            }
        }
        line_start += line.len() + 1;
    }
    apply_ranges(text, ranges, count)
}

// -- secret assignments ---------------------------------------------------------------------

/// Split a key on `_`, `-`, `.` and camelCase boundaries, lowercased.
fn key_segments(key: &str) -> Vec<String> {
    let mut segs = Vec::new();
    let mut cur = String::new();
    let mut prev_lower_or_digit = false;
    for c in key.chars() {
        if matches!(c, '_' | '-' | '.') {
            if !cur.is_empty() {
                segs.push(std::mem::take(&mut cur));
            }
            prev_lower_or_digit = false;
            continue;
        }
        if c.is_uppercase() && prev_lower_or_digit && !cur.is_empty() {
            segs.push(std::mem::take(&mut cur));
        }
        prev_lower_or_digit = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        segs.push(cur);
    }
    segs
}

fn is_secret_key(key: &str) -> bool {
    let segs = key_segments(key);
    if segs.iter().any(|s| {
        matches!(
            s.as_str(),
            "secret" | "token" | "password" | "passwd" | "credential" | "credentials"
        )
    }) {
        return true;
    }
    segs.windows(2).any(|w| {
        matches!(
            (w[0].as_str(), w[1].as_str()),
            ("api", "key") | ("access", "key") | ("private", "key") | ("auth", "token")
        )
    })
}

/// Digits with optional `.`, `_` and `-` separators (at least one digit).
fn is_number_like(v: &str) -> bool {
    v.bytes().any(|b| b.is_ascii_digit())
        && v.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

/// `^[A-Za-z_][A-Za-z0-9_.]*$` with no digit: a variable or field name.
fn is_identifier_without_digit(v: &str) -> bool {
    let mut bytes = v.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && v.bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'.')
}

/// Whether a candidate value looks like a literal secret (spec §14.2, clauses 1 to 3).
fn plausible_secret(v: &str, quoted: bool) -> bool {
    let chars = v.chars().count();
    if chars < 8 || v.starts_with(MARKER_PREFIX) || is_number_like(v) {
        return false;
    }
    if !quoted && is_identifier_without_digit(v) {
        return false;
    }
    if quoted && (v.starts_with("${") || v.starts_with("$(") || v.starts_with("{{")) {
        return false;
    }
    v.bytes().any(|b| b.is_ascii_digit()) || chars >= 20
}

/// What to do with the text after a secret-named key and its separator.
enum Candidate {
    /// Replace this byte range; continue scanning at the second value.
    Redact(Range<usize>, usize),
    /// Not a secret; continue scanning at this byte offset.
    Skip(usize),
}

fn candidate(text: &str, start: usize, sep: u8) -> Candidate {
    let rest = &text[start..];
    let Some(first) = rest.chars().next() else {
        return Candidate::Skip(text.len());
    };
    if first == '"' || first == '\'' {
        let body = &rest[1..];
        let limit = body.len().min(4096);
        let window = &body.as_bytes()[..limit];
        let close = window.iter().position(|b| *b == first as u8 || *b == b'\n');
        return match close {
            Some(c) if body.as_bytes()[c] == first as u8 => {
                let inner = &body[..c];
                let end = start + 1 + c + 1;
                if plausible_value(inner, true, true) {
                    Candidate::Redact(start + 1..start + 1 + c, end)
                } else {
                    Candidate::Skip(end)
                }
            }
            // Unterminated on this line: look past the quote.
            _ => Candidate::Skip(start + 1),
        };
    }

    let mut end = start;
    for (off, c) in rest.char_indices() {
        if ends_value(c) {
            break;
        }
        if matches!(c, '(' | '$' | '{' | '[' | '%') {
            return Candidate::Skip(start + off + c.len_utf8());
        }
        end = start + off + c.len_utf8();
    }
    if end == start {
        return Candidate::Skip(start + first.len_utf8());
    }
    let value = &text[start..end];
    if !plausible_value(value, false, true) {
        return Candidate::Skip(end);
    }
    if sep == b':' {
        // After a colon an unquoted value counts only at the end of the line, apart
        // from a `# comment`.
        let tail = &text[end..];
        let next = tail.chars().find(|c| !matches!(c, ' ' | '\t' | '\r'));
        if !matches!(next, None | Some('\n') | Some('#')) {
            return Candidate::Skip(end);
        }
    }
    Candidate::Redact(start..end, end)
}

fn is_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')
}

/// The key ending just before the separator at `sep` (skipping blanks and one closing
/// quote), never reading before `floor`. Bounded, so the whole scan stays linear.
fn key_before(text: &str, sep: usize, floor: usize) -> Option<&str> {
    let b = text.as_bytes();
    let mut j = sep;
    let mut blanks = 0;
    while j > floor && blanks < 32 && matches!(b[j - 1], b' ' | b'\t') {
        j -= 1;
        blanks += 1;
    }
    if j > floor && matches!(b[j - 1], b'"' | b'\'') {
        j -= 1;
    }
    let end = j;
    while j > floor && end - j < 256 && is_key_byte(b[j - 1]) {
        j -= 1;
    }
    while j < end && matches!(b[j], b'.' | b'-') {
        j += 1;
    }
    (j < end).then(|| &text[j..end])
}

/// One forward pass over `text`: every `=` and `:` is looked at once, the key is read
/// backwards (bounded), and only secret-named keys cause a forward scan of the value.
/// After any candidate the scan resumes past what was read.
fn redact_assignments(text: &str, count: &mut usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out: Option<String> = None;
    let mut last = 0usize; // end of the text already copied
    let mut pos = 0usize; // where the next separator search starts
    while pos < bytes.len() {
        let Some(off) = bytes[pos..].iter().position(|b| matches!(b, b'=' | b':')) else {
            break;
        };
        let sep = pos + off;
        let sep_byte = bytes[sep];
        let floor = pos;
        pos = sep + 1;
        let Some(key) = key_before(text, sep, floor) else {
            continue;
        };
        if !is_secret_key(key) {
            continue;
        }
        let mut v = sep + 1;
        let mut blanks = 0;
        while v < bytes.len() && blanks < 64 && matches!(bytes[v], b' ' | b'\t') {
            v += 1;
            blanks += 1;
        }
        if v >= bytes.len() {
            break;
        }
        match candidate(text, v, sep_byte) {
            Candidate::Redact(range, next) => {
                let buf = out.get_or_insert_with(|| String::with_capacity(text.len()));
                buf.push_str(&text[last..range.start]);
                buf.push_str(&marker("secret-assignment"));
                last = range.end;
                pos = next;
                *count += 1;
            }
            Candidate::Skip(next) => pos = next.max(sep + 1),
        }
    }
    out.map(|mut buf| {
        buf.push_str(&text[last..]);
        buf
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        assert!(PRIVATE_KEYS.iter().all(|s| s.re.is_some()));
        assert!(SHAPES.iter().all(|s| s.re.is_some()));
        assert!(AUTH_HEAD.is_some());
        assert!(DOCKER_AUTH.is_some());
        assert!(KEY_DATA_HEAD.is_some());
        assert!(URL_PASSWORD.is_some());
    }

    #[test]
    fn key_segments_split_camel_case_and_separators() {
        assert_eq!(key_segments("clientSecret"), ["client", "secret"]);
        assert_eq!(
            key_segments("aws.secret_access-key"),
            ["aws", "secret", "access", "key"]
        );
        assert!(is_secret_key("apiKey"));
        assert!(!is_secret_key("MAX_TOKENS"));
    }

    #[test]
    fn wordlike_values() {
        assert!(is_wordlike("X-Auth-Token-V2"));
        assert!(is_wordlike("OPENAI_API_KEY_2"));
        assert!(!is_wordlike("HC-CANARY-AWS-91c2-benchmark-canary"));
        assert!(!is_wordlike("s3cr3t-Passw0rd"));
        assert!(!is_wordlike("hunter2hunter2"));
    }

    #[test]
    fn value_rules() {
        assert!(!is_identifier_without_digit("abc123"));
        assert!(is_identifier_without_digit("args.password"));
        assert!(is_number_like("2024-11-09"));
        assert!(!is_number_like("abc"));
    }
}
