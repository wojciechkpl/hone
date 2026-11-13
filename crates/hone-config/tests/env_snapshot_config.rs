#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E1 and E8 of `docs/plans/env-snapshot-spec.md`: `env-snapshot` as a source name,
//! `[context.env-snapshot] timeout_secs` (1-60, default 15, a project file may only
//! lower it). API (extends `tests/strategy_selection.rs`): `StrategySelection` gains
//! `env_snapshot_timeout_secs: Chosen<u64>`; `"env-snapshot"` is a valid source name
//! that is NOT in the default `sources`; the `[context.env-snapshot]` table has the
//! single key `timeout_secs` (unknown keys are errors naming the file).

use std::path::Path;

use hone_config::ConfigError;
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, Origin, StrategySelection};

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn layer(path: &str, raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml(path, raw).unwrap_or_else(|e| panic!("fixture should parse: {e}\n{raw}"))
}
fn no_env(_: &str) -> Option<String> {
    None
}
fn select(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> StrategySelection {
    StrategySelection::from_sources(u, p, &no_env, &Flags::default()).unwrap_or_else(|e| panic!("{e}"))
}
fn select_err(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> ConfigError {
    StrategySelection::from_sources(u, p, &no_env, &Flags::default()).expect_err("should fail")
}
fn file(p: &str) -> Origin {
    Origin::File(p.into())
}
fn timeout_toml(n: u64) -> String {
    format!("[context.env-snapshot]\ntimeout_secs = {n}\n")
}

#[test]
fn test_e1_env_snapshot_should_be_off_by_default() {
    let s = select(None, None);
    assert!(!s.sources.value.contains(&"env-snapshot".to_owned()));
    assert_eq!(s.env_snapshot_timeout_secs.value, 15);
    assert_eq!(s.env_snapshot_timeout_secs.origin, Origin::Default);
}

#[test]
fn test_e1_the_user_file_can_turn_it_on() {
    let u = layer(USER, "[context]\nsources = [\"memory-index\", \"recall\", \"skills\", \"skeleton\", \"env-snapshot\"]\n");
    let s = select(Some(&u), None);
    assert!(s.sources.value.contains(&"env-snapshot".to_owned()));
    assert_eq!(s.sources.origin, file(USER));
}

#[test]
fn test_e1_a_project_file_cannot_turn_it_on() {
    let p = layer(PROJECT, "[context]\nsources = [\"recall\", \"env-snapshot\"]\n");
    let s = select(None, Some(&p));
    assert_eq!(s.sources.value, ["recall"], "intersected with the defaults, which lack env-snapshot");

    let u = layer(USER, "[context]\nsources = [\"recall\", \"env-snapshot\"]\n");
    let s = select(Some(&u), Some(&p));
    assert_eq!(s.sources.value, ["recall", "env-snapshot"].map(String::from), "the user turned it on, the project keeps it");
}

#[test]
fn test_e8_timeout_should_parse_with_its_file_as_origin_within_one_to_sixty() {
    for n in [1, 15, 60] {
        let u = layer(USER, &timeout_toml(n));
        let s = select(Some(&u), None);
        assert_eq!(s.env_snapshot_timeout_secs.value, n);
        assert_eq!(s.env_snapshot_timeout_secs.origin, file(USER));
    }
}

#[test]
fn test_e8_out_of_range_timeouts_should_be_config_errors_naming_the_file_and_key() {
    for n in [0, 61, 600] {
        for (path, is_user) in [(USER, true), (PROJECT, false)] {
            let l = layer(path, &timeout_toml(n));
            let e = if is_user { select_err(Some(&l), None) } else { select_err(None, Some(&l)) };
            match &e {
                ConfigError::InvalidField { path: p, reason } => {
                    assert_eq!(p, Path::new(path), "{n}");
                    assert!(reason.contains("timeout_secs"), "{reason}");
                    assert!(reason.contains("1") && reason.contains("60"), "the range is stated: {reason}");
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

#[test]
fn test_e8_wrong_types_and_unknown_keys_should_be_errors_naming_the_file() {
    for raw in [
        "[context.env-snapshot]\ntimeout_secs = \"fast\"\n",
        "[context.env-snapshot]\ntimeout_secs = 2.5\n",
        "[context.env-snapshot]\nbogus = 1\n",
    ] {
        let e = ConfigLayer::from_toml(USER, raw).expect_err(raw);
        match e {
            ConfigError::InvalidField { path, .. } => assert_eq!(path, Path::new(USER), "{raw}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn test_e8_a_project_file_may_only_lower_the_timeout() {
    let u = layer(USER, &timeout_toml(30));

    let s = select(Some(&u), Some(&layer(PROJECT, &timeout_toml(10))));
    assert_eq!((s.env_snapshot_timeout_secs.value, s.env_snapshot_timeout_secs.origin), (10, file(PROJECT)));

    let s = select(Some(&u), Some(&layer(PROJECT, &timeout_toml(45))));
    assert_eq!((s.env_snapshot_timeout_secs.value, s.env_snapshot_timeout_secs.origin), (30, file(USER)));
    let w: Vec<_> = s.warnings.iter().filter(|w| w.path == Path::new(PROJECT)).collect();
    assert_eq!(w.len(), 1);
    assert!(w[0].message.contains("timeout_secs"));
}

#[test]
fn test_e8_a_project_timeout_is_compared_with_the_default_when_the_user_sets_none() {
    let s = select(None, Some(&layer(PROJECT, &timeout_toml(5))));
    assert_eq!((s.env_snapshot_timeout_secs.value, s.env_snapshot_timeout_secs.origin), (5, file(PROJECT)));

    let s = select(None, Some(&layer(PROJECT, &timeout_toml(20))));
    assert_eq!(s.env_snapshot_timeout_secs.value, 15);
    assert_eq!(s.env_snapshot_timeout_secs.origin, Origin::Default);
    assert_eq!(s.warnings.len(), 1);
}

#[test]
fn test_e8_unknown_source_errors_should_list_env_snapshot_among_the_valid_names() {
    let u = layer(USER, "[context]\nsources = [\"bogus\"]\n");
    match select_err(Some(&u), None) {
        ConfigError::InvalidField { reason, .. } => assert!(reason.contains("env-snapshot"), "{reason}"),
        other => panic!("{other:?}"),
    }
}
