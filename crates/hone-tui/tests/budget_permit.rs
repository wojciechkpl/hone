//! Slice 4 RED-phase tests for the `:budget <usd>` and `:permit <tool>` TUI commands.
//!
//! Contract:
//! - `Command::Budget(f64)` variant parses from `/budget <positive-amount>`.
//! - `Command::Permit(String)` variant parses from `/permit <tool-name>`.
//! - Both appear in `COMMAND_NAMES` (reachable via `Command::completions`).
//! - `PendingGovernance` struct holds per-run governance overrides (budget + permits).
//! - `App` gains `set_pending_budget`, `add_pending_permit`, `pending_governance`,
//!   and `take_pending_governance` methods.
//! - `PendingGovernance` exposes `apply_to_budget(opt: &mut Option<f64>)` and
//!   `apply_to_policy(policy: &mut ToolApprovalPolicy)` helpers — the split design
//!   avoids constructing a full `AgentConfig` in tests.
//!
//! These tests are INTENTIONALLY failing until the rust-expert adds the required
//! symbols in the green phase.  The implementer MUST NOT modify this file.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hone_proto::ToolApprovalPolicy;
use hone_tui::{
    app::App,
    commands::Command,
    governance::PendingGovernance,
};

// ---------------------------------------------------------------------------
// 1-9. Command parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_budget_command_with_decimal() {
    let cmd = Command::parse("/budget 0.50").expect("parse returned None");
    assert_eq!(cmd, Command::Budget(0.5));
}

#[test]
fn parse_budget_command_with_integer() {
    let cmd = Command::parse("/budget 5").expect("parse returned None");
    assert_eq!(cmd, Command::Budget(5.0));
}

#[test]
fn parse_budget_command_with_negative_is_unknown() {
    let cmd = Command::parse("/budget -1").expect("parse returned None");
    assert!(
        matches!(cmd, Command::Unknown(_)),
        "expected Command::Unknown for negative budget, got: {cmd:?}"
    );
}

#[test]
fn parse_budget_command_with_zero_is_unknown() {
    let cmd = Command::parse("/budget 0").expect("parse returned None");
    assert!(
        matches!(cmd, Command::Unknown(_)),
        "expected Command::Unknown for zero budget (blocks every run), got: {cmd:?}"
    );
}

#[test]
fn parse_budget_command_empty_is_unknown() {
    let cmd = Command::parse("/budget").expect("parse returned None");
    assert!(
        matches!(cmd, Command::Unknown(_)),
        "expected Command::Unknown for missing budget argument, got: {cmd:?}"
    );
}

#[test]
fn parse_budget_command_non_numeric_is_unknown() {
    let cmd = Command::parse("/budget abc").expect("parse returned None");
    assert!(
        matches!(cmd, Command::Unknown(_)),
        "expected Command::Unknown for non-numeric budget, got: {cmd:?}"
    );
}

#[test]
fn parse_permit_command() {
    let cmd = Command::parse("/permit shell").expect("parse returned None");
    assert_eq!(cmd, Command::Permit("shell".to_string()));
}

#[test]
fn parse_permit_command_empty_is_unknown() {
    let cmd = Command::parse("/permit").expect("parse returned None");
    assert!(
        matches!(cmd, Command::Unknown(_)),
        "expected Command::Unknown for missing permit argument, got: {cmd:?}"
    );
}

#[test]
fn parse_permit_command_with_dotted_name() {
    // Tool names may be dotted (e.g. "shell.run").
    let cmd = Command::parse("/permit shell.run").expect("parse returned None");
    assert_eq!(cmd, Command::Permit("shell.run".to_string()));
}

// ---------------------------------------------------------------------------
// 10-11. Completions
// ---------------------------------------------------------------------------

#[test]
fn budget_in_command_completions() {
    let completions = Command::completions("b");
    assert!(
        completions.contains(&"budget"),
        "expected 'budget' in completions for prefix 'b', got: {completions:?}"
    );
}

#[test]
fn permit_in_command_completions() {
    let completions = Command::completions("p");
    assert!(
        completions.contains(&"permit"),
        "expected 'permit' in completions for prefix 'p', got: {completions:?}"
    );
}

// ---------------------------------------------------------------------------
// 12-16. App state machine
// ---------------------------------------------------------------------------

#[test]
fn set_pending_budget_stores_value() {
    let mut app = App::new();
    app.set_pending_budget(0.5);
    assert_eq!(
        app.pending_governance().budget_usd,
        Some(0.5),
        "set_pending_budget must store the value in pending_governance.budget_usd"
    );
}

#[test]
fn add_pending_permit_stores_tool() {
    let mut app = App::new();
    app.add_pending_permit("shell");
    assert_eq!(
        app.pending_governance().permits,
        vec!["shell".to_string()],
        "add_pending_permit must append the tool name to pending_governance.permits"
    );
}

#[test]
fn add_pending_permit_idempotent() {
    let mut app = App::new();
    app.add_pending_permit("shell");
    app.add_pending_permit("shell");
    assert_eq!(
        app.pending_governance().permits,
        vec!["shell".to_string()],
        "add_pending_permit must be idempotent — duplicate entries are not allowed"
    );
}

#[test]
fn add_multiple_pending_permits() {
    let mut app = App::new();
    app.add_pending_permit("shell");
    app.add_pending_permit("edit_file");
    assert_eq!(
        app.pending_governance().permits,
        vec!["shell".to_string(), "edit_file".to_string()],
        "add_pending_permit must preserve insertion order across distinct tools"
    );
}

#[test]
fn take_pending_governance_drains() {
    let mut app = App::new();
    app.set_pending_budget(1.0);
    app.add_pending_permit("shell");
    app.add_pending_permit("edit_file");

    let first = app.take_pending_governance();
    assert_eq!(first.budget_usd, Some(1.0), "first take must return the set budget");
    assert_eq!(
        first.permits,
        vec!["shell".to_string(), "edit_file".to_string()],
        "first take must return all permits"
    );

    // Second take must return the default (drained) value.
    let second = app.take_pending_governance();
    assert_eq!(
        second,
        PendingGovernance::default(),
        "second take must return PendingGovernance::default() — field must be drained"
    );
}

// ---------------------------------------------------------------------------
// 17-18. PendingGovernance::apply_to_budget and apply_to_policy helpers
// ---------------------------------------------------------------------------

#[test]
fn apply_pending_governance_sets_budget_limit_and_permits() {
    let gov = PendingGovernance {
        budget_usd: Some(0.5),
        permits: vec!["shell".to_string()],
    };

    // Test apply_to_budget.
    let mut budget_opt: Option<f64> = None;
    gov.clone().apply_to_budget(&mut budget_opt);
    assert_eq!(
        budget_opt,
        Some(0.5),
        "apply_to_budget must set budget_limit to the pending value"
    );

    // Test apply_to_policy with AskRisky (the default).
    let mut policy = ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    };
    gov.apply_to_policy(&mut policy);
    let ToolApprovalPolicy::AskRisky { always_allowed } = &policy else {
        panic!("expected AskRisky variant after apply_to_policy");
    };
    assert_eq!(
        always_allowed,
        &vec!["shell".to_string()],
        "apply_to_policy must push each permit into always_allowed via approve_always"
    );
}

#[test]
fn apply_pending_governance_is_noop_for_auto_policy() {
    // approve_always is a no-op for the Auto variant; apply_to_policy must
    // respect that — the policy remains exactly Auto after the call.
    let gov = PendingGovernance {
        budget_usd: None,
        permits: vec!["shell".to_string()],
    };

    let mut policy = ToolApprovalPolicy::Auto;
    gov.apply_to_policy(&mut policy);
    assert_eq!(
        policy,
        ToolApprovalPolicy::Auto,
        "apply_to_policy must leave ToolApprovalPolicy::Auto unchanged"
    );
}

// ---------------------------------------------------------------------------
// Post-review regression tests
// ---------------------------------------------------------------------------

/// f64::parse accepts "inf"/"-inf"/"nan" — `inf > 0.0` is `true`, so an
/// `usd > 0.0` guard alone lets `/budget inf` through. Reject by also
/// requiring `usd.is_finite()`.
#[test]
fn parse_budget_command_with_inf_is_unknown() {
    let cmd = Command::parse("/budget inf").expect("parse returned None");
    assert!(
        matches!(cmd, Command::Unknown(_)),
        "/budget inf must be rejected"
    );
}

#[test]
fn parse_budget_command_with_nan_is_unknown() {
    // `nan > 0.0` is false, so a naive guard already rejects this — but
    // pin the behaviour so it never silently regresses.
    let cmd = Command::parse("/budget nan").expect("parse returned None");
    assert!(
        matches!(cmd, Command::Unknown(_)),
        "/budget nan must be rejected"
    );
}

/// `apply_to_budget` must NOT clobber a pre-existing limit when the
/// pending override is `None`. Otherwise a no-op `PendingGovernance`
/// wipes a budget set elsewhere (e.g. via the agent's TOML config).
#[test]
fn apply_to_budget_preserves_existing_when_pending_is_none() {
    let gov = PendingGovernance::default(); // budget_usd: None
    let mut existing: Option<f64> = Some(2.50);
    gov.apply_to_budget(&mut existing);
    assert_eq!(
        existing,
        Some(2.50),
        "apply_to_budget must not overwrite when pending is None"
    );
}
