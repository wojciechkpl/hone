#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! S1-S4 (and the daemon default of S9) of `docs/plans/strategy-registry-spec.md`:
//! the `[context]` / `[memory]` tables as plain data, with precedence and trust.
//!
//! API required (new module `hone_config::strategy`; `Config` gains optional
//! `context` and `memory` tables with `deny_unknown_fields`, so
//! `ConfigLayer::from_toml` rejects unknown keys naming the file):
//!
//! ```ignore
//! #[derive(Debug, Clone, PartialEq, Eq)]
//! pub enum Origin { Flag, Env(String), File(PathBuf), Default }
//! #[derive(Debug, Clone, PartialEq)]
//! pub struct Chosen<T> { pub value: T, pub origin: Origin }
//!
//! #[derive(Debug, Clone, Default)]
//! pub struct Flags {
//!     pub skeleton_strategy: Option<String>,  // --skeleton-strategy
//!     pub no_inject: bool,                    // --no-inject
//!     pub daemon: bool,                       // honed: default sources omit "skeleton"
//! }
//!
//! #[derive(Debug, Clone)]
//! pub struct StrategySelection {
//!     pub sources: Chosen<Vec<String>>,       // default ["memory-index","recall","skills","skeleton"]
//!     pub skeleton: Chosen<String>,           // "adaptive" | "flat" | "dependency-graph" | "call-graph" | "hybrid"
//!     pub skeleton_max_tokens: Chosen<u32>,   // 2000
//!     pub skeleton_gap_threshold: Chosen<f32>,// 0.4
//!     pub compactor: Chosen<String>,          // "graduated" | "trim-only"
//!     pub graduated_phase: Chosen<bool>,      // false
//!     pub budget_ratio: Chosen<Option<f32>>,  // None = the per-model table
//!     pub recaller: Chosen<String>,           // "bm25" | "none"
//!     pub top_k: Chosen<usize>,               // 5
//!     pub exclude_kinds: Chosen<Vec<String>>, // ["outcome"]
//!     pub writer: Chosen<String>,             // "triggers" | "off"
//!     pub warnings: Vec<hone_config::resolve::Warning>,
//! }
//! impl StrategySelection {
//!     /// Precedence: flag, env var, project file, user file, default (S2). The project
//!     /// layer is untrusted and may only narrow (S3). Errors are
//!     /// `ConfigError::InvalidField { path, reason }`: for a file, `path` is that file;
//!     /// for an env var or flag, `path` is the variable name / flag (`HONE_SKELETON_STRATEGY`,
//!     /// `--skeleton-strategy`). The reason names the key and lists the valid names (S4).
//!     pub fn from_sources(user: Option<&ConfigLayer>, project: Option<&ConfigLayer>,
//!         env: &dyn Fn(&str) -> Option<String>, flags: &Flags)
//!         -> Result<StrategySelection, ConfigError>;
//! }
//! ```

use std::path::Path;

use hone_config::ConfigError;
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Chosen, Flags, Origin, StrategySelection};

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn layer(path: &str, raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml(path, raw).unwrap_or_else(|e| panic!("fixture should parse: {e}\n{raw}"))
}
fn user(raw: &str) -> ConfigLayer {
    layer(USER, raw)
}
fn project(raw: &str) -> ConfigLayer {
    layer(PROJECT, raw)
}
fn no_env(_: &str) -> Option<String> {
    None
}
fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| (*v).to_owned())
    }
}

fn select(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> StrategySelection {
    StrategySelection::from_sources(u, p, &no_env, &Flags::default())
        .unwrap_or_else(|e| panic!("selection should succeed: {e}"))
}

fn select_err(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> ConfigError {
    StrategySelection::from_sources(u, p, &no_env, &Flags::default())
        .expect_err("selection should fail")
}

fn parts(e: &ConfigError) -> (&Path, &str) {
    match e {
        ConfigError::InvalidField { path, reason } => (path, reason),
        other => panic!("expected InvalidField, got {other:?}"),
    }
}

fn file(path: &str) -> Origin {
    Origin::File(path.into())
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn assert_default<T: PartialEq + std::fmt::Debug>(c: &Chosen<T>, value: T) {
    assert_eq!(c.value, value);
    assert_eq!(c.origin, Origin::Default);
}

fn warns_for(sel: &StrategySelection, path: &str) -> Vec<String> {
    sel.warnings
        .iter()
        .filter(|w| w.path == Path::new(path))
        .map(|w| w.message.clone())
        .collect()
}

// ---- S1: the tables parse as plain data ----------------------------------------------

#[test]
fn test_s1_no_layers_should_resolve_to_the_defaults() {
    let s = select(None, None);
    assert_default(
        &s.sources,
        strings(&["memory-index", "recall", "skills", "skeleton"]),
    );
    assert_default(&s.skeleton, "adaptive".to_owned());
    assert_default(&s.skeleton_max_tokens, 2000);
    assert_default(&s.skeleton_gap_threshold, 0.4);
    assert_default(&s.compactor, "graduated".to_owned());
    assert_default(&s.graduated_phase, false);
    assert_default(&s.budget_ratio, None);
    assert_default(&s.recaller, "bm25".to_owned());
    assert_default(&s.top_k, 5);
    assert_default(&s.exclude_kinds, strings(&["outcome"]));
    assert_default(&s.writer, "triggers".to_owned());
    assert!(s.warnings.is_empty());
}

#[test]
fn test_s1_a_file_without_the_tables_should_resolve_to_the_defaults() {
    let u = user("default_model = \"anthropic:claude-sonnet-4-5\"\n");
    let s = select(Some(&u), None);
    assert_default(&s.skeleton, "adaptive".to_owned());
    assert_default(&s.top_k, 5);
}

#[test]
fn test_s1_every_key_should_parse_with_its_file_as_origin() {
    let u = user(
        r#"
[context]
sources = ["recall", "skills"]
compactor = "trim-only"
budget_ratio = 0.6

[context.skeleton]
use = "hybrid"
max_tokens = 1500
gap_threshold = 0.3

[context.graduated]
phase = true

[memory]
recaller = "none"
writer = "off"
top_k = 3
exclude_kinds = []
"#,
    );

    let s = select(Some(&u), None);

    assert_eq!(
        (s.sources.value.clone(), s.sources.origin.clone()),
        (strings(&["recall", "skills"]), file(USER))
    );
    assert_eq!(
        (s.compactor.value.as_str(), s.compactor.origin.clone()),
        ("trim-only", file(USER))
    );
    assert_eq!(
        (s.budget_ratio.value, s.budget_ratio.origin.clone()),
        (Some(0.6), file(USER))
    );
    assert_eq!(
        (s.skeleton.value.as_str(), s.skeleton.origin.clone()),
        ("hybrid", file(USER))
    );
    assert_eq!(
        (
            s.skeleton_max_tokens.value,
            s.skeleton_max_tokens.origin.clone()
        ),
        (1500, file(USER))
    );
    assert_eq!(
        (
            s.skeleton_gap_threshold.value,
            s.skeleton_gap_threshold.origin.clone()
        ),
        (0.3, file(USER))
    );
    assert_eq!(
        (s.graduated_phase.value, s.graduated_phase.origin.clone()),
        (true, file(USER))
    );
    assert_eq!(
        (s.recaller.value.as_str(), s.recaller.origin.clone()),
        ("none", file(USER))
    );
    assert_eq!(
        (s.writer.value.as_str(), s.writer.origin.clone()),
        ("off", file(USER))
    );
    assert_eq!((s.top_k.value, s.top_k.origin.clone()), (3, file(USER)));
    assert_eq!(
        (
            s.exclude_kinds.value.clone(),
            s.exclude_kinds.origin.clone()
        ),
        (vec![], file(USER))
    );
}

#[test]
fn test_s1_unknown_keys_should_be_errors_naming_the_file_and_the_key() {
    for raw in [
        "[context]\nbogus = 1\n",
        "[context.skeleton]\nbogus = 1\n",
        "[context.graduated]\nbogus = 1\n",
        "[memory]\nbogus = 1\n",
        "[context.nonsense]\nx = 1\n",
    ] {
        let e = ConfigLayer::from_toml(USER, raw).expect_err(raw);
        let (path, reason) = parts(&e);
        assert_eq!(path, Path::new(USER), "{raw}");
        assert!(
            reason.contains("bogus") || reason.contains("nonsense"),
            "{raw}: {reason}"
        );
    }
}

#[test]
fn test_s1_wrong_types_should_be_errors_naming_the_file() {
    for raw in [
        "[context]\nbudget_ratio = \"high\"\n",
        "[memory]\ntop_k = \"five\"\n",
        "[context.graduated]\nphase = \"yes\"\n",
        "[context]\nsources = \"recall\"\n",
    ] {
        let e = ConfigLayer::from_toml(PROJECT, raw).expect_err(raw);
        assert_eq!(parts(&e).0, Path::new(PROJECT), "{raw}");
    }
}

// ---- S2: precedence -----------------------------------------------------------------------

#[test]
fn test_s2_project_file_should_win_over_the_user_file_for_the_skeleton_choice() {
    let u = user("[context.skeleton]\nuse = \"flat\"\n");
    let p = project("[context.skeleton]\nuse = \"hybrid\"\n");
    let s = select(Some(&u), Some(&p));
    assert_eq!(
        (s.skeleton.value.as_str(), s.skeleton.origin),
        ("hybrid", file(PROJECT))
    );

    let s = select(Some(&u), None);
    assert_eq!(
        (s.skeleton.value.as_str(), s.skeleton.origin),
        ("flat", file(USER))
    );
}

#[test]
fn test_s2_env_var_should_win_over_files_and_a_flag_over_the_env_var() {
    let u = user("[context.skeleton]\nuse = \"flat\"\n");
    let p = project("[context.skeleton]\nuse = \"hybrid\"\n");
    let env = env_of(&[("HONE_SKELETON_STRATEGY", "call-graph")]);

    let s = StrategySelection::from_sources(Some(&u), Some(&p), &env, &Flags::default()).unwrap();
    assert_eq!(s.skeleton.value, "call-graph");
    assert_eq!(
        s.skeleton.origin,
        Origin::Env("HONE_SKELETON_STRATEGY".to_owned())
    );

    let flags = Flags {
        skeleton_strategy: Some("dependency-graph".to_owned()),
        ..Default::default()
    };
    let s = StrategySelection::from_sources(Some(&u), Some(&p), &env, &flags).unwrap();
    assert_eq!(s.skeleton.value, "dependency-graph");
    assert_eq!(s.skeleton.origin, Origin::Flag);
}

#[test]
fn test_s2_the_skeleton_env_var_should_accept_todays_spellings() {
    // `SkeletonStrategy::from_str` accepts underscores and aliases; the env var keeps doing so.
    for (raw, expected) in [
        ("call_graph", "call-graph"),
        ("deps", "dependency-graph"),
        ("FLAT", "flat"),
    ] {
        let pairs = [("HONE_SKELETON_STRATEGY", raw)];
        let env = env_of(&pairs);
        let s = StrategySelection::from_sources(None, None, &env, &Flags::default()).unwrap();
        assert_eq!(s.skeleton.value, expected, "{raw}");
    }
}

#[test]
fn test_s2_no_inject_flag_should_remove_skeleton_from_sources() {
    let flags = Flags {
        no_inject: true,
        ..Default::default()
    };
    let s = StrategySelection::from_sources(None, None, &no_env, &flags).unwrap();
    assert_eq!(
        s.sources.value,
        strings(&["memory-index", "recall", "skills"])
    );
    assert_eq!(s.sources.origin, Origin::Flag);
}

#[test]
fn test_s2_no_injection_env_var_should_remove_skeleton_only_when_it_is_one() {
    let on = env_of(&[("HONE_NO_INJECTION", "1")]);
    let s = StrategySelection::from_sources(None, None, &on, &Flags::default()).unwrap();
    assert!(!s.sources.value.contains(&"skeleton".to_owned()));
    assert_eq!(
        s.sources.origin,
        Origin::Env("HONE_NO_INJECTION".to_owned())
    );

    for v in ["0", "", "true"] {
        let pairs = [("HONE_NO_INJECTION", v)];
        let off = env_of(&pairs);
        let s = StrategySelection::from_sources(None, None, &off, &Flags::default()).unwrap();
        assert!(
            s.sources.value.contains(&"skeleton".to_owned()),
            "HONE_NO_INJECTION={v:?}"
        );
    }
}

#[test]
fn test_s2_no_inject_should_also_strip_skeleton_from_a_files_sources() {
    let u = user("[context]\nsources = [\"recall\", \"skeleton\"]\n");
    let flags = Flags {
        no_inject: true,
        ..Default::default()
    };
    let s = StrategySelection::from_sources(Some(&u), None, &no_env, &flags).unwrap();
    assert_eq!(s.sources.value, strings(&["recall"]));
}

#[test]
fn test_s2_phase_compaction_env_var_should_set_graduated_phase() {
    for v in ["1", "true", "on"] {
        let pairs = [("HONE_PHASE_COMPACTION", v)];
        let env = env_of(&pairs);
        let s = StrategySelection::from_sources(None, None, &env, &Flags::default()).unwrap();
        assert!(s.graduated_phase.value, "{v}");
        assert_eq!(
            s.graduated_phase.origin,
            Origin::Env("HONE_PHASE_COMPACTION".to_owned())
        );
    }
    for v in ["0", "false", "off", ""] {
        let pairs = [("HONE_PHASE_COMPACTION", v)];
        let env = env_of(&pairs);
        let s = StrategySelection::from_sources(None, None, &env, &Flags::default()).unwrap();
        assert!(!s.graduated_phase.value, "{v}");
    }
}

#[test]
fn test_s2_the_phase_env_var_should_win_over_the_user_file() {
    let u = user("[context.graduated]\nphase = false\n");
    let env = env_of(&[("HONE_PHASE_COMPACTION", "1")]);
    let s = StrategySelection::from_sources(Some(&u), None, &env, &Flags::default()).unwrap();
    assert!(s.graduated_phase.value);
    assert_eq!(
        s.graduated_phase.origin,
        Origin::Env("HONE_PHASE_COMPACTION".to_owned())
    );
}

// ---- S3: a project file can only narrow ------------------------------------------------------------

#[test]
fn test_s3_project_sources_should_be_intersected_with_the_users() {
    let u = user("[context]\nsources = [\"memory-index\", \"recall\", \"skeleton\"]\n");
    let p = project("[context]\nsources = [\"recall\", \"skills\", \"skeleton\"]\n");
    let s = select(Some(&u), Some(&p));
    assert_eq!(
        sorted(s.sources.value.clone()),
        strings(&["recall", "skeleton"]),
        "skills is not in the user's"
    );
    assert_eq!(s.sources.origin, file(PROJECT));
}

#[test]
fn test_s3_project_sources_should_narrow_the_default_set_too() {
    let p = project("[context]\nsources = [\"recall\"]\n");
    let s = select(None, Some(&p));
    assert_eq!(s.sources.value, strings(&["recall"]));
    assert_eq!(s.sources.origin, file(PROJECT));
}

#[test]
fn test_s3_project_numbers_should_only_take_effect_when_lower() {
    let u = user(
        "[memory]\ntop_k = 5\n[context.skeleton]\nmax_tokens = 2000\n[context]\nbudget_ratio = 0.6\n",
    );
    let lower = project(
        "[memory]\ntop_k = 3\n[context.skeleton]\nmax_tokens = 1000\n[context]\nbudget_ratio = 0.4\n",
    );
    let s = select(Some(&u), Some(&lower));
    assert_eq!((s.top_k.value, s.top_k.origin.clone()), (3, file(PROJECT)));
    assert_eq!(
        (s.skeleton_max_tokens.value, s.skeleton_max_tokens.origin.clone()),
        (1000, file(PROJECT))
    );
    assert_eq!(
        (s.budget_ratio.value, s.budget_ratio.origin.clone()),
        (Some(0.4), file(PROJECT))
    );
    assert!(
        warns_for(&s, PROJECT).is_empty(),
        "narrowing is not a warning"
    );

    let higher = project(
        "[memory]\ntop_k = 9\n[context.skeleton]\nmax_tokens = 9000\n[context]\nbudget_ratio = 0.9\n",
    );
    let s = select(Some(&u), Some(&higher));
    assert_eq!((s.top_k.value, s.top_k.origin.clone()), (5, file(USER)));
    assert_eq!(
        (s.skeleton_max_tokens.value, s.skeleton_max_tokens.origin.clone()),
        (2000, file(USER))
    );
    assert_eq!(
        (s.budget_ratio.value, s.budget_ratio.origin.clone()),
        (Some(0.6), file(USER))
    );
    let w = warns_for(&s, PROJECT);
    assert_eq!(
        w.len(),
        3,
        "one warning per ignored value, naming the project file: {w:?}"
    );
    assert!(w.iter().any(|m| m.contains("top_k")));
    assert!(w.iter().any(|m| m.contains("max_tokens")));
    assert!(w.iter().any(|m| m.contains("budget_ratio")));
}

#[test]
fn test_s3_project_top_k_should_be_compared_with_the_default_when_the_user_sets_none() {
    let s = select(None, Some(&project("[memory]\ntop_k = 9\n")));
    assert_eq!(s.top_k.value, 5);
    assert_eq!(s.top_k.origin, Origin::Default);
    assert_eq!(warns_for(&s, PROJECT).len(), 1);

    let s = select(None, Some(&project("[memory]\ntop_k = 2\n")));
    assert_eq!((s.top_k.value, s.top_k.origin.clone()), (2, file(PROJECT)));
}

#[test]
fn test_s3_project_writer_and_recaller_can_only_be_turned_off() {
    let s = select(
        None,
        Some(&project(
            "[memory]\nwriter = \"off\"\nrecaller = \"none\"\n",
        )),
    );
    assert_eq!(
        (s.writer.value.as_str(), s.writer.origin.clone()),
        ("off", file(PROJECT))
    );
    assert_eq!(
        (s.recaller.value.as_str(), s.recaller.origin.clone()),
        ("none", file(PROJECT))
    );

    let u = user("[memory]\nwriter = \"off\"\nrecaller = \"none\"\n");
    let p = project("[memory]\nwriter = \"triggers\"\nrecaller = \"bm25\"\n");
    let s = select(Some(&u), Some(&p));
    assert_eq!(
        (s.writer.value.as_str(), s.writer.origin.clone()),
        ("off", file(USER))
    );
    assert_eq!(
        (s.recaller.value.as_str(), s.recaller.origin.clone()),
        ("none", file(USER))
    );
    assert_eq!(
        warns_for(&s, PROJECT).len(),
        2,
        "both turn-ons are ignored with a warning"
    );
}

#[test]
fn test_s3_project_can_only_set_phase_compaction_to_false() {
    let u = user("[context.graduated]\nphase = true\n");
    let s = select(
        Some(&u),
        Some(&project("[context.graduated]\nphase = false\n")),
    );
    assert_eq!(
        (s.graduated_phase.value, s.graduated_phase.origin),
        (false, file(PROJECT))
    );

    let s = select(None, Some(&project("[context.graduated]\nphase = true\n")));
    assert!(
        !s.graduated_phase.value,
        "a repository cannot turn on a lossy compaction"
    );
    assert_eq!(warns_for(&s, PROJECT).len(), 1);

    let s = select(
        Some(&u),
        Some(&project("[context.graduated]\nphase = true\n")),
    );
    assert!(s.graduated_phase.value, "the user's own choice stands");
}

#[test]
fn test_s3_project_may_pick_any_skeleton_and_compactor() {
    let u = user("[context]\ncompactor = \"graduated\"\n");
    let p =
        project("[context]\ncompactor = \"trim-only\"\n[context.skeleton]\nuse = \"call-graph\"\n");
    let s = select(Some(&u), Some(&p));
    assert_eq!(
        (s.compactor.value.as_str(), s.compactor.origin),
        ("trim-only", file(PROJECT))
    );
    assert_eq!(
        (s.skeleton.value.as_str(), s.skeleton.origin),
        ("call-graph", file(PROJECT))
    );
}

// ---- S4: unknown names fail ---------------------------------------------------------------------------------

#[test]
fn test_s4_unknown_names_should_fail_naming_the_file_the_key_and_the_valid_names() {
    let table: [(&str, &str, &str, &str); 5] = [
        (
            "[context.skeleton]\nuse = \"bogus\"\n",
            "context.skeleton.use",
            "bogus",
            "adaptive",
        ),
        (
            "[context]\nsources = [\"recall\", \"bogus\"]\n",
            "context.sources",
            "bogus",
            "memory-index",
        ),
        (
            "[context]\ncompactor = \"llm-summary\"\n",
            "context.compactor",
            "llm-summary",
            "graduated",
        ),
        (
            "[memory]\nrecaller = \"fts5\"\n",
            "memory.recaller",
            "fts5",
            "bm25",
        ),
        (
            "[memory]\nwriter = \"llm-extract\"\n",
            "memory.writer",
            "llm-extract",
            "triggers",
        ),
    ];
    for (raw, key, bad, valid) in table {
        for (path, make) in [(USER, user as fn(&str) -> ConfigLayer), (PROJECT, project)] {
            let l = make(raw);
            let e = if path == USER {
                select_err(Some(&l), None)
            } else {
                select_err(None, Some(&l))
            };
            let (p, reason) = parts(&e);
            assert_eq!(p, Path::new(path), "{raw}");
            assert!(reason.contains(key), "{key} not named: {reason}");
            assert!(reason.contains(bad), "{bad} not named: {reason}");
            assert!(
                reason.contains(valid),
                "valid names not listed ({valid}): {reason}"
            );
        }
    }
}

#[test]
fn test_s4_unknown_env_and_flag_values_should_fail_too() {
    let env = env_of(&[("HONE_SKELETON_STRATEGY", "bogus")]);
    let e = StrategySelection::from_sources(None, None, &env, &Flags::default()).unwrap_err();
    let (p, reason) = parts(&e);
    assert_eq!(p, Path::new("HONE_SKELETON_STRATEGY"));
    assert!(
        reason.contains("bogus") && reason.contains("adaptive"),
        "{reason}"
    );

    let flags = Flags {
        skeleton_strategy: Some("bogus".to_owned()),
        ..Default::default()
    };
    let e = StrategySelection::from_sources(None, None, &no_env, &flags).unwrap_err();
    let (p, reason) = parts(&e);
    assert_eq!(p, Path::new("--skeleton-strategy"));
    assert!(
        reason.contains("bogus") && reason.contains("adaptive"),
        "{reason}"
    );
}

// ---- S9: honed's default ---------------------------------------------------------------------------------------

#[test]
fn test_s9_daemon_defaults_should_omit_the_skeleton_but_a_config_can_turn_it_on() {
    let flags = Flags {
        daemon: true,
        ..Default::default()
    };
    let s = StrategySelection::from_sources(None, None, &no_env, &flags).unwrap();
    assert_eq!(
        s.sources.value,
        strings(&["memory-index", "recall", "skills"])
    );
    assert_eq!(s.sources.origin, Origin::Default);

    let u = user("[context]\nsources = [\"memory-index\", \"recall\", \"skills\", \"skeleton\"]\n");
    let s = StrategySelection::from_sources(Some(&u), None, &no_env, &flags).unwrap();
    assert!(s.sources.value.contains(&"skeleton".to_owned()));
    assert_eq!(s.sources.origin, file(USER));
}
