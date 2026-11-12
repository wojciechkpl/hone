#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R8 of `docs/plans/doc-retrieval-spec.md` through the binary: `hone strategies` lists the
//! retriever and the chunkers and shows the `[retrieval]` settings. Extends the output
//! contract of `strategies_cli.rs`: `--json` `slots` gains, after `memory.writer` and always
//! present (retrieval being off or on),
//! - `{ "slot": "retrieval.retriever", "name": "fts5-docs", "params": { "top_k": N, "max_tokens": N }, "origin": .. }`
//! - `{ "slot": "retrieval.chunker", "name": "markdown" | "fixed", "params": { "lines": N, "overlap": N }, "origin": .. }`
//!
//! and `context.sources` names `retrieval` when it is on. The text output mentions `fts5-docs`
//! and both chunker names. Invalid `[retrieval]` values make `--check` exit 2 naming the file
//! and the key.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (home, project) = (root.join("home"), root.join("project"));
        std::fs::create_dir_all(home.join(".config/hone")).unwrap();
        std::fs::create_dir_all(project.join(".hone")).unwrap();
        let e = Self { _tmp: tmp, home, project };
        e.user_config("");
        e
    }

    fn user_config(&self, raw: &str) -> PathBuf {
        let p = self.home.join(".config/hone/config.toml");
        // Keeps a build without `hone strategies` from reaching a local model.
        let guard = "[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n\n";
        std::fs::write(&p, format!("{guard}{raw}")).unwrap();
        p
    }

    fn project_config(&self, raw: &str) -> PathBuf {
        let p = self.project.join(".hone/config.toml");
        std::fs::write(&p, raw).unwrap();
        p
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hone"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.project)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn json(&self) -> Value {
        let out = self.run(&["strategies", "--json"]);
        assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("not JSON: {e}"))
    }
}

fn slot<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["slot"] == name)
        .unwrap_or_else(|| panic!("no slot {name}"))
}

#[test]
fn test_r8_strategies_should_list_the_retriever_and_the_chunker_with_defaults() {
    let e = Env::new();
    let v = e.json();

    let r = slot(&v, "retrieval.retriever");
    assert_eq!(r["name"], "fts5-docs");
    assert_eq!(r["params"]["top_k"], 3);
    assert_eq!(r["params"]["max_tokens"], 1200);
    assert_eq!(r["origin"], "default");
    let c = slot(&v, "retrieval.chunker");
    assert_eq!(c["name"], "markdown");
    assert_eq!(c["params"]["lines"], 40);
    assert_eq!(c["params"]["overlap"], 5);
}

#[test]
fn test_r8_text_output_should_mention_the_retriever_and_both_chunkers() {
    let e = Env::new();
    let out = e.run(&["strategies"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    for needle in ["retrieval.retriever", "fts5-docs", "retrieval.chunker", "markdown", "fixed"] {
        assert!(text.contains(needle), "{needle} missing:\n{text}");
    }
}

#[test]
fn test_r8_the_configured_values_and_their_file_should_show() {
    let e = Env::new();
    let user = e.user_config(
        "[context]\nsources = [\"memory-index\", \"recall\", \"skills\", \"retrieval\"]\n\n[retrieval]\ntop_k = 5\nmax_tokens = 2000\nchunker = \"fixed\"\n\n[retrieval.fixed]\nlines = 20\noverlap = 2\n",
    );

    let v = e.json();

    let names: Vec<&str> = slot(&v, "context.sources")["name"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(names.contains(&"retrieval"), "{names:?}");
    let r = slot(&v, "retrieval.retriever");
    assert_eq!((r["params"]["top_k"].as_u64(), r["params"]["max_tokens"].as_u64()), (Some(5), Some(2000)));
    assert_eq!(r["origin"], format!("file:{}", user.display()));
    let c = slot(&v, "retrieval.chunker");
    assert_eq!(c["name"], "fixed");
    assert_eq!((c["params"]["lines"].as_u64(), c["params"]["overlap"].as_u64()), (Some(20), Some(2)));
}

#[test]
fn test_r8_a_project_file_cannot_raise_top_k_and_the_warning_names_it() {
    let e = Env::new();
    let project = e.project_config("[retrieval]\ntop_k = 15\n");

    let v = e.json();

    assert_eq!(slot(&v, "retrieval.retriever")["params"]["top_k"], 3);
    let w = v["warnings"].as_array().unwrap();
    assert!(
        w.iter().any(|x| x["path"] == project.display().to_string() && x["message"].as_str().unwrap().contains("top_k")),
        "{w:?}"
    );
}

#[test]
fn test_r8_invalid_retrieval_settings_should_make_check_exit_two_naming_the_file_and_key() {
    for (raw, key) in [
        ("[retrieval]\ntop_k = 99\n", "top_k"),
        ("[retrieval]\nmax_tokens = 5\n", "max_tokens"),
        ("[retrieval]\nchunker = \"semantic\"\n", "retrieval.chunker"),
        ("[retrieval]\nbogus = 1\n", "bogus"),
    ] {
        let e = Env::new();
        let path = e.user_config(raw);

        let out = e.run(&["strategies", "--check"]);

        assert_eq!(out.status.code(), Some(2), "{raw}");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(err.contains(&path.display().to_string()) && err.contains(key), "{raw}: {err}");
        assert!(out.stdout.is_empty());
    }
}
