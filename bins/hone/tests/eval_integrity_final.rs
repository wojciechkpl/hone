#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F35, F36 and F37 of `docs/plans/eval-profiles-spec.md` ("Final verification").
//!
//! Contract pinned:
//! - F35: whenever `integrity_violated` is true, `hone eval` prints on stderr that the suite
//!   changed during the run, naming the first changed path (relative to the tasks file's
//!   directory, e.g. `checks/ref.txt`) and saying the results cannot be trusted (the line
//!   contains `changed`, the path and `trust`); it exits 1 whatever the pass rate; and the run
//!   is not recorded in the eval history (library: `latest_with_tag` finds nothing);
//! - F36: on Unix a rewrite whose content AND mtime are restored (`touch -r`) is still detected
//!   (through ctime / inode);
//! - F37: `--check` prints a warning (stderr, containing `warning` and the link's path relative
//!   to the tasks file) for each directory symlink under an integrity dir that points OUTSIDE
//!   the integrity dirs, and still exits 0; a directory symlink to another integrity dir is not
//!   mentioned.
//!
//! The CLI tests use a tiny Ollama-protocol server that always answers with plain text, so no
//! agent calls a tool and the success checks do the tampering.

mod eval_support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use eval_support::*;
use hone_eval::{EvalOptions, EvalRunStore};
use serde_json::json;

// ---- a model that just answers ----------------------------------------------------------------------

fn serve(mut stream: TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
        .unwrap_or(0);
    while buf.len() < header_end + len {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let payload = format!(
        "{}\n{}\n",
        json!({"model":"m","message":{"role":"assistant","content":"all done"},"done":false}),
        json!({"model":"m","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":3,"eval_count":2}),
    );
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn mock_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            std::thread::spawn(move || serve(s));
        }
    });
    url
}

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    dir: PathBuf,
}

impl Env {
    fn new(base_url: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (home, dir) = (root.join("home"), root.join("suite"));
        std::fs::create_dir_all(home.join(".config/hone")).unwrap();
        for d in ["proj", "checks"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("checks/ref.txt"), "reference text\n").unwrap();
        std::fs::write(home.join(".config/hone/config.toml"), format!("[providers.ollama]\nbase_url = \"{base_url}\"\n")).unwrap();
        Self { _tmp: tmp, root, home, dir }
    }

    fn tasks(&self, raw: &str) {
        std::fs::write(self.dir.join("tasks.toml"), raw).unwrap();
    }

    fn hone(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hone"))
            .arg("eval")
            .arg(self.dir.join("tasks.toml"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .env("HONE_DISABLE_PERSISTENT_STORES", "1")
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

// ---- F35 -----------------------------------------------------------------------------------------------

#[test]
fn test_f35_a_violation_after_a_passing_run_should_fail_the_eval_visibly() {
    let e = Env::new(&mock_url());
    e.tasks(&format!(
        "model = \"ollama:m\"\n\n[eval]\nintegrity = [\"checks\"]\n\n{}{}{}",
        task_toml("t1", "proj", "true"),
        task_toml("t2", "proj", "echo changed >> \"$EVAL_SUITE_DIR/checks/ref.txt\""),
        task_toml("t3", "proj", "true"),
    ));

    let out = e.hone(&["-o", "out"]);

    assert_eq!(out.status.code(), Some(1), "exit 1 whatever the pass rate; stdout: {}", text(&out.stdout));
    let err = text(&out.stderr);
    let line = err
        .lines()
        .find(|l| l.contains("checks/ref.txt"))
        .unwrap_or_else(|| panic!("a stderr line names the first changed path: {err}"));
    assert!(line.contains("changed"), "{line}");
    assert!(err.to_lowercase().contains("trust"), "it says the results cannot be trusted: {err}");
}

#[tokio::test]
async fn test_f35_a_violated_run_should_not_be_added_to_the_eval_history() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("checks")).unwrap();
    std::fs::write(f.base.join("checks/ref.txt"), "reference text\n").unwrap();
    let raw = format!(
        "[eval]\nintegrity = [\"checks\"]\n\n{}{}",
        task_toml("t1", "proj", "true"),
        task_toml("t2", "proj", "echo changed >> \"$EVAL_SUITE_DIR/checks/ref.txt\""),
    );
    let store = Arc::new(EvalRunStore::open_in_memory().await.unwrap());

    let run = runner(&f, &raw, quiet_builder(Arc::default()))
        .run(&EvalOptions { tag: Some("violated".to_owned()), history: Some(store.clone()), ..opts(&f) })
        .await
        .unwrap();

    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], true, "harness");
    assert!(store.latest_with_tag("violated").await.unwrap().is_none(), "an untrustworthy run is not recorded");
}

// ---- F36 -----------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_f36_a_rewrite_with_content_and_mtime_restored_should_still_be_detected() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("suite")).unwrap();
    std::fs::write(f.base.join("suite/ref.txt"), "original text\n").unwrap();
    // Copy the file aside, rewrite it, restore the content, then restore the mtime too.
    let check = "f=\"$EVAL_SUITE_DIR/suite/ref.txt\"; d=$(mktemp -d); cp \"$f\" \"$d/aside\"; touch -r \"$f\" \"$d/stamp\"; \
                 echo junk > \"$f\"; cp \"$d/aside\" \"$f\"; touch -r \"$d/stamp\" \"$f\"; rm -rf \"$d\"";
    let raw = format!("[eval]\nintegrity = [\"suite\"]\n\n{}{}", task_toml("t1", "proj", check), task_toml("t2", "proj", "true"));

    let run = runner(&f, &raw, quiet_builder(Arc::default())).run(&opts(&f)).await.unwrap();

    assert_eq!(std::fs::read_to_string(f.base.join("suite/ref.txt")).unwrap(), "original text\n", "harness: content restored");
    for t in ["t1", "t2"] {
        let o = outcome(&run, t);
        assert_eq!(o["status"], "infra_error", "{t}: {o}");
        assert!(o["message"].as_str().is_some_and(|m| m.contains("suite modified during the run")), "{t}: {o}");
    }
    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], true);
}

// ---- F37 -----------------------------------------------------------------------------------------------

#[test]
fn test_f37_check_should_warn_about_a_directory_symlink_that_points_outside_the_integrity_dirs() {
    let e = Env::new("http://127.0.0.1:9");
    let elsewhere: &Path = &e.root.join("elsewhere");
    std::fs::create_dir_all(elsewhere).unwrap();
    std::fs::write(elsewhere.join("lib.txt"), "x").unwrap();
    std::fs::create_dir_all(e.dir.join("realdata")).unwrap();
    std::os::unix::fs::symlink(elsewhere, e.dir.join("checks/sharedlib")).unwrap(); // outside
    std::os::unix::fs::symlink("../realdata", e.dir.join("checks/inner")).unwrap(); // another integrity dir
    e.tasks(&format!(
        "model = \"ollama:m\"\n\n[eval]\nintegrity = [\"checks\", \"realdata\"]\n\n{}",
        task_toml("t1", "proj", "true")
    ));

    let out = e.hone(&["--check"]);

    assert_eq!(out.status.code(), Some(0), "still exits 0; stderr: {}", text(&out.stderr));
    let err = text(&out.stderr);
    let warn = err.lines().find(|l| l.contains("sharedlib")).unwrap_or_else(|| panic!("a warning names the link: {err:?}"));
    assert!(warn.to_lowercase().contains("warning") && warn.contains("checks/sharedlib"), "{warn}");
    assert!(!err.contains("inner"), "a link to another integrity directory is fine: {err}");
}
