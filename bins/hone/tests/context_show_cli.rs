#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, dead_code)]
//! CR12 of `docs/plans/context-receipts-spec.md` at the binary: `hone context
//! show [--session NAME] [--run R] [--turn N] [--json]` against a sessions
//! database in a temp HOME (`$HOME/.local/share/hone/sessions.db`). Nothing
//! reaches the network or a model.
//!
//! - unknown session / run / turn: exit code 1 and a message on stderr;
//! - a session without receipts: a one-line note on stdout, exit 0;
//! - default session: the most recently updated one;
//! - `--json`: the stored receipts as an array with `run` and `seq`.

#[path = "../src/session.rs"]
mod session;

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use hone_proto::ContextReceipt;
use session::SessionStore;

fn receipt(turn: u32, call: u32) -> ContextReceipt {
    ContextReceipt {
        turn,
        call,
        model: "mock:test".to_owned(),
        request_sha256: format!("{turn:02x}{call:02x}").repeat(16),
        blocks: vec![],
        dropped: vec![],
        compaction: vec![],
        tools_offered: vec![],
    }
}

struct Home {
    _tmp: tempfile::TempDir,
    home: PathBuf,
}

impl Home {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(home.join(".local/share/hone")).unwrap();
        // A dummy provider config, as the other binary-level tests use.
        std::fs::create_dir_all(home.join(".config/hone")).unwrap();
        std::fs::write(
            home.join(".config/hone/config.toml"),
            "default_model = \"openai:dummy-model\"\n[providers.openai]\nbase_url = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        Self { _tmp: tmp, home }
    }

    fn db(&self) -> PathBuf {
        self.home.join(".local/share/hone/sessions.db")
    }

    async fn store(&self) -> SessionStore {
        SessionStore::open(&self.db()).await.unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hone"))
            .args(["context", "show"])
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

async fn session_with_receipts(h: &Home, name: &str, receipts: &[ContextReceipt]) {
    let store = h.store().await;
    store.save_session(name, &[]).await.unwrap();
    store.append_context_receipts(name, receipts).await.unwrap();
}

#[tokio::test]
async fn test_cr12_unknown_session_should_exit_one_with_a_message() {
    let h = Home::new();
    session_with_receipts(&h, "real", &[receipt(1, 1)]).await;

    let out = h.run(&["--session", "missing"]);

    assert_eq!(out.status.code(), Some(1), "stderr: {}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("missing"),
        "{}",
        text(&out.stderr)
    );
}

#[tokio::test]
async fn test_cr12_session_without_receipts_should_print_a_note_and_exit_zero() {
    let h = Home::new();
    h.store().await.save_session("bare", &[]).await.unwrap();

    let out = h.run(&["--session", "bare"]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert_eq!(stdout.trim_end().lines().count(), 1, "{stdout:?}");
    assert!(
        stdout.to_lowercase().contains("no context receipts"),
        "{stdout}"
    );
}

#[tokio::test]
async fn test_cr12_unknown_run_and_turn_should_exit_one() {
    let h = Home::new();
    session_with_receipts(&h, "s", &[receipt(1, 1)]).await;

    assert_eq!(
        h.run(&["--session", "s", "--run", "9"]).status.code(),
        Some(1)
    );
    let out = h.run(&["--session", "s", "--turn", "9"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!text(&out.stderr).is_empty());
}

#[tokio::test]
async fn test_cr12_json_should_print_the_stored_receipts_with_run_and_seq() {
    let h = Home::new();
    session_with_receipts(&h, "s", &[receipt(1, 1), receipt(1, 2)]).await;

    let out = h.run(&["--session", "s", "--json"]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON on stdout");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(
        (arr[0]["run"].as_u64(), arr[0]["seq"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(arr[1]["receipt"]["call"], 2);
}

#[tokio::test]
async fn test_cr12_default_session_should_be_the_most_recently_updated() {
    let h = Home::new();
    session_with_receipts(&h, "older", &[receipt(1, 1)]).await;
    std::thread::sleep(std::time::Duration::from_millis(1100)); // updated_at has 1 s resolution
    session_with_receipts(&h, "newer", &[receipt(7, 3)]).await;

    let out = h.run(&["--json"]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v[0]["receipt"]["turn"], 7, "the newer session's receipt");
}

#[tokio::test]
async fn test_cr12_text_output_should_show_the_run_turn_and_call() {
    let h = Home::new();
    session_with_receipts(&h, "s", &[receipt(2, 3)]).await;

    let out = h.run(&["--session", "s"]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("run 1") && stdout.contains("turn 2") && stdout.contains("call 3"),
        "{stdout}"
    );
}
