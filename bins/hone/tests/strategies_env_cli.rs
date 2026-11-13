#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E8 of `docs/plans/env-snapshot-spec.md` through the binary: `hone strategies`
//! shows `env-snapshot` and its `timeout_secs`. Extends the output contract of
//! `strategies_cli.rs`: in `--json`, the `context.sources` slot's `params` is an
//! object keyed by source name; when `env-snapshot` is enabled it holds
//! `"env-snapshot": { "timeout_secs": N }` (and no such key when it is off). The text
//! output mentions `env-snapshot` and its timeout too. An out-of-range `timeout_secs`
//! makes `--check` exit 2 naming the file and the key.

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

fn sources(v: &Value) -> &Value {
    v["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["slot"] == "context.sources")
        .expect("a context.sources slot")
}

const ON: &str = "[context]\nsources = [\"memory-index\", \"recall\", \"skills\", \"skeleton\", \"env-snapshot\"]\n";

#[test]
fn test_e8_strategies_should_not_list_env_snapshot_when_it_is_off() {
    let e = Env::new();
    let v = e.json();
    let names: Vec<&str> = sources(&v)["name"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    assert!(!names.contains(&"env-snapshot"), "{names:?}");
    assert!(sources(&v)["params"].get("env-snapshot").is_none());
}

#[test]
fn test_e8_strategies_should_show_env_snapshot_and_its_timeout_when_on() {
    let e = Env::new();
    e.user_config(&format!("{ON}\n[context.env-snapshot]\ntimeout_secs = 20\n"));

    let v = e.json();

    let names: Vec<&str> = sources(&v)["name"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    assert!(names.contains(&"env-snapshot"), "{names:?}");
    assert_eq!(sources(&v)["params"]["env-snapshot"]["timeout_secs"], 20);

    let out = e.run(&["strategies"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("env-snapshot") && text.contains("20"), "{text}");
}

#[test]
fn test_e8_the_default_timeout_should_show_as_fifteen() {
    let e = Env::new();
    e.user_config(ON);
    assert_eq!(e.json()["slots"][0]["params"]["env-snapshot"]["timeout_secs"], 15);
}

#[test]
fn test_e8_a_project_file_cannot_turn_it_on_or_raise_its_timeout() {
    let e = Env::new();
    let project = e.project_config(&format!("{ON}\n[context.env-snapshot]\ntimeout_secs = 50\n"));

    let v = e.json();

    let names: Vec<&str> = sources(&v)["name"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    assert!(!names.contains(&"env-snapshot"));
    let w = v["warnings"].as_array().unwrap();
    assert!(
        w.iter().any(|x| x["path"] == project.display().to_string() && x["message"].as_str().unwrap().contains("timeout_secs")),
        "{w:?}"
    );
}

#[test]
fn test_e8_an_out_of_range_timeout_should_make_check_exit_two() {
    for n in [0, 61] {
        let e = Env::new();
        let path = e.user_config(&format!("{ON}\n[context.env-snapshot]\ntimeout_secs = {n}\n"));

        let out = e.run(&["strategies", "--check"]);

        assert_eq!(out.status.code(), Some(2), "timeout_secs = {n}");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(err.contains(&path.display().to_string()) && err.contains("timeout_secs"), "{err}");
        assert!(out.stdout.is_empty());
    }
}
