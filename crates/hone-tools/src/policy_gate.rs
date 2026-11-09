//! Permission-policy decision point for tool calls (P2.2, moved here in P2.3).
//!
//! [`decide`] is a pure function: it asks the tool for its subjects, evaluates
//! them against the compiled policy and says whether the call may run. Callers
//! (the agent loop, the MCP server, honed) own everything stateful: prompting
//! and session approvals.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hone_config::policy::{
    CompiledPolicy, PathOp, PolicyContext, Subject, builtin_rules,
};
use hone_config::{Decision, PermissionError};
use hone_proto::ToolCallRequest;

use crate::subjects::lexical_subject_path;
use crate::{SubjectContext, Tool, ToolInput, subject_path};

/// Synthetic rule id used when a shell line could not be parsed.
pub const UNPARSED_COMMAND_RULE: &str = "builtin:unparsed-command";
/// Synthetic rule id used for malformed hosts.
pub const INVALID_HOST_RULE: &str = "builtin:invalid-host";

/// Rule id of the pipe-to-shell deny (built-ins only). Never approvable.
pub const PIPE_TO_SHELL_RULE: &str = "builtin:pipe-to-shell";
/// Tool-level ask ids (built-ins only); approved by id.
pub const COMPUTER_CONTROL_RULE: &str = "builtin:computer-control";
pub const SCREEN_CAPTURE_RULE: &str = "builtin:screen-capture";

/// Environment variable that turns the built-in rules on (opt-in; exactly `1`).
pub const POLICY_BUILTINS_ENV: &str = "HONE_POLICY_BUILTINS";

/// True only for exactly `"1"`. Callers pass `std::env::var(POLICY_BUILTINS_ENV).ok().as_deref()`;
/// this function never reads the environment.
pub fn builtins_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

/// One-line warning for a `HONE_POLICY_BUILTINS` value that is set but not exactly
/// `1`: built-ins stay off. `None` for unset, empty or `1`.
pub fn builtins_env_warning(value: Option<&str>) -> Option<String> {
    match value {
        Some(v) if !v.is_empty() && v != "1" => Some(format!(
            "{POLICY_BUILTINS_ENV}={v:?} is not recognised (only 1 turns built-in rules on); \
             built-in rules are off"
        )),
        _ => None,
    }
}

/// Whether `rule_id` is a synthetic id that must never be remembered.
pub fn is_synthetic_rule(rule_id: &str) -> bool {
    rule_id == UNPARSED_COMMAND_RULE || rule_id == INVALID_HOST_RULE
}

/// Permission policy attached to an agent.
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    pub compiled: Arc<CompiledPolicy>,
    /// Used for case-insensitivity and the home dir when extracting subjects.
    pub context: PolicyContext,
    /// Rule ids approved up front (`--approve`). They cover every subject.
    pub pre_approved: Vec<String>,
    /// True when the built-in layer was loaded. Turns on pipe-to-shell denial,
    /// tool-level asks and output redaction.
    pub builtins: bool,
}

/// Outcome of [`decide`].
#[derive(Debug, Clone)]
pub enum GateDecision {
    Allow,
    Deny { rule_id: String },
    Ask { rule_id: String },
}

/// Session-approval key for a (rule, subject) pair. Used for catch-all rules,
/// where approving one subject must not approve every subject. Opaque.
pub fn approval_key(rule_id: &str, subject: &Subject) -> String {
    let body = match subject {
        Subject::Command(c) => format!(
            "cmd\u{0}{}",
            c.split_whitespace().collect::<Vec<_>>().join(" ")
        ),
        Subject::Path { path, op } => {
            let o = match op {
                PathOp::Read => 'r',
                PathOp::Write => 'w',
            };
            format!("path\u{0}{o}\u{0}{}", path.to_string_lossy())
        }
        Subject::Host(h) => format!("host\u{0}{}", normalise_host_key(h)),
    };
    format!("{rule_id}\u{0}{body}")
}

/// Like [`approval_key`], for a subject that carries context (leading assignments,
/// wrapper words, working directory). Without context the keys are equal.
pub fn approval_key_with_context(rule_id: &str, subject: &Subject, context: &str) -> String {
    let base = approval_key(rule_id, subject);
    if context.is_empty() {
        base
    } else {
        format!("{base}\u{0}ctx\u{0}{context}")
    }
}

fn normalise_host_key(h: &str) -> String {
    let lower = h.trim().to_lowercase();
    let no_port = match lower.matches(':').count() {
        1 => lower.split_once(':').map_or(lower.as_str(), |(a, _)| a),
        _ => lower.as_str(),
    };
    no_port.trim_end_matches('.').to_owned()
}

fn subject_context(policy: &PolicyConfig) -> SubjectContext {
    SubjectContext {
        workdir: policy.context.workdir.clone(),
        home: policy.context.home.clone(),
        case_insensitive: policy.context.case_insensitive_paths,
    }
}

/// Decide whether `call` may run. An unknown tool (`None`) has no subjects and
/// is allowed.
///
/// `session_approved` holds rule ids (literal rules) and [`approval_key`]s (rules
/// with a wildcard, approved only for the subject being evaluated).
/// `pre_approved` ids always cover every subject. A Deny is never lifted.
pub fn decide(
    policy: &PolicyConfig,
    session_approved: &HashSet<String>,
    tool: Option<&dyn Tool>,
    call: &ToolCallRequest,
) -> GateDecision {
    let Some(tool) = tool else {
        return GateDecision::Allow;
    };
    let found = tool.subjects(
        &ToolInput::new(call.arguments.clone()),
        &subject_context(policy),
    );

    // A nested command string, heredoc script or `source` line never matches a
    // remembered approval (it can still be pre-approved with --approve).
    let no_session = HashSet::new();
    let session_approved = if found.no_remember {
        &no_session
    } else {
        session_approved
    };

    // A pipe into a shell or interpreter is denied outright; no approval lifts it.
    if policy.builtins && found.pipe_to_interpreter {
        return GateDecision::Deny {
            rule_id: PIPE_TO_SHELL_RULE.to_owned(),
        };
    }

    let pre = |id: &str| policy.pre_approved.iter().any(|p| p == id);
    let approved_for = |id: &str, subject: Option<(&Subject, &str)>| {
        pre(id) || session_covers(policy, session_approved, id, subject)
    };

    // Evaluate each subject on its own so one approved rule never covers a
    // different Ask rule in the same call. A Deny is never dropped.
    let mut first_ask: Option<String> = None;
    let all = found
        .subjects
        .iter()
        .enumerate()
        .map(|(i, s)| (s, found.context_of(i)))
        .chain(found.alt_subjects.iter().map(|s| (s, "")));
    for (subject, ctx) in all {
        let verdict = policy.compiled.evaluate(subject);
        let rule_id = verdict.rule_id.unwrap_or_default();
        match verdict.decision {
            Decision::Allow => {}
            Decision::Deny => return GateDecision::Deny { rule_id },
            Decision::Ask => {
                if first_ask.is_none()
                    && !approved_for(&rule_id, Some((subject, ctx)))
                {
                    first_ask = Some(rule_id);
                }
            }
        }
    }

    // Tool-level ask (built-ins only): approved by id, never outranks a Deny above.
    if policy.builtins {
        if let Some(id) = tool.builtin_ask() {
            if !pre(id) && !session_approved.contains(id) {
                first_ask.get_or_insert_with(|| id.to_owned());
            }
        }
    }

    // A command we could not parse may hide anything a rule cares about. It
    // never outranks a parsed Deny (returned above) or an earlier parsed Ask.
    // With built-ins on, a path built from a variable or substitution is unknown too.
    let unparsed = found.unparsed || (policy.builtins && found.var_path);
    let has_rules = policy.compiled.has_command_rules() || policy.compiled.has_path_rules();
    if unparsed && has_rules && !approved_for(UNPARSED_COMMAND_RULE, None) {
        first_ask.get_or_insert_with(|| UNPARSED_COMMAND_RULE.to_owned());
    }

    match first_ask {
        Some(rule_id) => GateDecision::Ask { rule_id },
        None => GateDecision::Allow,
    }
}

/// True if the session set covers `id` for `subject`. A literal non-command rule is
/// covered by its bare id; any other rule only by the (rule, subject) key.
fn session_covers(
    policy: &PolicyConfig,
    session_approved: &HashSet<String>,
    id: &str,
    subject: Option<(&Subject, &str)>,
) -> bool {
    if policy.compiled.approves_by_id(id) {
        session_approved.contains(id)
    } else {
        subject.is_some_and(|(s, ctx)| {
            session_approved.contains(&approval_key_with_context(id, s, ctx))
        })
    }
}

/// What to remember when the user approves `rule_id` for the rest of the session:
/// the bare rule id for a literal path or host rule or a tool-level ask, or else the
/// [`approval_key`] of every subject of `call` that this rule asked about (command
/// rules, even literal ones, are remembered per command line).
pub fn session_approval_entries(
    policy: &PolicyConfig,
    tool: Option<&dyn Tool>,
    call: &ToolCallRequest,
    rule_id: &str,
) -> Vec<String> {
    if is_synthetic_rule(rule_id) {
        return Vec::new();
    }
    if policy.compiled.approves_by_id(rule_id) {
        return vec![rule_id.to_owned()];
    }
    let Some(tool) = tool else {
        return Vec::new();
    };
    if policy.builtins && tool.builtin_ask() == Some(rule_id) {
        return vec![rule_id.to_owned()];
    }
    let found = tool.subjects(
        &ToolInput::new(call.arguments.clone()),
        &subject_context(policy),
    );
    if found.no_remember {
        return Vec::new();
    }
    found
        .subjects
        .iter()
        .enumerate()
        .map(|(i, s)| (s, found.context_of(i)))
        .chain(found.alt_subjects.iter().map(|s| (s, "")))
        .filter(|(s, _)| {
            let v = policy.compiled.evaluate(s);
            v.decision == Decision::Ask && v.rule_id.as_deref() == Some(rule_id)
        })
        .map(|(s, ctx)| approval_key_with_context(rule_id, s, ctx))
        .collect()
}

/// Home directory used to find the user-level policy: `$HOME` if absolute, else
/// the passwd entry. Always absolute when `Some`.
pub fn default_policy_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .or_else(dirs_next::home_dir)
        .filter(|h| h.is_absolute())
}

const USER_FILE: [&str; 3] = [".config", "hone", "permissions.toml"];
const PROJECT_FILE: [&str; 2] = [".hone", "permissions.toml"];

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Resolve the permission policy for an agent running in `working_dir`.
///
/// With `builtins == false`, returns `Ok(None)` when neither the user file
/// (`<home>/.config/hone/permissions.toml`) nor the project file
/// (`<workdir>/.hone/permissions.toml`) exists, which is exactly the
/// behaviour without a policy. With `builtins == true` the built-in layer is
/// always loaded, so the result is always `Some`. A relative `home` counts as unknown.
pub fn resolve_permission_policy(
    working_dir: &Path,
    home: Option<PathBuf>,
    builtins: bool,
) -> Result<Option<PolicyConfig>, PermissionError> {
    let home = home.filter(|h| h.is_absolute());
    // A workdir that can't be canonicalised is only an error once there is a
    // policy file to apply (no files means no policy, as today).
    let (workdir, canon_err) = match std::fs::canonicalize(working_dir) {
        Ok(w) => (w, None),
        Err(e) => (working_dir.to_path_buf(), Some(e)),
    };

    let user = home
        .as_ref()
        .map(|h| USER_FILE.iter().fold(h.clone(), |p, c| p.join(c)))
        .filter(|p| exists(p));
    let project = PROJECT_FILE.iter().fold(workdir.clone(), |p, c| p.join(c));
    if !builtins && user.is_none() && !exists(&project) {
        return Ok(None);
    }

    if let Some(e) = canon_err {
        return Err(PermissionError::Io(e));
    }

    let context = PolicyContext::for_platform(workdir, home);
    let builtin = if builtins { builtin_rules() } else { Vec::new() };
    // The path the caller passed (absolute, unresolved) is a second spelling of the workdir.
    let logical = std::path::absolute(working_dir).ok();
    let compiled = CompiledPolicy::load_layers_logical(
        &context,
        logical.as_deref(),
        &builtin,
        user.as_deref(),
        &project,
    )?;
    Ok(Some(PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: Vec::new(),
        builtins,
    }))
}

/// Validate `--approve` / `approve` rule ids against the loaded policy.
///
/// Every id must name a rule of the compiled policy (explicit or automatic) or
/// be one of the synthetic ids. With built-ins on, the tool-level ids and
/// `builtin:pipe-to-shell` are accepted too. Returns the ids unchanged and in order. The
/// error text has no `hone: ` prefix and escapes control characters in ids.
pub fn validate_approvals(
    policy: Option<&PolicyConfig>,
    ids: &[String],
) -> Result<Vec<String>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let Some(policy) = policy else {
        return Err("--approve given but no permission policy is loaded".to_owned());
    };
    for id in ids {
        let always_denies = (policy.builtins && id == PIPE_TO_SHELL_RULE)
            || policy.compiled.rule_decision(id) == Some(Decision::Deny);
        if always_denies {
            return Err(format!(
                "--approve: rule '{}' always denies; --approve only lifts ask rules",
                id.escape_default()
            ));
        }
        let builtin_only = policy.builtins
            && matches!(
                id.as_str(),
                PIPE_TO_SHELL_RULE | COMPUTER_CONTROL_RULE | SCREEN_CAPTURE_RULE
            );
        if !is_synthetic_rule(id) && !builtin_only && !policy.compiled.has_rule_id(id) {
            return Err(format!(
                "--approve: unknown rule id '{}'",
                id.escape_default()
            ));
        }
    }
    Ok(ids.to_vec())
}

/// What a [`ReadFilter`] says about one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadAccess {
    Allowed,
    /// The policy asks and the ask has not been approved.
    NeedsApproval,
    Denied,
}

/// Decides whether a directory walker (`grep`, `glob`) may touch a path.
pub type ReadFilter = Arc<dyn Fn(&Path) -> ReadAccess + Send + Sync>;

/// Build a [`ReadFilter`] from the policy and a snapshot of the session approvals.
///
/// Each candidate path is canonicalised like a tool subject, evaluated as a read,
/// and mapped: Allow to `Allowed`, Deny to `Denied`, Ask to `Allowed` when approved
/// (same rules as [`decide`]) and `NeedsApproval` otherwise.
pub fn read_filter(policy: &PolicyConfig, session_approved: &HashSet<String>) -> ReadFilter {
    let policy = policy.clone();
    let session = session_approved.clone();
    let sctx = subject_context(&policy);
    Arc::new(move |path: &Path| {
        // The path as resolved and as written; the stricter verdict wins.
        let raw = path.to_string_lossy();
        let canonical = subject_path(&raw, &sctx);
        let lexical = lexical_subject_path(&raw, &sctx);
        let mut forms = vec![canonical.clone()];
        if lexical != canonical {
            forms.push(lexical);
        }
        let mut access = ReadAccess::Allowed;
        for form in forms {
            let subject = Subject::Path {
                path: form,
                op: PathOp::Read,
            };
            let verdict = policy.compiled.evaluate(&subject);
            let this = match verdict.decision {
                Decision::Allow => ReadAccess::Allowed,
                Decision::Deny => return ReadAccess::Denied,
                Decision::Ask => {
                    let id = verdict.rule_id.unwrap_or_default();
                    let approved = policy.pre_approved.contains(&id)
                        || session_covers(&policy, &session, &id, Some((&subject, "")));
                    if approved {
                        ReadAccess::Allowed
                    } else {
                        ReadAccess::NeedsApproval
                    }
                }
            };
            if this == ReadAccess::NeedsApproval {
                access = ReadAccess::NeedsApproval;
            }
        }
        access
    })
}

/// Decide whether a provider that runs its own tools may be used.
///
/// A provider that executes tools itself bypasses the gate, so under a
/// non-empty policy it is refused unless `allow_env` is exactly `"1"`, in
/// which case a one-line warning is returned. No policy, an empty policy or a
/// provider that doesn't run its own tools yields `Ok(None)`. The refusal text
/// starts with `this provider `; callers may insert the provider id.
pub fn check_provider_gating(
    policy: Option<&PolicyConfig>,
    runs_own_tools: bool,
    allow_env: Option<&str>,
) -> Result<Option<String>, String> {
    let non_empty = policy.is_some_and(|p| !p.compiled.is_empty());
    if !runs_own_tools || !non_empty {
        return Ok(None);
    }
    let builtins_only = policy.is_some_and(|p| p.builtins && !p.compiled.has_file_rules());
    let redaction_note = if policy.is_some_and(|p| p.builtins) {
        "; tool output from this provider is not redacted"
    } else {
        ""
    };
    if allow_env == Some("1") {
        return Ok(Some(format!(
            "this provider runs its own tools, which bypass the permission policy; \
             continuing because HONE_ALLOW_UNGATED_PROVIDER=1{redaction_note}"
        )));
    }
    if builtins_only {
        return Err(
            "this provider runs its own tools, which the permission policy can't check, and the \
             policy is only the built-in rules: unset HONE_POLICY_BUILTINS, use another \
             provider, or set HONE_ALLOW_UNGATED_PROVIDER=1"
                .to_owned(),
        );
    }
    Err(
        "this provider runs its own tools, which the permission policy can't check; \
         remove the policy files, use another provider, or set HONE_ALLOW_UNGATED_PROVIDER=1"
            .to_owned(),
    )
}
