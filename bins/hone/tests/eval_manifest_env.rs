#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F23 of `docs/plans/eval-profiles-spec.md` (review round 1): the manifest records the
//! values of the environment variables that change runs. This binary has ONE test that sets
//! and removes process-global variables, so nothing races.
//!
//! Contract: `manifest.json` has `env`, an object with exactly the keys `HONE_AUDIT_LOG`,
//! `HONE_CONTEXT_WINDOW`, `HONE_POLICY_BUILTINS` and `HONE_PHASE_COMPACTION`, each the
//! variable's value as a string, or null when unset.

mod eval_support;

use eval_support::*;

const VARS: [&str; 4] = ["HONE_AUDIT_LOG", "HONE_CONTEXT_WINDOW", "HONE_POLICY_BUILTINS", "HONE_PHASE_COMPACTION"];

#[tokio::test]
async fn test_f23_manifest_should_list_the_run_changing_environment_values() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    let r = runner(&f, &task_toml("t", "proj", "true"), quiet_builder(Default::default()));
    // SAFETY: the only test of this binary; no other thread reads these variables.
    unsafe {
        for v in VARS {
            std::env::remove_var(v);
        }
        std::env::set_var("HONE_AUDIT_LOG", "1");
        std::env::set_var("HONE_CONTEXT_WINDOW", "4096");
        std::env::set_var("HONE_PHASE_COMPACTION", "on");
    }

    let run = r.run(&opts(&f)).await.unwrap();

    let m = read_json(&run.join("manifest.json"));
    let env = m["env"].as_object().expect("an env object");
    let mut keys: Vec<&str> = env.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut want = VARS.to_vec();
    want.sort_unstable();
    assert_eq!(keys, want);
    assert_eq!(m["env"]["HONE_AUDIT_LOG"], "1");
    assert_eq!(m["env"]["HONE_CONTEXT_WINDOW"], "4096");
    assert_eq!(m["env"]["HONE_PHASE_COMPACTION"], "on");
    assert!(m["env"]["HONE_POLICY_BUILTINS"].is_null(), "unset -> null: {m}");
    unsafe {
        for v in VARS {
            std::env::remove_var(v);
        }
    }
}
