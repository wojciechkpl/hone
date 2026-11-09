//! Permission policy model v2 (see `docs/plans/policy-v2-spec.md`).
//!
//! Three layers (built-in, user, project) of ordered rules over commands,
//! paths and hosts. User rules replace built-in verdicts; project rules can
//! only tighten. All globs are compiled once in [`CompiledPolicy::compile`];
//! evaluation never touches the filesystem.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Component, Path, PathBuf};

use globset::{Glob, GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};

use crate::permissions::{Decision, PermissionError, Rule};

// -- public types -------------------------------------------------------------

/// What a tool call touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A shell command line.
    Command(String),
    /// A filesystem path and the kind of access.
    Path { path: PathBuf, op: PathOp },
    /// A network host (optionally with `:port`).
    Host(String),
}

/// Kind of filesystem access in a [`Subject::Path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOp {
    Read,
    Write,
}

/// Operation filter on a path rule (`read`, `write` or `any`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpFilter {
    Read,
    Write,
    Any,
}

/// Policy layer, in increasing order of precedence of authorship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Builtin,
    User,
    Project,
}

impl Layer {
    /// Lowercase name, used in automatic rule ids.
    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Builtin => "builtin",
            Layer::User => "user",
            Layer::Project => "project",
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Environment used to expand path patterns and resolve relative subjects.
#[derive(Debug, Clone)]
pub struct PolicyContext {
    /// Absolute working directory.
    pub workdir: PathBuf,
    /// Home directory; `None` makes `~` patterns unmatchable.
    pub home: Option<PathBuf>,
}

/// Outcome of evaluating a subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub decision: Decision,
    /// Id of the deciding rule; `None` when no rule matched.
    pub rule_id: Option<String>,
    /// Layer of the deciding rule; `None` when no rule matched.
    pub layer: Option<Layer>,
}

impl Verdict {
    fn default_allow() -> Self {
        Verdict {
            decision: Decision::Allow,
            rule_id: None,
            layer: None,
        }
    }
}

/// One `[[rules]]` entry. Well-formedness is checked in [`CompiledPolicy::compile`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    /// v1 spelling of `command`.
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    v1_match: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    op: Option<OpFilter>,
    decision: Decision,
}

impl PolicyRule {
    fn blank(decision: Decision) -> Self {
        PolicyRule {
            id: None,
            command: None,
            v1_match: None,
            path: None,
            host: None,
            op: None,
            decision,
        }
    }

    /// A command rule (literal word-boundary prefix, or glob if it has `*`/`?`).
    pub fn command(pattern: impl Into<String>, decision: Decision) -> Self {
        PolicyRule {
            command: Some(pattern.into()),
            ..Self::blank(decision)
        }
    }

    /// A path rule with an operation filter.
    pub fn path(pattern: impl Into<String>, op: OpFilter, decision: Decision) -> Self {
        PolicyRule {
            path: Some(pattern.into()),
            op: Some(op),
            ..Self::blank(decision)
        }
    }

    /// A host rule (glob over the whole host).
    pub fn host(pattern: impl Into<String>, decision: Decision) -> Self {
        PolicyRule {
            host: Some(pattern.into()),
            ..Self::blank(decision)
        }
    }

    /// Set an explicit id (must match `[A-Za-z0-9._-]{1,64}`).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl From<&Rule> for PolicyRule {
    fn from(rule: &Rule) -> Self {
        PolicyRule::command(rule.r#match.clone(), rule.decision)
    }
}

// -- TOML loading -------------------------------------------------------------

/// Maximum size of a policy file.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Maximum number of rules per layer.
const MAX_RULES_PER_LAYER: usize = 1000;
/// Maximum pattern length in bytes.
const MAX_PATTERN_BYTES: usize = 1024;

#[derive(Deserialize)]
struct VersionProbe {
    version: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[allow(dead_code)]
    version: Option<i64>,
    #[serde(default)]
    rules: Vec<PolicyRule>,
}

/// Cut a parser message at its first quote (parsers quote file text) and drop
/// control characters.
fn sanitize_message(msg: &str) -> String {
    let cut = msg.find(['`', '"', '\'']).map_or(msg, |i| &msg[..i]);
    // Control characters (including newlines) become spaces, then whitespace
    // runs collapse, so multi-line parser messages stay readable.
    let spaced: String = cut
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clean.trim();
    if clean.is_empty() {
        "parse error".to_string()
    } else {
        clean.to_string()
    }
}

/// Convert a TOML error to a message with `line:column` and no file text.
fn toml_error(content: &str, e: &toml::de::Error) -> PermissionError {
    let mut msg = sanitize_message(e.message());
    if let Some(span) = e.span() {
        let upto = content.get(..span.start).unwrap_or("");
        let line = upto.matches('\n').count() + 1;
        let col = upto.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        msg.push_str(&format!(" at line {line}:{col}"));
    }
    PermissionError::Toml(msg)
}

fn unsafe_file(path: &Path, reason: &str) -> PermissionError {
    PermissionError::UnsafeFile {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

/// Load rules from a v1 or v2 policy file. A missing file yields no rules.
///
/// The path (symlinks followed) must be a regular file of at most 64 KiB.
pub fn load_policy_file(path: &Path) -> Result<Vec<PolicyRule>, PermissionError> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(PermissionError::Io(e)),
    };
    if !meta.is_file() {
        return Err(unsafe_file(path, "not a regular file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(unsafe_file(path, "file exceeds 64 KiB"));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| PermissionError::Toml("file is not valid UTF-8".to_string()))?;
    let probe: VersionProbe = toml::from_str(&content).map_err(|e| toml_error(&content, &e))?;
    if let Some(v) = probe.version {
        if v != 1 && v != 2 {
            return Err(PermissionError::UnsupportedVersion(v));
        }
    }
    let file: PolicyFile = toml::from_str(&content).map_err(|e| toml_error(&content, &e))?;
    Ok(file.rules)
}

// -- compiled form ------------------------------------------------------------

#[derive(Debug)]
enum CommandMatcher {
    Literal(String),
    Glob(GlobMatcher),
}

#[derive(Debug)]
enum Matcher {
    Command(CommandMatcher),
    Path {
        op: OpFilter,
        /// Empty means the rule can never match (e.g. `~` without a home).
        globs: Vec<GlobMatcher>,
    },
    Host(GlobMatcher),
}

#[derive(Debug)]
struct CompiledRule {
    id: String,
    decision: Decision,
    matcher: Matcher,
}

/// A subject prepared once per evaluation (trimmed / normalised).
enum Prepared<'a> {
    Command(&'a str),
    Path { op: PathOp, path: PathBuf },
    Host(String),
}

impl CompiledRule {
    fn matches(&self, s: &Prepared<'_>) -> bool {
        match (&self.matcher, s) {
            (Matcher::Command(CommandMatcher::Literal(p)), Prepared::Command(c)) => c
                .strip_prefix(p.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t'])),
            (Matcher::Command(CommandMatcher::Glob(g)), Prepared::Command(c)) => g.is_match(c),
            (Matcher::Path { op, globs }, Prepared::Path { op: sop, path }) => {
                let op_ok = matches!(
                    (op, sop),
                    (OpFilter::Any, _)
                        | (OpFilter::Read, PathOp::Read)
                        | (OpFilter::Write, PathOp::Write)
                );
                op_ok && globs.iter().any(|g| g.is_match(path))
            }
            (Matcher::Host(g), Prepared::Host(h)) => g.is_match(h),
            _ => false,
        }
    }
}

/// A policy with all globs compiled; evaluation is pure and fast.
#[derive(Debug)]
pub struct CompiledPolicy {
    workdir: PathBuf,
    builtin: Vec<CompiledRule>,
    user: Vec<CompiledRule>,
    project: Vec<CompiledRule>,
}

fn strictness(d: Decision) -> u8 {
    match d {
        Decision::Allow => 0,
        Decision::Ask => 1,
        Decision::Deny => 2,
    }
}

fn invalid_host_verdict() -> Verdict {
    Verdict {
        decision: Decision::Ask,
        rule_id: Some("builtin:invalid-host".to_string()),
        layer: Some(Layer::Builtin),
    }
}

impl CompiledPolicy {
    /// Validate and compile the three layers.
    pub fn compile(
        ctx: &PolicyContext,
        builtin: &[PolicyRule],
        user: &[PolicyRule],
        project: &[PolicyRule],
    ) -> Result<Self, PermissionError> {
        let env = ExpansionEnv::new(ctx)?;
        let mut seen = HashSet::new();
        let builtin = compile_layer(&env, Layer::Builtin, builtin, &mut seen)?;
        let user = compile_layer(&env, Layer::User, user, &mut seen)?;
        let project = compile_layer(&env, Layer::Project, project, &mut seen)?;
        Ok(CompiledPolicy {
            workdir: env.workdir,
            builtin,
            user,
            project,
        })
    }

    /// Evaluate one subject.
    pub fn evaluate(&self, subject: &Subject) -> Verdict {
        let s = match subject {
            Subject::Command(c) => Prepared::Command(c.trim()),
            Subject::Path { path, op } => {
                let joined;
                let p = if path.is_absolute() {
                    path.as_path()
                } else {
                    joined = self.workdir.join(path);
                    joined.as_path()
                };
                Prepared::Path {
                    op: *op,
                    path: normalise_path(p),
                }
            }
            Subject::Host(h) => match normalise_host_subject(h) {
                Some(h) => Prepared::Host(h),
                None => {
                    // Malformed: rules still see the raw lowercased text, and the
                    // result is never looser than Ask (ties go to invalid-host).
                    let v = self.evaluate_prepared(&Prepared::Host(h.to_lowercase()));
                    return if strictness(v.decision) > strictness(Decision::Ask) {
                        v
                    } else {
                        invalid_host_verdict()
                    };
                }
            },
        };
        self.evaluate_prepared(&s)
    }

    fn evaluate_prepared(&self, s: &Prepared<'_>) -> Verdict {
        let base = first_match(&self.user, Layer::User, s)
            .or_else(|| first_match(&self.builtin, Layer::Builtin, s));
        let proj = first_match(&self.project, Layer::Project, s);
        match (base, proj) {
            (Some(b), Some(p)) if strictness(p.decision) > strictness(b.decision) => p,
            (Some(b), _) => b,
            (None, Some(p)) if strictness(p.decision) > strictness(Decision::Allow) => p,
            (None, _) => Verdict::default_allow(),
        }
    }

    /// Strictest verdict over all subjects; ties go to the earliest subject.
    /// Returns early on the first `Deny`.
    pub fn evaluate_all(&self, subjects: &[Subject]) -> Verdict {
        let mut best: Option<Verdict> = None;
        for subject in subjects {
            let v = self.evaluate(subject);
            if v.decision == Decision::Deny
                && best.as_ref().is_none_or(|b| b.decision != Decision::Deny)
            {
                return v;
            }
            match &best {
                Some(b) if strictness(v.decision) <= strictness(b.decision) => {}
                _ => best = Some(v),
            }
        }
        best.unwrap_or_else(Verdict::default_allow)
    }

    /// Load user and project files from explicit paths; all errors propagate.
    ///
    /// The project path must not itself be a symlink (a project directory is
    /// untrusted); the user path may be.
    pub fn load_hierarchical_with_paths(
        ctx: &PolicyContext,
        builtin: &[PolicyRule],
        user: &Path,
        project: &Path,
    ) -> Result<Self, PermissionError> {
        let user_rules = load_policy_file(user)?;
        for ancestor in project.ancestors() {
            let inside = ancestor != ctx.workdir && ancestor.starts_with(&ctx.workdir);
            if ancestor != project && !inside {
                break;
            }
            if std::fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(unsafe_file(
                    ancestor,
                    "project policy path must not contain a symlink",
                ));
            }
        }
        let project_rules = load_policy_file(project)?;
        Self::compile(ctx, builtin, &user_rules, &project_rules)
    }

    /// Load `<home>/.config/hone/permissions.toml` and
    /// `<workdir>/.hone/permissions.toml`. Requires `ctx.home`.
    pub fn load_hierarchical(
        ctx: &PolicyContext,
        builtin: &[PolicyRule],
    ) -> Result<Self, PermissionError> {
        let home = ctx
            .home
            .as_ref()
            .ok_or_else(|| PermissionError::InvalidContext("home directory is required".into()))?;
        let user = home.join(".config").join("hone").join("permissions.toml");
        let hone_dir = ctx.workdir.join(".hone");
        if std::fs::symlink_metadata(&hone_dir).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(unsafe_file(&hone_dir, "`.hone` must not be a symlink"));
        }
        let project = hone_dir.join("permissions.toml");
        Self::load_hierarchical_with_paths(ctx, builtin, &user, &project)
    }
}

fn first_match(rules: &[CompiledRule], layer: Layer, s: &Prepared<'_>) -> Option<Verdict> {
    rules.iter().find(|r| r.matches(s)).map(|r| Verdict {
        decision: r.decision,
        rule_id: Some(r.id.clone()),
        layer: Some(layer),
    })
}

// -- compilation helpers ------------------------------------------------------

/// Normalised, glob-escaped workdir/home used when expanding path patterns.
struct ExpansionEnv {
    workdir: PathBuf,
    workdir_escaped: String,
    home_escaped: Option<String>,
}

impl ExpansionEnv {
    fn new(ctx: &PolicyContext) -> Result<Self, PermissionError> {
        let ctx_err = |m: &str| PermissionError::InvalidContext(m.to_string());
        if !ctx.workdir.is_absolute() {
            return Err(ctx_err("workdir must be an absolute path"));
        }
        let workdir = normalise_path(&ctx.workdir);
        let workdir_escaped = escape_glob(
            workdir
                .to_str()
                .ok_or_else(|| ctx_err("workdir must be valid UTF-8"))?,
        );
        let home_escaped = match &ctx.home {
            None => None,
            Some(h) => {
                if !h.is_absolute() {
                    return Err(ctx_err("home must be an absolute path"));
                }
                let n = normalise_path(h);
                Some(escape_glob(
                    n.to_str()
                        .ok_or_else(|| ctx_err("home must be valid UTF-8"))?,
                ))
            }
        };
        Ok(ExpansionEnv {
            workdir,
            workdir_escaped,
            home_escaped,
        })
    }
}

fn compile_layer(
    env: &ExpansionEnv,
    layer: Layer,
    rules: &[PolicyRule],
    seen: &mut HashSet<String>,
) -> Result<Vec<CompiledRule>, PermissionError> {
    if rules.len() > MAX_RULES_PER_LAYER {
        return Err(PermissionError::TooManyRules {
            layer,
            count: rules.len(),
            max: MAX_RULES_PER_LAYER,
        });
    }
    let mut out = Vec::with_capacity(rules.len());
    for (i, rule) in rules.iter().enumerate() {
        let index = i + 1;
        let invalid = |reason: &str| PermissionError::InvalidRule {
            layer,
            index,
            reason: reason.to_string(),
        };

        if let Some(id) = &rule.id {
            let ok = (1..=64).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
            if !ok {
                return Err(invalid("rule id must match [A-Za-z0-9._-]{1,64}"));
            }
        }

        let command = rule.command.as_ref().or(rule.v1_match.as_ref());
        let kinds = usize::from(rule.command.is_some())
            + usize::from(rule.v1_match.is_some())
            + usize::from(rule.path.is_some())
            + usize::from(rule.host.is_some());
        if kinds != 1 {
            return Err(invalid(
                "rule must have exactly one of `command`, `match`, `path` or `host`",
            ));
        }
        if rule.op.is_some() && rule.path.is_none() {
            return Err(invalid("`op` is only valid on path rules"));
        }
        let pattern = command
            .or(rule.path.as_ref())
            .or(rule.host.as_ref())
            .map_or("", String::as_str);
        if pattern.is_empty() {
            return Err(invalid("pattern must not be empty"));
        }
        if pattern.chars().any(char::is_control) {
            return Err(invalid("pattern must not contain control characters"));
        }
        if pattern.len() > MAX_PATTERN_BYTES {
            return Err(invalid("pattern longer than 1024 bytes"));
        }

        let matcher = if let Some(p) = command {
            if p.trim() != p {
                return Err(invalid(
                    "command pattern must not have surrounding whitespace",
                ));
            }
            Matcher::Command(compile_command(p)?)
        } else if let Some(p) = &rule.path {
            let globs = compile_path(env, p).map_err(|e| match e {
                PathPatternError::Invalid(r) => invalid(&r),
                PathPatternError::Glob(e) => e,
            })?;
            if globs.is_empty() {
                tracing::warn!(layer = %layer, index, "rule uses `~` but no home directory is known; it can never match");
            }
            Matcher::Path {
                op: rule.op.unwrap_or(OpFilter::Any),
                globs,
            }
        } else if let Some(p) = &rule.host {
            let pat = normalise_host_pattern(p).map_err(invalid)?;
            Matcher::Host(build_glob(Glob::new(&pat), &pat)?)
        } else {
            return Err(invalid("rule has no matcher"));
        };

        let id = match &rule.id {
            Some(id) => {
                if !seen.insert(id.clone()) {
                    return Err(PermissionError::DuplicateRuleId(id.clone()));
                }
                id.clone()
            }
            None => format!("{}:{}", layer.as_str(), index),
        };
        out.push(CompiledRule {
            id,
            decision: rule.decision,
            matcher,
        });
    }
    Ok(out)
}

fn build_glob(
    glob: Result<Glob, globset::Error>,
    pattern: &str,
) -> Result<GlobMatcher, PermissionError> {
    glob.map(|g| g.compile_matcher())
        .map_err(|e| PermissionError::InvalidGlob {
            pattern: pattern.to_string(),
            reason: e.to_string(),
        })
}

fn compile_command(pattern: &str) -> Result<CommandMatcher, PermissionError> {
    if pattern.contains(['*', '?']) {
        Ok(CommandMatcher::Glob(build_glob(
            Glob::new(pattern),
            pattern,
        )?))
    } else {
        Ok(CommandMatcher::Literal(pattern.to_string()))
    }
}

// -- hosts --------------------------------------------------------------------

fn canonical_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => v6.to_string(),
        },
    }
}

fn has_forbidden_host_char(s: &str) -> bool {
    !s.is_ascii() || s.chars().any(|c| c.is_ascii_control() || c.is_whitespace())
}

/// Normalise a host subject; `None` means malformed.
fn normalise_host_subject(raw: &str) -> Option<String> {
    if !raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'[' | b']'))
    {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let is_port = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let host: &str = if let Some(rest) = lower.strip_prefix('[') {
        let (inner, after) = rest.split_once(']')?;
        if !after.is_empty() && !after.strip_prefix(':').is_some_and(is_port) {
            return None;
        }
        inner.parse::<Ipv6Addr>().ok()?;
        inner
    } else {
        match lower.matches(':').count() {
            0 => &lower,
            1 => {
                let (h, port) = lower.split_once(':')?;
                if !is_port(port) {
                    return None;
                }
                h
            }
            _ => {
                lower.parse::<Ipv6Addr>().ok()?;
                &lower
            }
        }
    };
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    Some(match host.parse::<IpAddr>() {
        Ok(ip) => canonical_ip(ip),
        Err(_) => host.to_string(),
    })
}

/// Normalise a host pattern; `Err` carries the `InvalidRule` reason.
fn normalise_host_pattern(raw: &str) -> Result<String, &'static str> {
    if !raw.is_ascii() {
        return Err("host pattern must be ASCII (use punycode)");
    }
    if has_forbidden_host_char(raw) || raw.contains(['@', '/', '#', '\\']) {
        return Err("host pattern contains a forbidden character");
    }
    let lower = raw.to_ascii_lowercase();
    if let Some(inner) = lower.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if let Ok(ip) = inner.parse::<Ipv6Addr>() {
            return Ok(canonical_ip(IpAddr::V6(ip)));
        }
    }
    if lower.contains(':') {
        return match lower.parse::<Ipv6Addr>() {
            Ok(ip) => Ok(canonical_ip(IpAddr::V6(ip))),
            Err(_) => Err("host pattern must not contain a port or ':'"),
        };
    }
    let t = lower.trim_end_matches('.');
    if t.is_empty() {
        return Err("host pattern is empty");
    }
    Ok(match t.parse::<Ipv4Addr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => t.to_string(),
    })
}

// -- paths --------------------------------------------------------------------

enum PathPatternError {
    Invalid(String),
    Glob(PermissionError),
}

/// Escape glob metacharacters (and `\`) with backslashes.
fn escape_glob(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// True if `seg` has an unescaped glob metacharacter.
fn segment_has_glob(seg: &str) -> bool {
    let mut chars = seg.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '*' | '?' | '[' | ']' | '{' | '}' => return true,
            _ => {}
        }
    }
    false
}

/// True if `seg` has two adjacent unescaped `*`.
fn segment_has_double_star(seg: &str) -> bool {
    let mut chars = seg.chars();
    let mut prev_star = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
                prev_star = false;
            }
            '*' if prev_star => return true,
            '*' => prev_star = true,
            _ => prev_star = false,
        }
    }
    false
}

fn compile_path(env: &ExpansionEnv, raw: &str) -> Result<Vec<GlobMatcher>, PathPatternError> {
    let invalid = |m: &str| PathPatternError::Invalid(m.to_string());

    let expanded = if raw == "~" || raw.starts_with("~/") {
        let Some(home) = &env.home_escaped else {
            return Ok(Vec::new());
        };
        format!(
            "{home}{}",
            raw[1..].replace("{workdir}", &env.workdir_escaped)
        )
    } else {
        raw.replace("{workdir}", &env.workdir_escaped)
    };
    let anchored = if expanded.starts_with('/') || expanded.starts_with("**") {
        expanded
    } else {
        format!("{}/{expanded}", env.workdir_escaped)
    };

    // Lexical normalisation over segments, tracking which are literal.
    let absolute = anchored.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for seg in anchored.split('/') {
        match seg {
            "" | "." => {}
            ".." => match stack.last() {
                Some(top) if !segment_has_glob(top) && *top != ".." => {
                    stack.pop();
                }
                None if absolute => {}
                _ => return Err(invalid("`..` cannot be resolved after a glob segment")),
            },
            s => {
                if s != "**" && segment_has_double_star(s) {
                    return Err(invalid("`**` must be a whole path segment"));
                }
                stack.push(s);
            }
        }
    }
    let joined = stack.join("/");
    let pattern = if absolute {
        format!("/{joined}")
    } else {
        joined
    };

    let build = |p: &str| -> Result<GlobMatcher, PathPatternError> {
        build_glob(
            GlobBuilder::new(p)
                .literal_separator(true)
                .backslash_escape(true)
                .build(),
            p,
        )
        .map_err(PathPatternError::Glob)
    };
    let mut globs = vec![build(&pattern)?];
    if let Some(dir) = pattern.strip_suffix("/**") {
        globs.push(build(if dir.is_empty() { "/" } else { dir })?);
    }
    Ok(globs)
}

/// Lexically normalise a path: drop `.`, resolve `..` (never above root),
/// drop trailing slashes. Never touches the filesystem and never converts
/// through UTF-8.
fn normalise_path(path: &Path) -> PathBuf {
    let mut prefix: Option<&OsStr> = None;
    let mut absolute = false;
    let mut stack: Vec<&OsStr> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => prefix = Some(p.as_os_str()),
            Component::RootDir => absolute = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if stack.last().is_some_and(|t| *t != "..") {
                    stack.pop();
                } else if !absolute {
                    stack.push(OsStr::new(".."));
                }
            }
            Component::Normal(s) => stack.push(s),
        }
    }
    let mut out = OsString::new();
    if let Some(p) = prefix {
        out.push(p);
    }
    if absolute {
        out.push("/");
    }
    for (i, seg) in stack.iter().enumerate() {
        if i > 0 {
            out.push("/");
        }
        out.push(seg);
    }
    PathBuf::from(out)
}
