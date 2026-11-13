#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F16 and the F21 size warning of `docs/plans/eval-profiles-spec.md` (review round 1).
//! These need two new methods and so fail to compile until they exist; they live apart from
//! `eval_review1.rs` so the rest of that file stays individually red.
//!
//! API required (new):
//! - `EvalRunner::with_copy_warn_bytes(self, bytes: u64) -> Self`: the size above which a
//!   task's copy is reported (default 200 MB). The report is an entry in the manifest's
//!   `warnings` array (strings naming the task), once per task however many runs it has.
//! - `TasksFile::with_ablation_over(&self, slot, base: Option<&str>, user: Option<&ConfigLayer>)`:
//!   like `with_ablation`, but the base is the user's config plus the base profile, and a
//!   generated value equal to the base's is skipped. Source toggles are named
//!   `context.sources:<name>=off` (source on in the base) or `=on` (off in the base).

mod eval_support;

use std::sync::Arc;

use eval_support::*;
use hone_config::resolve::ConfigLayer;
use hone_eval::tasks::TasksFile;

// ---- F16: ablation over the effective base ---------------------------------------------------------

const BASIC: &str = "[[tasks]]\nid = \"t\"\nprompt = \"p\"\nsuccess_check = \"true\"\n\n[profiles.lean.context]\ncompactor = \"trim-only\"\n";

fn user_layer(raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml("/home/u/.config/hone/config.toml", raw).unwrap()
}

#[test]
fn test_f16_a_source_toggle_should_change_exactly_one_source_of_the_users_effective_base() {
    let tf = TasksFile::from_toml_str("tasks.toml", BASIC).unwrap();
    let user = user_layer("[context]\nsources = [\"memory-index\", \"recall\", \"skills\"]\n");

    let ab = tf.with_ablation_over("context.sources:recall", None, Some(&user)).unwrap();

    let base = ab.profile_selection("default", Some(&user)).unwrap();
    assert_eq!(base.sources.value, ["memory-index", "recall", "skills"]);
    let names = ab.profile_names();
    assert_eq!(names, ["default", "context.sources:recall=off"]);
    let off = ab.profile_selection("context.sources:recall=off", Some(&user)).unwrap();
    assert_eq!(off.sources.value, ["memory-index", "skills"], "only recall left");
    assert_eq!(off.skeleton.value, base.skeleton.value);
    assert_eq!(off.compactor.value, base.compactor.value);
    assert_eq!(off.recaller.value, base.recaller.value);
}

#[test]
fn test_f16_a_source_the_base_lacks_should_be_toggled_on_not_off() {
    let tf = TasksFile::from_toml_str("tasks.toml", BASIC).unwrap();
    let user = user_layer("[context]\nsources = [\"memory-index\", \"recall\", \"skills\"]\n");

    let ab = tf.with_ablation_over("context.sources:skeleton", None, Some(&user)).unwrap();

    assert_eq!(ab.profile_names(), ["default", "context.sources:skeleton=on"]);
    let on = ab.profile_selection("context.sources:skeleton=on", Some(&user)).unwrap();
    assert!(on.sources.value.contains(&"skeleton".to_owned()));
    assert_eq!(on.sources.value.len(), 4);
}

#[test]
fn test_f16_the_skeleton_ablation_should_never_generate_a_variant_equal_to_the_base() {
    let tf = TasksFile::from_toml_str("tasks.toml", BASIC).unwrap();
    let user = user_layer("[context.skeleton]\nuse = \"flat\"\n");

    let ab = tf.with_ablation_over("context.skeleton", None, Some(&user)).unwrap();

    let base = ab.profile_selection("default", Some(&user)).unwrap();
    assert_eq!(base.skeleton.value, "flat");
    let names = ab.profile_names();
    assert!(!names.contains(&"context.skeleton=flat".to_owned()), "{names:?}");
    assert_eq!(
        &names[1..],
        ["context.skeleton=adaptive", "context.skeleton=call-graph", "context.skeleton=dependency-graph", "context.skeleton=hybrid"]
    );
    for n in &names[1..] {
        let sel = ab.profile_selection(n, Some(&user)).unwrap();
        assert_ne!(sel.skeleton.value, base.skeleton.value, "{n}");
        assert_eq!(sel.compactor.value, base.compactor.value, "{n}");
    }
}

#[tokio::test]
async fn test_f21_a_large_copy_should_be_reported_once_per_task() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("big")).unwrap();
    std::fs::create_dir_all(f.base.join("small")).unwrap();
    std::fs::write(f.base.join("big/blob.bin"), vec![7u8; 4096]).unwrap();
    std::fs::write(f.base.join("small/a.txt"), "a").unwrap();
    let raw = format!("{}{}", task_toml("big", "big", "true"), task_toml("small", "small", "true"));
    let r = runner(&f, &raw, quiet_builder(Arc::default())).with_copy_warn_bytes(1024);

    let run = r.run(&hone_eval::EvalOptions { repeats: Some(2), ..opts(&f) }).await.unwrap();

    let m = read_json(&run.join("manifest.json"));
    let warnings: Vec<&str> = m["warnings"].as_array().expect("a warnings array").iter().map(|w| w.as_str().unwrap()).collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("big") && !warnings[0].contains("small"), "{warnings:?}");
}

