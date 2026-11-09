#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 read filter for `grep` and `glob` (`docs/plans/policy-p2.5-spec.md` section 8).
//!
//! API assumptions (the spec leaves exact signatures open; simplest reading):
//! - `policy_gate::read_filter(&PolicyConfig, &HashSet<String>) -> ReadFilter`, where
//!   `ReadFilter = Arc<dyn Fn(&Path) -> ReadAccess + Send + Sync>` and
//!   `ReadAccess::{Allowed, NeedsApproval, Denied}` (matched with `matches!`, so no
//!   `PartialEq`/`Debug` is required).
//! - `ToolContext.read_filter: Option<ReadFilter>`; `None` is today's behaviour.
//! - The grep skip note is part of the tool's output value (checked on the serialised JSON
//!   so the exact placement is free): `(N file(s) skipped by permission policy)`.
//! - The native walkers skip hidden files (`.env`) before the filter ever sees them, so the
//!   file-walking tests use visible files (`notes/private.txt` asks, `secret/k.txt` is
//!   denied). The `.env` rules are checked on the filter function directly.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hone_config::policy::{PathOp, Subject};
use hone_sandbox::NullSandbox;
use hone_tools::policy_gate::{
    PolicyConfig, ReadAccess, ReadFilter, approval_key, read_filter, resolve_permission_policy,
};
use hone_tools::{GlobTool, GrepTool, Tool, ToolContext, ToolInput};
use serde_json::{Value, json};

const USER_ASK_NOTES: &str = "version = 2\n[[rules]]\nid = \"ask-notes\"\npath = \"notes/**\"\nop = \"read\"\ndecision = \"ask\"\n";
const PROJECT_DENY_SECRET: &str = "version = 2\n[[rules]]\nid = \"no-secret\"\npath = \"secret/**\"\nop = \"read\"\ndecision = \"deny\"\n";

struct Fx {
    _wd: tempfile::TempDir,
    _home: tempfile::TempDir,
    wd: PathBuf,
    home: PathBuf,
}

fn put(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Workdir with: `.env`, `src/a.txt`, `notes/private.txt`, `secret/k.txt`, all containing
/// the word `needle`.
fn fixture() -> Fx {
    let wd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let wdp = std::fs::canonicalize(wd.path()).unwrap();
    let homep = std::fs::canonicalize(home.path()).unwrap();
    put(&wdp.join(".env"), "API_TOKEN=needle-env\n");
    put(&wdp.join("src/a.txt"), "needle in src\n");
    put(&wdp.join("notes/private.txt"), "needle in notes\n");
    put(&wdp.join("secret/k.txt"), "needle in secret\n");
    put(&homep.join(".config/hone/permissions.toml"), USER_ASK_NOTES);
    put(&wdp.join(".hone/permissions.toml"), PROJECT_DENY_SECRET);
    Fx {
        _wd: wd,
        _home: home,
        wd: wdp,
        home: homep,
    }
}

fn policy(f: &Fx, builtins: bool) -> PolicyConfig {
    resolve_permission_policy(&f.wd, Some(f.home.clone()), builtins)
        .unwrap()
        .expect("policy")
}

fn ctx(wd: &Path, filter: Option<ReadFilter>) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        working_dir: wd.to_path_buf(),
        read_filter: filter,
    }
}

fn none() -> HashSet<String> {
    HashSet::new()
}

fn access(f: &ReadFilter, p: &Path) -> &'static str {
    match f(p) {
        ReadAccess::Allowed => "allowed",
        ReadAccess::NeedsApproval => "needs-approval",
        ReadAccess::Denied => "denied",
    }
}

async fn grep(c: &ToolContext, extra: Value) -> Value {
    let mut input = json!({ "pattern": "needle" });
    if let (Some(o), Some(e)) = (input.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    GrepTool
        .call(ToolInput::new(input), c)
        .await
        .expect("grep ok")
        .value
}

async fn glob(c: &ToolContext, pattern: &str) -> Vec<String> {
    let v = GlobTool
        .call(ToolInput::new(json!({ "pattern": pattern })), c)
        .await
        .expect("glob ok")
        .value;
    v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

/// Number in `(N file(s) skipped by permission policy)`, if present.
fn skipped_count(rendered: &str) -> Option<usize> {
    let end = rendered.find(" file(s) skipped by permission policy)")?;
    let start = rendered[..end].rfind('(')? + 1;
    rendered[start..end].parse().ok()
}

// -- policy_gate::read_filter --------------------------------------------------------------

#[test]
fn filter_maps_allow_ask_and_deny_for_reads() {
    let f = fixture();
    let rf = read_filter(&policy(&f, true), &none());
    assert_eq!(access(&rf, &f.wd.join("src/a.txt")), "allowed");
    assert_eq!(
        access(&rf, &f.wd.join("notes/private.txt")),
        "needs-approval"
    );
    assert_eq!(access(&rf, &f.wd.join("secret/k.txt")), "denied");
}

#[test]
fn filter_applies_the_builtin_env_rules() {
    let f = fixture();
    let rf = read_filter(&policy(&f, true), &none());
    assert_eq!(access(&rf, &f.wd.join(".env")), "needs-approval");
    assert_eq!(access(&rf, &f.wd.join("sub/.env.local")), "needs-approval");
    assert_eq!(access(&rf, &f.wd.join(".env.example")), "allowed");
}

#[test]
fn filter_without_builtins_does_not_protect_env_files() {
    let f = fixture();
    let rf = read_filter(&policy(&f, false), &none());
    assert_eq!(access(&rf, &f.wd.join(".env")), "allowed");
    assert_eq!(access(&rf, &f.wd.join("secret/k.txt")), "denied");
}

#[test]
fn filter_treats_a_path_it_cannot_stat_like_any_other_path() {
    let f = fixture();
    let rf = read_filter(&policy(&f, true), &none());
    assert_eq!(
        access(&rf, &f.wd.join("secret/does-not-exist.txt")),
        "denied"
    );
    assert_eq!(access(&rf, &f.wd.join("nope/missing.txt")), "allowed");
}

#[cfg(unix)]
#[test]
fn filter_canonicalises_symlinks_before_evaluating() {
    let f = fixture();
    let link = f.wd.join("innocent.txt");
    std::os::unix::fs::symlink(f.wd.join("secret/k.txt"), &link).unwrap();
    let rf = read_filter(&policy(&f, true), &none());
    assert_eq!(access(&rf, &link), "denied");
}

#[test]
fn filter_approved_ask_by_pre_approved_id_is_allowed() {
    let f = fixture();
    let mut p = policy(&f, true);
    p.pre_approved = vec!["ask-notes".to_owned()];
    let rf = read_filter(&p, &none());
    assert_eq!(access(&rf, &f.wd.join("notes/private.txt")), "allowed");
    assert_eq!(access(&rf, &f.wd.join(".env")), "needs-approval");
}

#[test]
fn filter_approved_ask_by_session_subject_key_is_allowed() {
    let f = fixture();
    let p = policy(&f, true);
    let target = f.wd.join("notes/private.txt");
    let key = approval_key(
        "ask-notes",
        &Subject::Path {
            path: target.clone(),
            op: PathOp::Read,
        },
    );
    let session: HashSet<String> = [key].into_iter().collect();
    let rf = read_filter(&p, &session);
    assert_eq!(access(&rf, &target), "allowed");
    // only the approved file
    assert_eq!(access(&rf, &f.wd.join("notes/other.txt")), "needs-approval");
}

#[test]
fn filter_deny_is_never_lifted_by_approval() {
    let f = fixture();
    let mut p = policy(&f, true);
    p.pre_approved = vec!["no-secret".to_owned()];
    let session: HashSet<String> = ["no-secret".to_owned()].into_iter().collect();
    let rf = read_filter(&p, &session);
    assert_eq!(access(&rf, &f.wd.join("secret/k.txt")), "denied");
}

#[test]
fn filter_captures_a_snapshot_of_the_session_set() {
    let f = fixture();
    let p = policy(&f, true);
    let mut session = HashSet::new();
    let rf = read_filter(&p, &session);
    session.insert("ask-notes".to_owned());
    assert_eq!(
        access(&rf, &f.wd.join("notes/private.txt")),
        "needs-approval"
    );
}

// -- native grep ---------------------------------------------------------------------------------

#[tokio::test]
async fn grep_skips_denied_and_needs_approval_files_and_reports_the_count() {
    let f = fixture();
    let rf = read_filter(&policy(&f, true), &none());
    let out = grep(&ctx(&f.wd, Some(rf)), json!({})).await;
    let rendered = out.to_string();

    assert!(rendered.contains("src/a.txt"), "{rendered}");
    assert!(!rendered.contains("needle in secret"), "{rendered}");
    assert!(!rendered.contains("secret/k.txt"), "{rendered}");
    assert!(!rendered.contains("needle in notes"), "{rendered}");
    assert!(!rendered.contains("private.txt"), "{rendered}");
    assert!(!rendered.contains("needle-env"), "{rendered}");
    // secret/k.txt and notes/private.txt are skipped (.env is hidden and never walked)
    assert!(
        skipped_count(&rendered).is_some_and(|n| n >= 2),
        "expected a skip note counting at least 2 files: {rendered}"
    );
}

#[tokio::test]
async fn grep_keeps_files_whose_ask_was_approved() {
    let f = fixture();
    let mut p = policy(&f, true);
    p.pre_approved = vec!["ask-notes".to_owned()];
    let rf = read_filter(&p, &none());
    let rendered = grep(&ctx(&f.wd, Some(rf)), json!({})).await.to_string();
    assert!(rendered.contains("needle in notes"), "{rendered}");
    assert!(!rendered.contains("needle in secret"), "{rendered}");
    assert!(
        skipped_count(&rendered).is_some_and(|n| n >= 1),
        "{rendered}"
    );
}

#[tokio::test]
async fn grep_has_no_skip_note_when_nothing_was_skipped() {
    let f = fixture();
    let rf = read_filter(&policy(&f, true), &none());
    let out = grep(&ctx(&f.wd, Some(rf)), json!({ "path": "src" })).await;
    let rendered = out.to_string();
    assert!(rendered.contains("needle in src"), "{rendered}");
    assert!(
        !rendered.contains("skipped by permission policy"),
        "{rendered}"
    );
}

#[tokio::test]
async fn grep_without_a_filter_changes_nothing() {
    let f = fixture();
    let rendered = grep(&ctx(&f.wd, None), json!({})).await.to_string();
    assert!(rendered.contains("needle in secret"), "{rendered}");
    assert!(rendered.contains("needle in notes"), "{rendered}");
    assert!(rendered.contains("needle in src"), "{rendered}");
    assert!(
        !rendered.contains("skipped by permission policy"),
        "{rendered}"
    );
}

#[tokio::test]
async fn grep_honours_an_arbitrary_filter_closure() {
    let f = fixture();
    let rf: ReadFilter = Arc::new(|p: &Path| {
        let s = p.to_string_lossy();
        if s.contains("secret") {
            ReadAccess::Denied
        } else if s.contains("notes") {
            ReadAccess::NeedsApproval
        } else {
            ReadAccess::Allowed
        }
    });
    let rendered = grep(&ctx(&f.wd, Some(rf)), json!({})).await.to_string();
    assert!(rendered.contains("needle in src"));
    assert!(!rendered.contains("needle in secret"));
    assert!(!rendered.contains("needle in notes"));
    assert_eq!(skipped_count(&rendered), Some(2), "{rendered}");
}

// -- native glob ---------------------------------------------------------------------------------

#[tokio::test]
async fn glob_drops_denied_entries_and_keeps_needs_approval_entries() {
    let f = fixture();
    let rf = read_filter(&policy(&f, true), &none());
    let files = glob(&ctx(&f.wd, Some(rf)), "**/*.txt").await;
    assert!(files.iter().any(|p| p.ends_with("src/a.txt")), "{files:?}");
    assert!(
        files.iter().any(|p| p.ends_with("notes/private.txt")),
        "a name is not the content: NeedsApproval entries stay: {files:?}"
    );
    assert!(
        !files.iter().any(|p| p.contains("secret")),
        "denied entries are dropped: {files:?}"
    );
}

#[tokio::test]
async fn glob_without_a_filter_changes_nothing() {
    let f = fixture();
    let files = glob(&ctx(&f.wd, None), "**/*.txt").await;
    assert_eq!(files.len(), 3, "{files:?}");
    assert!(files.iter().any(|p| p.contains("secret")));
}

#[tokio::test]
async fn glob_honours_an_arbitrary_filter_closure() {
    let f = fixture();
    let rf: ReadFilter = Arc::new(|p: &Path| {
        if p.to_string_lossy().contains("src") {
            ReadAccess::Denied
        } else {
            ReadAccess::NeedsApproval
        }
    });
    let files = glob(&ctx(&f.wd, Some(rf)), "**/*.txt").await;
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(!files.iter().any(|p| p.contains("src")));
}
