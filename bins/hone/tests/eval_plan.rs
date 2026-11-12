#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F3, F4 and F5 of `docs/plans/eval-profiles-spec.md`: the tasks file with `[eval]`
//! and `[profiles.*]`, the expansion into runs, and `--ablate`, as pure functions.
//!
//! API required (new module `hone_eval::tasks`):
//!
//! ```ignore
//! impl TaskSpec { pub memory_fixture: Option<String> }          // new optional field
//! pub struct TasksFile { pub tasks: Vec<TaskSpec>, pub model: Option<String>, .. }
//! impl TasksFile {
//!     /// `file` is the label used in errors (the path). Validates every profile with
//!     /// the `[context]` / `[memory]` schema of config.toml (hone-config: unknown keys
//!     /// and unknown strategy names are errors). The error's Display names `file`,
//!     /// the profile and the offending key or name.
//!     pub fn from_toml_str(file: &str, raw: &str) -> Result<TasksFile, EvalConfigError>;
//!     pub fn base_profile(&self) -> String;       // `[eval] base_profile`, default "default"
//!     pub fn repeats(&self) -> u32;               // `[eval] repeats`, default 1
//!     /// Base profile first, then the others sorted by name. The built-in `default`
//!     /// profile (no overrides) always exists.
//!     pub fn profile_names(&self) -> Vec<String>;
//!     /// The profile applied over `user` like a project file would be, except that it
//!     /// may also widen (e.g. turn `[context.graduated] phase` on).
//!     pub fn profile_selection(&self, name: &str, user: Option<&ConfigLayer>)
//!         -> Result<StrategySelection, EvalConfigError>;
//!     /// Runs in order: for each task (file order), for each repeat (1-based), for each
//!     /// selected profile (in `profile_names` order). `profiles` empty = all.
//!     pub fn expand(&self, opts: &ExpandOptions) -> Result<Vec<PlannedRun>, EvalConfigError>;
//!     /// `--ablate SLOT [base]`: a TasksFile whose profiles are the base plus one
//!     /// generated profile per built-in value of SLOT other than the base's, named
//!     /// `<slot>=<value>` (`context.sources:<name>=off` for a source toggle).
//!     pub fn with_ablation(&self, slot: &str, base: Option<&str>)
//!         -> Result<TasksFile, EvalConfigError>;
//! }
//! #[derive(Default)] pub struct ExpandOptions { pub profiles: Vec<String>, pub repeats: Option<u32> }
//! #[derive(Debug, Clone, PartialEq, Eq)]
//! pub struct PlannedRun { pub profile: String, pub task: String, pub repeat: u32 }
//! ```
//! `StrategySelection` is `hone_config::strategy::StrategySelection` (PR D).

use hone_config::strategy::StrategySelection;
use hone_eval::tasks::{ExpandOptions, PlannedRun, TasksFile};

const FILE: &str = "/evals/tasks.toml";

const BASIC: &str = r#"
model = "ollama:qwen"

[[tasks]]
id = "t1"
prompt = "one"
success_check = "true"
memory_fixture = "fixtures/t1.jsonl"

[[tasks]]
id = "t2"
prompt = "two"
success_check = "true"

[profiles.lean.context]
compactor = "trim-only"

[profiles.phase.context.graduated]
phase = true

[eval]
repeats = 2
"#;

fn parse(raw: &str) -> TasksFile {
    TasksFile::from_toml_str(FILE, raw).unwrap_or_else(|e| panic!("should parse: {e}\n{raw}"))
}

fn err(raw: &str) -> String {
    TasksFile::from_toml_str(FILE, raw)
        .err()
        .unwrap_or_else(|| panic!("should fail:\n{raw}"))
        .to_string()
}

fn run(profile: &str, task: &str, repeat: u32) -> PlannedRun {
    PlannedRun {
        profile: profile.to_owned(),
        task: task.to_owned(),
        repeat,
    }
}

/// Every chosen value of a selection, as one comparable string (origins left out).
fn values(s: &StrategySelection) -> Vec<String> {
    vec![
        format!("sources={:?}", s.sources.value),
        format!("skeleton={:?}", s.skeleton.value),
        format!("max_tokens={:?}", s.skeleton_max_tokens.value),
        format!("gap={:?}", s.skeleton_gap_threshold.value),
        format!("compactor={:?}", s.compactor.value),
        format!("phase={:?}", s.graduated_phase.value),
        format!("budget={:?}", s.budget_ratio.value),
        format!("recaller={:?}", s.recaller.value),
        format!("top_k={:?}", s.top_k.value),
        format!("exclude={:?}", s.exclude_kinds.value),
        format!("writer={:?}", s.writer.value),
    ]
}

fn differing(a: &StrategySelection, b: &StrategySelection) -> Vec<String> {
    values(a)
        .into_iter()
        .zip(values(b))
        .filter(|(x, y)| x != y)
        .map(|(x, _)| x.split('=').next().unwrap().to_owned())
        .collect()
}

// ---- F3: profiles ------------------------------------------------------------------------

#[test]
fn test_f3_tasks_file_should_parse_tasks_model_fixture_and_eval_defaults() {
    let tf = parse(BASIC);

    assert_eq!(tf.tasks.len(), 2);
    assert_eq!(tf.tasks[0].memory_fixture.as_deref(), Some("fixtures/t1.jsonl"));
    assert_eq!(tf.tasks[1].memory_fixture, None);
    assert_eq!(tf.model.as_deref(), Some("ollama:qwen"));
    assert_eq!(tf.repeats(), 2);
    assert_eq!(tf.base_profile(), "default");
    let plain = parse("[[tasks]]\nid = \"a\"\nprompt = \"p\"\nsuccess_check = \"true\"\n");
    assert_eq!(plain.repeats(), 1);
    assert_eq!(plain.profile_names(), ["default"]);
}

#[test]
fn test_f3_profile_names_should_list_the_base_first_then_the_rest_sorted() {
    let tf = parse(BASIC);
    assert_eq!(tf.profile_names(), ["default", "lean", "phase"]);

    let named = parse(&format!("{BASIC}\n[eval]\nbase_profile = \"lean\"\n").replace("[eval]\nrepeats = 2\n", ""));
    assert_eq!(named.base_profile(), "lean");
    assert_eq!(named.profile_names(), ["lean", "default", "phase"]);
}

#[test]
fn test_f3_profile_selection_should_apply_the_profile_over_the_defaults() {
    let tf = parse(BASIC);

    let base = tf.profile_selection("default", None).unwrap();
    let lean = tf.profile_selection("lean", None).unwrap();

    assert_eq!(base.compactor.value, "graduated");
    assert_eq!(lean.compactor.value, "trim-only");
    assert_eq!(differing(&base, &lean), ["compactor"]);
}

#[test]
fn test_f3_a_profile_may_widen_what_a_project_file_may_not() {
    // A project config can only turn phase compaction off (S3); the evaluator's own file may turn it on.
    let tf = parse(BASIC);
    let phase = tf.profile_selection("phase", None).unwrap();
    assert!(phase.graduated_phase.value);
}

#[test]
fn test_f3_an_unknown_key_in_a_profile_should_name_the_file_the_profile_and_the_key() {
    let msg = err("[[tasks]]\nid=\"a\"\nprompt=\"p\"\nsuccess_check=\"true\"\n[profiles.lean.context]\ncolour = \"red\"\n");
    for needle in [FILE, "lean", "colour"] {
        assert!(msg.contains(needle), "{needle:?} missing from {msg}");
    }
}

#[test]
fn test_f3_an_unknown_strategy_name_in_a_profile_should_name_the_file_profile_and_name() {
    let msg = err("[[tasks]]\nid=\"a\"\nprompt=\"p\"\nsuccess_check=\"true\"\n[profiles.odd.context.skeleton]\nuse = \"nope\"\n");
    for needle in [FILE, "odd", "nope"] {
        assert!(msg.contains(needle), "{needle:?} missing from {msg}");
    }
}

#[test]
fn test_f3_a_base_profile_that_is_not_defined_should_be_an_error() {
    let msg = err("[[tasks]]\nid=\"a\"\nprompt=\"p\"\nsuccess_check=\"true\"\n[eval]\nbase_profile = \"ghost\"\n");
    assert!(msg.contains("ghost"), "{msg}");
}

#[test]
fn test_f3_selecting_an_unknown_profile_should_be_an_error_naming_it() {
    let tf = parse(BASIC);
    let e = tf
        .expand(&ExpandOptions {
            profiles: vec!["ghost".to_owned()],
            repeats: None,
        })
        .unwrap_err()
        .to_string();
    assert!(e.contains("ghost"), "{e}");
}

// ---- F4: repeats and order ---------------------------------------------------------------

#[test]
fn test_f4_runs_should_be_task_major_with_profiles_interleaved_and_repeats_inside_a_task() {
    let tf = parse(BASIC); // repeats = 2 from [eval]

    let runs = tf.expand(&ExpandOptions::default()).unwrap();

    let mut want = Vec::new();
    for task in ["t1", "t2"] {
        for repeat in 1..=2 {
            for profile in ["default", "lean", "phase"] {
                want.push(run(profile, task, repeat));
            }
        }
    }
    assert_eq!(runs, want);
    assert_eq!(runs.len(), 12);
}

#[test]
fn test_f4_the_repeats_option_should_override_the_file_and_default_to_one() {
    let tf = parse(BASIC);
    let three = tf
        .expand(&ExpandOptions { profiles: vec![], repeats: Some(3) })
        .unwrap();
    assert_eq!(three.len(), 2 * 3 * 3);
    let plain = parse("[[tasks]]\nid = \"a\"\nprompt = \"p\"\nsuccess_check = \"true\"\n");
    assert_eq!(plain.expand(&ExpandOptions::default()).unwrap(), [run("default", "a", 1)]);
}

#[test]
fn test_f4_selected_profiles_should_run_in_canonical_order_and_alone() {
    let tf = parse(BASIC);
    let runs = tf
        .expand(&ExpandOptions {
            profiles: vec!["phase".to_owned(), "lean".to_owned()],
            repeats: Some(1),
        })
        .unwrap();
    assert_eq!(
        runs,
        [run("lean", "t1", 1), run("phase", "t1", 1), run("lean", "t2", 1), run("phase", "t2", 1)]
    );
}

// ---- F5: --ablate -----------------------------------------------------------------------------

fn ablated(slot: &str) -> (TasksFile, Vec<String>) {
    let ab = parse(BASIC).with_ablation(slot, None).unwrap_or_else(|e| panic!("{slot}: {e}"));
    let names = ab.profile_names();
    (ab, names)
}

#[test]
fn test_f5_ablating_the_skeleton_should_add_one_profile_per_other_builtin_value() {
    let (ab, names) = ablated("context.skeleton");

    assert_eq!(
        names,
        [
            "default",
            "context.skeleton=call-graph",
            "context.skeleton=dependency-graph",
            "context.skeleton=flat",
            "context.skeleton=hybrid",
        ]
    );
    let base = ab.profile_selection("default", None).unwrap();
    for name in &names[1..] {
        let sel = ab.profile_selection(name, None).unwrap();
        assert_eq!(differing(&base, &sel), ["skeleton"], "{name}: only the ablated slot varies");
        assert_eq!(format!("context.skeleton={}", sel.skeleton.value), *name);
    }
}

#[test]
fn test_f5_each_slot_should_vary_only_itself() {
    let cases: [(&str, &str, &str); 4] = [
        ("context.compactor", "context.compactor=trim-only", "compactor"),
        ("context.graduated.phase", "context.graduated.phase=true", "phase"),
        ("memory.recaller", "memory.recaller=none", "recaller"),
        ("context.sources:recall", "context.sources:recall=off", "sources"),
    ];
    for (slot, profile, field) in cases {
        let (ab, names) = ablated(slot);
        assert_eq!(names, ["default", profile], "{slot}");
        let base = ab.profile_selection("default", None).unwrap();
        let sel = ab.profile_selection(profile, None).unwrap();
        assert_eq!(differing(&base, &sel), [field], "{slot}");
    }
    let (ab, _) = ablated("context.sources:recall");
    let sel = ab.profile_selection("context.sources:recall=off", None).unwrap();
    assert!(!sel.sources.value.contains(&"recall".to_owned()));
    assert!(sel.sources.value.contains(&"skills".to_owned()), "other sources stay on");
}

#[test]
fn test_f5_a_chosen_base_profile_should_be_the_starting_point() {
    // `lean` already uses trim-only; ablating the skeleton must keep that.
    let ab = parse(BASIC).with_ablation("context.skeleton", Some("lean")).unwrap();

    let names = ab.profile_names();
    assert_eq!(names[0], "lean");
    let base = ab.profile_selection("lean", None).unwrap();
    for name in &names[1..] {
        let sel = ab.profile_selection(name, None).unwrap();
        assert_eq!(sel.compactor.value, "trim-only", "{name}");
        assert_eq!(differing(&base, &sel), ["skeleton"], "{name}");
    }
}

#[test]
fn test_f5_an_unknown_slot_or_base_should_be_an_error() {
    let tf = parse(BASIC);
    let e = tf.with_ablation("context.colour", None).err().expect("unknown slot").to_string();
    assert!(e.contains("context.colour"), "{e}");
    let e = tf.with_ablation("context.skeleton", Some("ghost")).err().expect("unknown base").to_string();
    assert!(e.contains("ghost"), "{e}");
}

#[test]
fn test_f5_ablation_runs_should_expand_like_any_profiles() {
    let (ab, names) = ablated("memory.recaller");
    let runs = ab.expand(&ExpandOptions { profiles: vec![], repeats: Some(1) }).unwrap();
    assert_eq!(runs.len(), 2 * names.len());
    assert_eq!(runs[0], run("default", "t1", 1));
    assert_eq!(runs[1], run("memory.recaller=none", "t1", 1));
}
