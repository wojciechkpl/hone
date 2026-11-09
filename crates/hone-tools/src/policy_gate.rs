//! Permission-policy decision point for tool calls (P2.2, moved here in P2.3).
//!
//! [`decide`] is a pure function: it asks the tool for its subjects, evaluates
//! them against the compiled policy and says whether the call may run. Callers
//! (the agent loop, the MCP server, honed) own everything stateful: prompting
//! and session approvals.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hone_config::policy::{CompiledPolicy, PathOp, PolicyContext, Subject};
use hone_config::{Decision, PermissionError};
use hone_proto::ToolCallRequest;

use crate::{SubjectContext, Tool, ToolInput};

/// Synthetic rule id used when a shell line could not be parsed.
pub const UNPARSED_COMMAND_RULE: &str = "builtin:unparsed-command";
/// Synthetic rule id used for malformed hosts.
pub const INVALID_HOST_RULE: &str = "builtin:invalid-host";

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

    let pre = |id: &str| policy.pre_approved.iter().any(|p| p == id);
    // A literal rule is approved by its id; any rule with a wildcard only by the
    // (rule, subject) key. `pre_approved` ids (named by the user) cover everything.
    let approved_for = |id: &str, subject: Option<&Subject>| {
        pre(id)
            || if policy.compiled.is_literal(id) {
                session_approved.contains(id)
            } else {
                subject.is_some_and(|s| session_approved.contains(&approval_key(id, s)))
            }
    };

    // Evaluate each subject on its own so one approved rule never covers a
    // different Ask rule in the same call. A Deny is never dropped.
    let mut first_ask: Option<String> = None;
    for subject in &found.subjects {
        let verdict = policy.compiled.evaluate(subject);
        let rule_id = verdict.rule_id.unwrap_or_default();
        match verdict.decision {
            Decision::Allow => {}
            Decision::Deny => return GateDecision::Deny { rule_id },
            Decision::Ask => {
                if first_ask.is_none() && !approved_for(&rule_id, Some(subject)) {
                    first_ask = Some(rule_id);
                }
            }
        }
    }

    // A command we could not parse may hide anything a rule cares about. It
    // never outranks a parsed Deny (returned above) or an earlier parsed Ask.
    let has_rules = policy.compiled.has_command_rules() || policy.compiled.has_path_rules();
    if found.unparsed && has_rules && !approved_for(UNPARSED_COMMAND_RULE, None) {
        first_ask.get_or_insert_with(|| UNPARSED_COMMAND_RULE.to_owned());
    }

    match first_ask {
        Some(rule_id) => GateDecision::Ask { rule_id },
        None => GateDecision::Allow,
    }
}

/// What to remember when the user approves `rule_id` for the rest of the session:
/// the bare rule id for a literal rule, or else the [`approval_key`] of every subject
/// of `call` that this rule asked about.
pub fn session_approval_entries(
    policy: &PolicyConfig,
    tool: Option<&dyn Tool>,
    call: &ToolCallRequest,
    rule_id: &str,
) -> Vec<String> {
    if is_synthetic_rule(rule_id) {
        return Vec::new();
    }
    if policy.compiled.is_literal(rule_id) {
        return vec![rule_id.to_owned()];
    }
    let Some(tool) = tool else {
        return Vec::new();
    };
    let found = tool.subjects(
        &ToolInput::new(call.arguments.clone()),
        &subject_context(policy),
    );
    found
        .subjects
        .iter()
        .filter(|s| {
            let v = policy.compiled.evaluate(s);
            v.decision == Decision::Ask && v.rule_id.as_deref() == Some(rule_id)
        })
        .map(|s| approval_key(rule_id, s))
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
/// Returns `Ok(None)` when neither the user file
/// (`<home>/.config/hone/permissions.toml`) nor the project file
/// (`<workdir>/.hone/permissions.toml`) exists, which is exactly the
/// behaviour without a policy. A relative `home` counts as unknown.
pub fn resolve_permission_policy(
    working_dir: &Path,
    home: Option<PathBuf>,
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
    if user.is_none() && !exists(&project) {
        return Ok(None);
    }

    if let Some(e) = canon_err {
        return Err(PermissionError::Io(e));
    }

    let context = PolicyContext::for_platform(workdir, home);
    let compiled = CompiledPolicy::load_layers(&context, &[], user.as_deref(), &project)?;
    Ok(Some(PolicyConfig {
        compiled: Arc::new(compiled),
        context,
        pre_approved: Vec::new(),
    }))
}

/// Validate `--approve` / `approve` rule ids against the loaded policy.
///
/// Every id must name a rule of the compiled policy (explicit or automatic) or
/// be one of the synthetic ids. Returns the ids unchanged and in order. The
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
        if !is_synthetic_rule(id) && !policy.compiled.has_rule_id(id) {
            return Err(format!(
                "--approve: unknown rule id '{}'",
                id.escape_default()
            ));
        }
    }
    Ok(ids.to_vec())
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
    if allow_env == Some("1") {
        return Ok(Some(
            "this provider runs its own tools, which bypass the permission policy; \
             continuing because HONE_ALLOW_UNGATED_PROVIDER=1"
                .to_owned(),
        ));
    }
    Err(
        "this provider runs its own tools, which the permission policy can't check; \
         remove the policy files, use another provider, or set HONE_ALLOW_UNGATED_PROVIDER=1"
            .to_owned(),
    )
}
