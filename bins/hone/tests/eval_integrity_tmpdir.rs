#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F33 of `docs/plans/eval-profiles-spec.md`: the system temp directory is not hashed, so a
//! `TMPDIR` below an integrity directory (where every run's working-directory copy lives while
//! it runs) does not flag the suite. One test, alone in its binary, because it sets `TMPDIR`
//! for the whole process.

mod eval_support;

use std::sync::Arc;

use eval_support::*;

#[tokio::test]
async fn test_f33_a_tmpdir_below_an_integrity_directory_should_not_be_hashed() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("scratch/tmp")).unwrap();
    let tmp = f.base.join("scratch/tmp");
    // SAFETY: the only test of this binary; no other thread reads the environment.
    let old = std::env::var_os("TMPDIR");
    unsafe { std::env::set_var("TMPDIR", &tmp) };
    let raw = format!(
        "[eval]\nintegrity = [\"scratch\"]\n\n{}{}",
        task_toml("t1", "proj", "true"),
        task_toml("t2", "proj", "true")
    );

    let run = runner(&f, &raw, quiet_builder(Arc::default())).run(&opts(&f)).await.unwrap();

    unsafe {
        match old {
            Some(v) => std::env::set_var("TMPDIR", v),
            None => std::env::remove_var("TMPDIR"),
        }
    }
    for t in ["t1", "t2"] {
        assert_eq!(outcome(&run, t)["status"], "pass", "{t}: copies under TMPDIR are not a suite change");
    }
    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], false);
}
