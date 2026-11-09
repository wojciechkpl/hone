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
    /// Match path rules case-insensitively (macOS/Windows filesystems).
    pub case_insensitive_paths: bool,
}

impl PolicyContext {
    /// Context with `case_insensitive_paths` set per target OS (true on macOS and Windows).
    pub fn for_platform(workdir: PathBuf, home: Option<PathBuf>) -> Self {
        PolicyContext {
            workdir,
            home,
            case_insensitive_paths: cfg!(any(target_os = "macos", target_os = "windows")),
        }
    }
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
    /// Additional path patterns of the same rule (built-in rules only; never
    /// read from or written to a file).
    #[serde(skip)]
    extra_paths: Vec<String>,
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
            extra_paths: Vec::new(),
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

    /// Add another path pattern to a path rule; the rule matches if any pattern does.
    /// Not available from policy files.
    pub fn or_path(mut self, pattern: impl Into<String>) -> Self {
        self.extra_paths.push(pattern.into());
        self
    }

    /// Set an explicit id (must match `[A-Za-z0-9._-]{1,64}`; in the built-in
    /// layer `builtin:[a-z0-9-]{1,48}`).
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

// -- built-in rules ------------------------------------------------------------

/// Ids the gate uses for decisions that aren't rules; built-in rules never use them.
const RESERVED_BUILTIN_IDS: [&str; 5] = [
    "builtin:unparsed-command",
    "builtin:invalid-host",
    "builtin:pipe-to-shell",
    "builtin:computer-control",
    "builtin:screen-capture",
];

/// The built-in rule set (`docs/plans/policy-p2.5-spec.md` section 3), in
/// evaluation order. Compile it as the Builtin layer.
pub fn builtin_rules() -> Vec<PolicyRule> {
    use Decision::{Allow, Ask, Deny};
    use OpFilter::{Any, Write};

    let path = |id: &str, patterns: &[&str], op: OpFilter, d: Decision| {
        let mut it = patterns.iter();
        let first = it.next().copied().unwrap_or("/");
        let mut r = PolicyRule::path(first, op, d).with_id(id);
        for p in it {
            r = r.or_path(*p);
        }
        r
    };
    let cmd = |id: &str, pattern: &str, d: Decision| PolicyRule::command(pattern, d).with_id(id);

    vec![
        path(
            "builtin:env-template",
            &[
                "{workdir}/**/.env.example",
                "{workdir}/**/.env.sample",
                "{workdir}/**/.env.template",
            ],
            Any,
            Allow,
        ),
        path(
            "builtin:env-file",
            &["**/.env", "**/.env.*", "**/*.env", "**/.envrc"],
            Any,
            Ask,
        ),
        path("builtin:ssh", &["~/.ssh/**"], Any, Ask),
        path(
            "builtin:cloud-credentials",
            &[
                "~/.aws/**",
                "~/.config/gcloud/**",
                "~/.azure/**",
                "~/.kube/**",
                "~/.docker/config.json",
                "~/.vault-token",
                "~/.terraform.d/credentials.tfrc.json",
            ],
            Any,
            Ask,
        ),
        path(
            "builtin:git-credentials",
            &[
                "~/.git-credentials",
                "~/.config/git/credentials",
                "~/.netrc",
                "~/.config/gh/hosts.yml",
            ],
            Any,
            Ask,
        ),
        path(
            "builtin:package-credentials",
            &[
                "**/.npmrc",
                "**/.pypirc",
                "~/.cargo/credentials",
                "~/.cargo/credentials.toml",
                "~/.gem/credentials",
            ],
            Any,
            Ask,
        ),
        path("builtin:gnupg", &["~/.gnupg/**"], Any, Ask),
        path(
            "builtin:shell-history",
            &[
                "~/.bash_history",
                "~/.zsh_history",
                "~/.local/share/fish/fish_history",
                "~/.python_history",
                "~/.psql_history",
                "~/.mysql_history",
            ],
            Any,
            Ask,
        ),
        path(
            "builtin:hone-config",
            &["~/.config/hone/**", "**/.hone/**"],
            Write,
            Ask,
        ),
        path(
            "builtin:shell-startup",
            &[
                "~/.bashrc",
                "~/.bash_profile",
                "~/.bash_login",
                "~/.bash_logout",
                "~/.profile",
                "~/.zshrc",
                "~/.zprofile",
                "~/.zshenv",
                "~/.zlogin",
                "~/.zlogout",
                "~/.config/fish/**",
            ],
            Write,
            Ask,
        ),
        path("builtin:git-hooks", &["**/.git", "**/.git/**"], Write, Ask),
        path(
            "builtin:git-config",
            &["~/.gitconfig", "~/.config/git/**"],
            Write,
            Ask,
        ),
        path(
            "builtin:autostart",
            &[
                "~/.config/systemd/**",
                "~/.config/autostart/**",
                "~/Library/LaunchAgents/**",
            ],
            Write,
            Ask,
        ),
        path(
            "builtin:user-bin",
            &["~/.local/bin/**", "~/bin/**", "~/.cargo/bin/**"],
            Write,
            Ask,
        ),
        path(
            "builtin:toolchain-config",
            &["~/.cargo/config.toml", "~/.cargo/config"],
            Write,
            Ask,
        ),
        path("builtin:workdir-write", &["{workdir}/**"], Write, Allow),
        path(
            "builtin:temp-write",
            &[
                "/tmp/**",
                "/private/tmp/**",
                "/var/tmp/**",
                "/private/var/tmp/**",
                "/var/folders/*/*/T/**",
                "/private/var/folders/*/*/T/**",
                "/dev/null",
                "/dev/stdout",
                "/dev/stderr",
                "/dev/tty",
                "/dev/fd/**",
            ],
            Write,
            Allow,
        ),
        path("builtin:outside-workdir", &["/**"], Write, Ask),
        cmd("builtin:git-push", "git push", Ask),
        cmd("builtin:chmod-777", "chmod *777*", Ask),
        cmd("builtin:git-config-get", "git config --get*", Allow),
        cmd("builtin:git-config-list", "git config -l*", Allow),
        cmd("builtin:git-config-list-long", "git config --list*", Allow),
        cmd("builtin:git-config-cmd", "git config", Ask),
        cmd("builtin:git-inline-config", "git -c *", Ask),
        cmd("builtin:git-remote-set-url", "git remote set-url", Ask),
        cmd("builtin:git-remote-add", "git remote add", Ask),
        cmd("builtin:sudo", "sudo", Deny),
        cmd("builtin:sudoedit", "sudoedit", Deny),
        cmd("builtin:su", "su", Deny),
        cmd("builtin:doas", "doas", Deny),
        cmd("builtin:pkexec", "pkexec", Deny),
        cmd("builtin:run0", "run0", Deny),
        cmd("builtin:crontab-list", "crontab -l", Allow),
        cmd("builtin:crontab", "crontab", Ask),
        cmd("builtin:at", "at", Ask),
        cmd("builtin:systemd-run", "systemd-run", Ask),
        cmd("builtin:launchctl", "launchctl", Ask),
        PolicyRule::host("*", Ask).with_id("builtin:new-host"),
    ]
}

/// Explicit-id check per layer: built-in ids are `builtin:[a-z0-9-]{1,48}` (and not
/// reserved); user and project ids keep `[A-Za-z0-9._-]{1,64}`, which has no `:`.
fn explicit_id_ok(layer: Layer, id: &str) -> bool {
    match layer {
        Layer::Builtin => id.strip_prefix("builtin:").is_some_and(|name| {
            (1..=48).contains(&name.len())
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && !RESERVED_BUILTIN_IDS.contains(&id)
        }),
        Layer::User | Layer::Project => {
            (1..=64).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        }
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
    /// The pattern matches (nearly) everything: only wildcards, `/` and `.`.
    catch_all: bool,
    /// The pattern has no glob metacharacters (`* ? [ {`).
    literal: bool,
    /// Approving the rule remembers its bare id: a literal host rule, or a literal
    /// path rule with one pattern and an explicit `read`/`write` op.
    by_id: bool,
}

/// True if `pattern` has no literal characters other than `/` and `.`.
fn pattern_is_catch_all(pattern: &str, is_command: bool) -> bool {
    if is_command {
        return pattern == "*";
    }
    pattern.chars().all(|c| matches!(c, '*' | '?' | '/' | '.')) && pattern.contains(['*', '?'])
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

impl CompiledPolicy {
    /// True when any layer holds a path rule.
    pub fn has_path_rules(&self) -> bool {
        [&self.builtin, &self.user, &self.project]
            .iter()
            .any(|l| l.iter().any(|r| matches!(r.matcher, Matcher::Path { .. })))
    }

    fn all_rules(&self) -> impl Iterator<Item = &CompiledRule> {
        self.builtin
            .iter()
            .chain(self.user.iter())
            .chain(self.project.iter())
    }

    /// True when no layer has any rule.
    pub fn is_empty(&self) -> bool {
        self.builtin.is_empty() && self.user.is_empty() && self.project.is_empty()
    }

    /// True if some rule (explicit or automatic id) has this id. Synthetic ids
    /// such as `builtin:invalid-host` are not rules and return false.
    pub fn has_rule_id(&self, id: &str) -> bool {
        self.all_rules().any(|r| r.id == id)
    }

    /// True if the rule with this id has a pattern without glob metacharacters
    /// (`* ? [ {`), so it matches one thing and can be approved by id.
    /// Unknown ids return false.
    pub fn is_literal(&self, id: &str) -> bool {
        self.all_rules().any(|r| r.id == id && r.literal)
    }

    /// True if approving the rule with this id remembers the bare id: the rule is
    /// literal and not a command rule (command rules are prefix matches, so they
    /// are approved per command line). Unknown ids return false.
    pub fn approves_by_id(&self, id: &str) -> bool {
        self.all_rules()
            .any(|r| r.id == id && r.by_id)
    }

    /// True if the rule with this id matches (nearly) everything, so approving it
    /// for one subject must not approve all subjects.
    pub fn is_catch_all(&self, id: &str) -> bool {
        self.all_rules().any(|r| r.id == id && r.catch_all)
    }

    /// The decision of the rule with this id (explicit or automatic); `None` for an
    /// unknown id.
    pub fn rule_decision(&self, id: &str) -> Option<Decision> {
        self.all_rules().find(|r| r.id == id).map(|r| r.decision)
    }

    /// True when the user or project layer holds a rule (policy files exist and
    /// have rules), as opposed to only the built-in layer.
    pub fn has_file_rules(&self) -> bool {
        !self.user.is_empty() || !self.project.is_empty()
    }

    /// True when any layer holds a command rule.
    pub fn has_command_rules(&self) -> bool {
        [&self.builtin, &self.user, &self.project]
            .iter()
            .any(|l| l.iter().any(|r| matches!(r.matcher, Matcher::Command(_))))
    }
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
        Self::compile_logical(ctx, None, builtin, user, project)
    }

    /// Like [`Self::compile`], with the working directory as the caller passed it
    /// (absolute, unresolved). `{workdir}` then expands to both that path and
    /// `ctx.workdir`, and a pattern matches if either form matches.
    pub fn compile_logical(
        ctx: &PolicyContext,
        logical_workdir: Option<&Path>,
        builtin: &[PolicyRule],
        user: &[PolicyRule],
        project: &[PolicyRule],
    ) -> Result<Self, PermissionError> {
        let env = ExpansionEnv::new(ctx, logical_workdir)?;
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
        Self::load_layers(ctx, builtin, Some(user), project)
    }

    /// Like [`Self::load_hierarchical_with_paths`], but the user layer is optional.
    pub fn load_layers(
        ctx: &PolicyContext,
        builtin: &[PolicyRule],
        user: Option<&Path>,
        project: &Path,
    ) -> Result<Self, PermissionError> {
        Self::load_layers_logical(ctx, None, builtin, user, project)
    }

    /// Like [`Self::load_layers`], with a logical working directory
    /// (see [`Self::compile_logical`]).
    pub fn load_layers_logical(
        ctx: &PolicyContext,
        logical_workdir: Option<&Path>,
        builtin: &[PolicyRule],
        user: Option<&Path>,
        project: &Path,
    ) -> Result<Self, PermissionError> {
        let user_rules = match user {
            Some(u) => load_policy_file(u)?,
            None => Vec::new(),
        };
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
        Self::compile_logical(ctx, logical_workdir, builtin, &user_rules, &project_rules)
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
    /// Glob-escaped working directories: the canonical one first, then the logical
    /// one when it differs.
    workdir_variants: Vec<String>,
    home_escaped: Option<String>,
    case_insensitive: bool,
}

impl ExpansionEnv {
    fn new(ctx: &PolicyContext, logical: Option<&Path>) -> Result<Self, PermissionError> {
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
        let mut workdir_variants = vec![workdir_escaped];
        if let Some(l) = logical.filter(|l| l.is_absolute()) {
            let l = normalise_path(l);
            if l != workdir {
                if let Some(text) = l.to_str() {
                    workdir_variants.push(escape_glob(text));
                }
            }
        }
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
            workdir_variants,
            home_escaped,
            case_insensitive: ctx.case_insensitive_paths,
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
            if !explicit_id_ok(layer, id) {
                return Err(invalid(match layer {
                    Layer::Builtin => "built-in rule id must match builtin:[a-z0-9-]{1,48}",
                    _ => "rule id must match [A-Za-z0-9._-]{1,64}",
                }));
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
            let mut globs = Vec::new();
            for pat in std::iter::once(p).chain(rule.extra_paths.iter()) {
                globs.extend(compile_path(env, pat).map_err(|e| match e {
                    PathPatternError::Invalid(r) => invalid(&r),
                    PathPatternError::Glob(e) => e,
                })?);
            }
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
            catch_all: pattern_is_catch_all(pattern, command.is_some()),
            literal: !std::iter::once(pattern)
                .chain(rule.extra_paths.iter().map(String::as_str))
                .any(|p| p.contains(['*', '?', '[', '{'])),
            by_id: !std::iter::once(pattern)
                .chain(rule.extra_paths.iter().map(String::as_str))
                .any(|p| p.contains(['*', '?', '[', '{']))
                && (rule.host.is_some()
                    || (rule.path.is_some()
                        && rule.extra_paths.is_empty()
                        && matches!(rule.op, Some(OpFilter::Read | OpFilter::Write)))),
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
    let uses_workdir = raw.contains("{workdir}")
        || !(raw == "~" || raw.starts_with("~/") || raw.starts_with('/') || raw.starts_with("**"));
    let variants: &[String] = if uses_workdir {
        &env.workdir_variants
    } else {
        &env.workdir_variants[..1]
    };
    let mut out = Vec::new();
    for wd in variants {
        out.extend(compile_path_with(env, raw, wd)?);
    }
    Ok(out)
}

fn compile_path_with(
    env: &ExpansionEnv,
    raw: &str,
    workdir_escaped: &str,
) -> Result<Vec<GlobMatcher>, PathPatternError> {
    let invalid = |m: &str| PathPatternError::Invalid(m.to_string());

    let expanded = if raw == "~" || raw.starts_with("~/") {
        let Some(home) = &env.home_escaped else {
            return Ok(Vec::new());
        };
        format!(
            "{home}{}",
            raw[1..].replace("{workdir}", workdir_escaped)
        )
    } else {
        raw.replace("{workdir}", workdir_escaped)
    };
    let anchored = if expanded.starts_with('/') || expanded.starts_with("**") {
        expanded
    } else {
        format!("{workdir_escaped}/{expanded}")
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
                .case_insensitive(env.case_insensitive)
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
