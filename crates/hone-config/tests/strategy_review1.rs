#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Review round 1 of `docs/plans/strategy-registry-spec.md` in `hone-config`:
//! S11 (selection half: a project ratio is only ever a cap), S12 (escaped,
//! truncated values in errors), S15 (`exclude_kinds` validated), S18 (one parser
//! for `HONE_PHASE_COMPACTION`), S19 (`top_k >= 1`) and S20 (dropped project
//! sources warn). Same API as `strategy_selection.rs`.

use hone_config::ConfigError;
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn layer(path: &str, raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml(path, raw).unwrap_or_else(|e| panic!("fixture should parse: {e}\n{raw}"))
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn select(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> Result<StrategySelection, ConfigError> {
    StrategySelection::from_sources(u, p, &no_env, &Flags::default())
}

fn parts(e: &ConfigError) -> (String, String) {
    match e {
        ConfigError::InvalidField { path, reason } => (path.display().to_string(), reason.clone()),
        other => panic!("expected InvalidField, got {other:?}"),
    }
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

// ---- S11 -----------------------------------------------------------------------------------------------------

#[test]
fn test_s11_a_project_ratio_without_a_user_value_should_not_become_the_budget_ratio() {
    // The selection cannot know the model's default ratio (0.5 for most models), so a
    // project value on its own must not be recorded as THE ratio: it is applied as
    // min(user or per-model default, project) where the model is known.
    let p = layer(PROJECT, "[context]\nbudget_ratio = 0.6\n");

    let s = select(None, Some(&p)).unwrap();

    assert_ne!(
        s.budget_ratio.value,
        Some(0.6),
        "a project 0.6 recorded as the effective ratio would raise a 0.5 model"
    );
}

#[test]
fn test_s11_a_project_ratio_above_the_users_should_warn_naming_the_file() {
    let u = layer(USER, "[context]\nbudget_ratio = 0.4\n");
    let p = layer(PROJECT, "[context]\nbudget_ratio = 0.6\n");

    let s = select(Some(&u), Some(&p)).unwrap();

    assert_eq!(s.budget_ratio.value, Some(0.4));
    assert!(
        s.warnings
            .iter()
            .any(|w| w.path.to_str() == Some(PROJECT) && w.message.contains("budget_ratio")),
        "{:?}",
        s.warnings
    );
}

// ---- S12: errors never echo raw control characters ----------------------------------------------------------

const EVIL: &str = r"\u001b]0;x\u0007\u001b[2K\r\nhone: warning: forged line ";

#[test]
fn test_s12_an_invalid_name_in_a_file_should_be_shown_escaped_and_truncated() {
    for (key, table) in [
        ("memory.recaller", "memory"),
        ("memory.writer", "memory"),
        ("context.compactor", "context"),
    ] {
        let leaf = key.rsplit('.').next().unwrap();
        for file in [USER, PROJECT] {
            let raw = format!("[{table}]\n{leaf} = \"{EVIL}{}\"\n", "A".repeat(200));
            let l = layer(file, &raw);
            let (u, p) = if file == USER { (Some(&l), None) } else { (None, Some(&l)) };

            let e = select(u, p).expect_err("an invalid name");

            let (path, reason) = parts(&e);
            assert_eq!(path, file);
            let shown = e.to_string();
            assert!(!has_control(&reason) && !has_control(&shown), "{key}: control characters in {shown:?}");
            assert!(!shown.contains('\n'), "{key}: no forged line: {shown:?}");
            assert!(reason.contains(key), "{reason}");
            assert!(
                !reason.contains(&"A".repeat(81)),
                "{key}: the value is truncated to 80 characters: {reason}"
            );
        }
    }
}

#[test]
fn test_s12_an_invalid_skeleton_use_should_be_escaped_too() {
    let l = layer(PROJECT, &format!("[context.skeleton]\nuse = \"{EVIL}\"\n"));

    let e = select(None, Some(&l)).expect_err("invalid");

    let shown = e.to_string();
    assert!(!has_control(&shown), "{shown:?}");
}

#[test]
fn test_s12_an_invalid_env_var_or_flag_value_should_be_escaped_too() {
    let evil = "bad\u{1b}[2K\r\nhone: warning: forged".to_owned() + &"B".repeat(200);

    let e = StrategySelection::from_sources(
        None,
        None,
        &|k| (k == "HONE_SKELETON_STRATEGY").then(|| evil.clone()),
        &Flags::default(),
    )
    .expect_err("invalid env value");
    let shown = e.to_string();
    assert!(!has_control(&shown), "env: {shown:?}");
    assert!(!shown.contains(&"B".repeat(81)), "env: truncated: {shown}");
    assert!(shown.contains("HONE_SKELETON_STRATEGY"));

    let e = StrategySelection::from_sources(
        None,
        None,
        &no_env,
        &Flags {
            skeleton_strategy: Some(evil.clone()),
            ..Default::default()
        },
    )
    .expect_err("invalid flag value");
    let shown = e.to_string();
    assert!(!has_control(&shown), "flag: {shown:?}");
    assert!(shown.contains("--skeleton-strategy"));
}

#[test]
fn test_s12_a_plain_invalid_value_should_still_name_the_valid_names() {
    let l = layer(USER, "[memory]\nrecaller = \"nope\"\n");

    let e = select(Some(&l), None).expect_err("invalid");

    let (_, reason) = parts(&e);
    assert!(reason.contains("nope") && reason.contains("bm25") && reason.contains("none"), "{reason}");
}

// ---- S15: exclude_kinds validated -------------------------------------------------------------------------------

#[test]
fn test_s15_an_unknown_exclude_kind_should_be_a_config_error_naming_file_key_and_valid_kinds() {
    for file in [USER, PROJECT] {
        let l = layer(file, "[memory]\nexclude_kinds = [\"outcome\", \"bogus\"]\n");
        let (u, p) = if file == USER { (Some(&l), None) } else { (None, Some(&l)) };

        let e = select(u, p).expect_err("an unknown kind must be rejected");

        let (path, reason) = parts(&e);
        assert_eq!(path, file);
        assert!(reason.contains("memory.exclude_kinds"), "{reason}");
        assert!(reason.contains("bogus"), "{reason}");
        for kind in ["conversation", "user", "feedback", "project", "reference", "outcome"] {
            assert!(reason.contains(kind), "valid kind {kind} listed: {reason}");
        }
    }
}

#[test]
fn test_s15_known_kinds_should_be_accepted_in_any_case() {
    let l = layer(USER, "[memory]\nexclude_kinds = [\"Outcome\", \"conversation\"]\n");
    assert!(select(Some(&l), None).is_ok());
    let l = layer(USER, "[memory]\nexclude_kinds = []\n");
    assert!(select(Some(&l), None).is_ok());
}

// ---- S18: one parser for HONE_PHASE_COMPACTION -----------------------------------------------------------------------

#[test]
fn test_s18_the_env_var_should_be_trimmed_and_case_insensitive() {
    for on in ["1", " 1", "1 ", "true", "TRUE", "True", "on", "On", "ON", " on "] {
        let s = StrategySelection::from_sources(
            None,
            None,
            &|k| (k == "HONE_PHASE_COMPACTION").then(|| on.to_owned()),
            &Flags::default(),
        )
        .unwrap();
        assert!(s.graduated_phase.value, "{on:?} enables phase compaction");
    }
    for off in ["0", "false", "off", "", "yes", "2", " "] {
        let s = StrategySelection::from_sources(
            None,
            None,
            &|k| (k == "HONE_PHASE_COMPACTION").then(|| off.to_owned()),
            &Flags::default(),
        )
        .unwrap();
        assert!(!s.graduated_phase.value, "{off:?} leaves it off");
    }
}

// ---- S19: top_k >= 1 ------------------------------------------------------------------------------------------------------

#[test]
fn test_s19_top_k_zero_should_be_an_error_from_any_file() {
    for file in [USER, PROJECT] {
        let l = layer(file, "[memory]\ntop_k = 0\n");
        let (u, p) = if file == USER { (Some(&l), None) } else { (None, Some(&l)) };

        let e = select(u, p).expect_err("top_k = 0 must be rejected");

        let (path, reason) = parts(&e);
        assert_eq!(path, file);
        assert!(reason.contains("memory.top_k"), "{reason}");
        assert!(reason.contains("none"), "points at recaller = \"none\": {reason}");
    }
    let l = layer(USER, "[memory]\ntop_k = 1\n");
    assert_eq!(select(Some(&l), None).unwrap().top_k.value, 1);
}

// ---- S20: dropped project sources are reported ---------------------------------------------------------------------------

#[test]
fn test_s20_a_project_source_the_user_does_not_have_should_warn_naming_the_file() {
    let u = layer(USER, "[context]\nsources = [\"recall\", \"skills\"]\n");
    let p = layer(PROJECT, "[context]\nsources = [\"recall\", \"skeleton\"]\n");

    let s = select(Some(&u), Some(&p)).unwrap();

    assert_eq!(s.sources.value, ["recall"], "the intersection stands");
    let w: Vec<_> = s
        .warnings
        .iter()
        .filter(|w| w.path.to_str() == Some(PROJECT))
        .collect();
    assert_eq!(w.len(), 1, "{:?}", s.warnings);
    assert!(w[0].message.contains("skeleton"), "{}", w[0].message);
}

#[test]
fn test_s20_a_project_source_list_inside_the_default_sources_should_not_warn() {
    let p = layer(PROJECT, "[context]\nsources = [\"recall\", \"skills\"]\n");

    let s = select(None, Some(&p)).unwrap();

    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
}
