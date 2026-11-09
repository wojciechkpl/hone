//! P2.5 section 15.7: `SymbolAtTool::subjects` adds the as-written path (14.7) next to the
//! canonical one, so a symlinked dotfile is still caught by the policy.
//!
//! WIRING NEEDED (the `mod` line lives in production source, which the test author may not
//! edit): add to the end of `symbol_tools.rs`
//!
//! ```ignore
//! #[cfg(test)]
//! #[path = "symbol_tools_aswritten_tests.rs"]
//! mod aswritten_tests;
//! ```
//!
//! Until that line exists these tests are not compiled, so they cannot be run red.

use super::*;
use hone_config::policy::{PathOp, Subject};

#[cfg(unix)]
#[tokio::test]
async fn symbol_at_subjects_include_the_as_written_and_the_canonical_path() {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd.path()).unwrap();
    let home = std::fs::canonicalize(home.path()).unwrap();
    std::fs::write(wd.join("notes.rs"), "fn f() {}\n").unwrap();
    let link = home.join(".netrc");
    std::os::unix::fs::symlink(wd.join("notes.rs"), &link).unwrap();

    let index = Arc::new(SqliteIndex::in_memory().await.expect("index"));
    let tool = SymbolAtTool::new(index);
    let sctx = SubjectContext {
        workdir: wd.clone(),
        home: Some(home.clone()),
        case_insensitive: false,
    };
    let ts = tool.subjects(
        &ToolInput::new(json!({ "file": link.to_string_lossy(), "line": 1 })),
        &sctx,
    );

    let has = |p: &std::path::Path| {
        ts.subjects.contains(&Subject::Path {
            path: p.to_path_buf(),
            op: PathOp::Read,
        })
    };
    assert!(has(&link), "as-written path missing: {:?}", ts.subjects);
    assert!(
        has(&wd.join("notes.rs")),
        "canonical path missing: {:?}",
        ts.subjects
    );
}

#[tokio::test]
async fn symbol_at_subjects_for_a_plain_file_are_unchanged() {
    let wd = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd.path()).unwrap();
    let index = Arc::new(SqliteIndex::in_memory().await.expect("index"));
    let tool = SymbolAtTool::new(index);
    let sctx = SubjectContext {
        workdir: wd.clone(),
        home: None,
        case_insensitive: false,
    };
    let ts = tool.subjects(
        &ToolInput::new(json!({ "file": "src/a.rs", "line": 1 })),
        &sctx,
    );
    assert_eq!(
        ts.subjects,
        vec![Subject::Path {
            path: wd.join("src/a.rs"),
            op: PathOp::Read
        }]
    );
}
