//! Permission-policy decision point for tool calls (P2.2).
//!
//! [`decide`] is a pure function: it asks the tool for its subjects, evaluates
//! them against the compiled policy and says whether the call may run. The
//! agent loop owns everything stateful (prompting, session approvals).

use std::collections::HashSet;
use std::sync::Arc;

use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, PolicyContext};
use hone_proto::ToolCallRequest;
use hone_tools::{SubjectContext, Tool, ToolInput};

/// Synthetic rule id used when a shell line could not be parsed.
pub(crate) const UNPARSED_COMMAND_RULE: &str = "builtin:unparsed-command";
/// Synthetic rule id used for malformed hosts.
pub(crate) const INVALID_HOST_RULE: &str = "builtin:invalid-host";

/// Whether `rule_id` is a synthetic id that must never be remembered.
pub(crate) fn is_synthetic_rule(rule_id: &str) -> bool {
    rule_id == UNPARSED_COMMAND_RULE || rule_id == INVALID_HOST_RULE
}

/// Permission policy attached to an agent.
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    pub compiled: Arc<CompiledPolicy>,
    /// Used for case-insensitivity and the home dir when extracting subjects.
    pub context: PolicyContext,
    /// Rule ids approved up front (populated by `--approve` in P2.3).
    pub pre_approved: Vec<String>,
}

/// Outcome of [`decide`].
#[derive(Debug, Clone)]
pub enum GateDecision {
    Allow,
    Deny { rule_id: String },
    Ask { rule_id: String },
}

/// Decide whether `call` may run. An unknown tool (`None`) has no subjects and
/// is allowed.
pub fn decide(
    policy: &PolicyConfig,
    session_approved: &HashSet<String>,
    tool: Option<&dyn Tool>,
    call: &ToolCallRequest,
) -> GateDecision {
    let Some(tool) = tool else {
        return GateDecision::Allow;
    };
    let sctx = SubjectContext {
        workdir: policy.context.workdir.clone(),
        home: policy.context.home.clone(),
        case_insensitive: policy.context.case_insensitive_paths,
    };
    let found = tool.subjects(&ToolInput::new(call.arguments.clone()), &sctx);

    let approved =
        |id: &str| session_approved.contains(id) || policy.pre_approved.iter().any(|p| p == id);

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
                if first_ask.is_none() && !approved(&rule_id) {
                    first_ask = Some(rule_id);
                }
            }
        }
    }

    // A command we could not parse may hide anything a rule cares about. It
    // never outranks a parsed Deny (returned above) or an earlier parsed Ask.
    let has_rules = policy.compiled.has_command_rules() || policy.compiled.has_path_rules();
    if found.unparsed && has_rules && !approved(UNPARSED_COMMAND_RULE) {
        first_ask.get_or_insert_with(|| UNPARSED_COMMAND_RULE.to_owned());
    }

    match first_ask {
        Some(rule_id) => GateDecision::Ask { rule_id },
        None => GateDecision::Allow,
    }
}
