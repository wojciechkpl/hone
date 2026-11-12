#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! G4, G6 and G7 of `docs/plans/fts5-recall-spec.md` through the binary with a temp HOME.
//! No model is called (the Ollama endpoint points at a closed port).
//!
//! Contract pinned:
//! - G4: `hone strategies --json` lists the active `memory.recaller`; with
//!   `[memory] recaller = "fts5"` its name is `fts5` and `params` carry `top_k` and
//!   `exclude_kinds`. An unknown recaller name is an error (exit 2) that lists `fts5`
//!   among the valid names.
//! - G6: `hone --encrypted-memory strategies --json` (with the memory passphrase variable
//!   set) and `recaller = "fts5"`: the recaller slot is `bm25`, and `warnings` holds exactly
//!   one entry whose message mentions `fts5` and `bm25`.
//! - G7: `hone memory reindex` prints a line containing `indexed <N>` (case-insensitive)
//!   and leaves the FTS5 table `memories_fts` in `~/.local/share/hone/memory.db` with the
//!   N memories searchable.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use hone_memory::{Memory, MemoryKind, MemoryStore, SqliteStore};
use serde_json::Value;
use sqlx::Row;

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Env {
    fn new(user_config: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (home, project) = (root.join("home"), root.join("project"));
        std::fs::create_dir_all(home.join(".config/hone")).unwrap();
        std::fs::create_dir_all(home.join(".local/share/hone")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            home.join(".config/hone/config.toml"),
            format!("[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n\n{user_config}"),
        )
        .unwrap();
        Self {
            _tmp: tmp,
            home,
            project,
        }
    }

    fn command(&self, env: &[(&str, &str)], args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hone"));
        cmd.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.project)
            .stdin(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, env: &[(&str, &str)], args: &[&str]) -> Output {
        self.command(env, args).output().unwrap()
    }

    fn json(&self, env: &[(&str, &str)], args: &[&str]) -> Value {
        let out = self.run(env, args);
        assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}", text(&out.stdout)))
    }

    fn db(&self) -> PathBuf {
        self.home.join(".local/share/hone/memory.db")
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn recaller(v: &Value) -> &Value {
    v["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["slot"] == "memory.recaller")
        .unwrap_or_else(|| panic!("no memory.recaller slot in {v}"))
}

#[test]
fn test_g4_strategies_should_list_fts5_as_the_active_recaller_when_configured() {
    let e = Env::new("[memory]\nrecaller = \"fts5\"\n");

    let v = e.json(&[], &["strategies", "--json"]);

    assert_eq!(recaller(&v)["name"], "fts5");
    assert_eq!(recaller(&v)["params"]["top_k"], 5);
    assert_eq!(
        recaller(&v)["params"]["exclude_kinds"],
        serde_json::json!(["outcome"])
    );
    assert!(v["warnings"].as_array().unwrap().is_empty(), "{v}");
}

#[test]
fn test_g4_an_unknown_recaller_should_list_fts5_among_the_valid_names() {
    let e = Env::new("[memory]\nrecaller = \"vector-db\"\n");

    let out = e.run(&[], &["strategies", "--check"]);

    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(
        err.contains("vector-db") && err.contains("fts5") && err.contains("bm25"),
        "{err}"
    );
}

#[test]
fn test_g6_fts5_with_encrypted_memory_should_fall_back_to_bm25_with_one_warning() {
    let e = Env::new("[memory]\nrecaller = \"fts5\"\n");

    let v = e.json(
        &[("HONE_MEMORY_PASSPHRASE", "pw")],
        &["--encrypted-memory", "strategies", "--json"],
    );

    assert_eq!(recaller(&v)["name"], "bm25");
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{v}");
    let msg = warnings[0]["message"].as_str().unwrap();
    assert!(msg.contains("fts5") && msg.contains("bm25"), "{msg}");
}

async fn seed(db: &Path, n: usize) {
    let store = SqliteStore::open_with_decay_interval(db, 0).await.unwrap();
    for i in 0..n {
        store
            .record(Memory {
                id: format!("m{i}"),
                content: format!("note {i} about quokkas"),
                tags: vec![],
                weight: 1.0,
                created_at: 10 + i as u64,
                decay_at: None,
                kind: MemoryKind::Feedback,
                description: None,
                embedding: None,
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn test_g7_memory_reindex_should_print_the_count_and_leave_a_searchable_index() {
    let e = Env::new("");
    seed(&e.db(), 4).await;
    let mut cmd = e.command(&[], &["memory", "reindex"]);

    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    assert!(
        text(&out.stdout).to_lowercase().contains("indexed 4"),
        "{}",
        text(&out.stdout)
    );
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", e.db().display()))
        .await
        .unwrap();
    let hits = sqlx::query("SELECT count(*) FROM memories_fts WHERE memories_fts MATCH 'quokkas'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get::<i64, _>(0);
    assert_eq!(hits, 4);
}
