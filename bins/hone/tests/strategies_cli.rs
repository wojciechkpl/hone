#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! S10 (and the exit code of S4) of `docs/plans/strategy-registry-spec.md`: `hone
//! strategies [--json] [--check]` through the binary, with a temp HOME (user
//! config) and a temp project directory (cwd, project config). No model is called.
//!
//! Output contract:
//! - text: one block per slot with the slot key, the active name, its parameters,
//!   its origin and what chose it (flag, env var name, file path or default), then
//!   any warnings with the file they name;
//! - `--json`: one object `{ "slots": [ { "slot", "name", "params", "origin" }, .. ],
//!   "warnings": [ { "path", "message" }, .. ] }` where `origin` is `"default"`,
//!   `"flag"`, `"env:<NAME>"` or `"file:<path>"` (the origin of the name, or of the
//!   list for `context.sources`, whose `name` is an array of source names). Slots, in
//!   this order: `context.sources`, `context.skeleton`, `context.compactor`,
//!   `memory.recaller`, `memory.writer`;
//! - `--check`: nothing on stdout and exit 0 when valid; on an invalid config the
//!   error (naming the file and key, or the variable/flag) on stderr and exit 2.
//!   A plain `hone strategies` with an invalid config exits 2 as well.

use std::path::{Path, PathBuf};
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
        Self {
            _tmp: tmp,
            home,
            project,
        }
    }

    fn user_config(&self, raw: &str) -> PathBuf {
        let p = self.home.join(".config/hone/config.toml");
        // Keeps a build without `hone strategies` (the red state) from reaching a local model.
        let guard = "[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n\n";
        std::fs::write(&p, format!("{guard}{raw}")).unwrap();
        p
    }

    fn project_config(&self, raw: &str) -> PathBuf {
        let p = self.project.join(".hone/config.toml");
        std::fs::write(&p, raw).unwrap();
        p
    }

    fn run(&self, env: &[(&str, &str)], args: &[&str]) -> Output {
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
        cmd.output().unwrap()
    }

    fn json(&self, env: &[(&str, &str)], args: &[&str]) -> Value {
        let out = self.run(env, args);
        assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}", text(&out.stdout)))
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn slot<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["slots"]
        .as_array()
        .expect("slots array")
        .iter()
        .find(|s| s["slot"] == name)
        .unwrap_or_else(|| panic!("no slot {name} in {v}"))
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn test_s10_json_should_list_every_slot_with_defaults() {
    let e = Env::new();
    let v = e.json(&[], &["strategies", "--json"]);

    let slots: Vec<&str> = v["slots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["slot"].as_str().unwrap())
        .collect();
    assert_eq!(
        slots,
        [
            "context.sources",
            "context.skeleton",
            "context.compactor",
            "memory.recaller",
            "memory.writer"
        ]
    );
    assert_eq!(slot(&v, "context.skeleton")["name"], "adaptive");
    assert_eq!(slot(&v, "context.skeleton")["origin"], "default");
    assert_eq!(slot(&v, "context.skeleton")["params"]["max_tokens"], 2000);
    assert_eq!(slot(&v, "context.compactor")["name"], "graduated");
    assert_eq!(slot(&v, "context.compactor")["params"]["phase"], false);
    assert_eq!(slot(&v, "memory.recaller")["name"], "bm25");
    assert_eq!(slot(&v, "memory.recaller")["params"]["top_k"], 5);
    assert_eq!(
        slot(&v, "memory.recaller")["params"]["exclude_kinds"],
        serde_json::json!(["outcome"])
    );
    assert_eq!(slot(&v, "memory.writer")["name"], "triggers");
    assert_eq!(
        slot(&v, "context.sources")["name"],
        serde_json::json!(["memory-index", "recall", "skills", "skeleton"])
    );
    assert_eq!(v["warnings"], serde_json::json!([]));
}

#[test]
fn test_s10_text_should_show_each_slot_its_value_and_what_chose_it() {
    let e = Env::new();
    let user = e.user_config("[context.skeleton]\nuse = \"flat\"\n");

    let out = e.run(&[], &["strategies"]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    let s = text(&out.stdout);
    for needle in [
        "context.sources",
        "context.skeleton",
        "context.compactor",
        "memory.recaller",
        "memory.writer",
        "flat",
        "graduated",
        "bm25",
        "triggers",
        "default",
    ] {
        assert!(s.contains(needle), "{needle} missing:\n{s}");
    }
    assert!(
        s.contains(&path_str(&user)),
        "the file that chose `flat`:\n{s}"
    );
}

#[test]
fn test_s10_origins_should_name_the_file_the_env_var_or_the_flag() {
    let e = Env::new();
    let user = e.user_config("[context.skeleton]\nuse = \"flat\"\n");
    let project = e.project_config("[memory]\nwriter = \"off\"\n");

    let v = e.json(&[], &["strategies", "--json"]);
    assert_eq!(
        slot(&v, "context.skeleton")["origin"],
        format!("file:{}", path_str(&user))
    );
    assert_eq!(
        slot(&v, "memory.writer")["origin"],
        format!("file:{}", path_str(&project))
    );

    let v = e.json(
        &[("HONE_SKELETON_STRATEGY", "hybrid")],
        &["strategies", "--json"],
    );
    assert_eq!(slot(&v, "context.skeleton")["name"], "hybrid");
    assert_eq!(
        slot(&v, "context.skeleton")["origin"],
        "env:HONE_SKELETON_STRATEGY"
    );

    let v = e.json(
        &[("HONE_SKELETON_STRATEGY", "hybrid")],
        &["--skeleton-strategy", "call-graph", "strategies", "--json"],
    );
    assert_eq!(slot(&v, "context.skeleton")["name"], "call-graph");
    assert_eq!(slot(&v, "context.skeleton")["origin"], "flag");
}

#[test]
fn test_s10_no_inject_should_show_sources_without_the_skeleton() {
    let e = Env::new();

    let v = e.json(&[], &["--no-inject", "strategies", "--json"]);
    assert_eq!(
        slot(&v, "context.sources")["name"],
        serde_json::json!(["memory-index", "recall", "skills"])
    );
    assert_eq!(slot(&v, "context.sources")["origin"], "flag");

    let v = e.json(&[("HONE_NO_INJECTION", "1")], &["strategies", "--json"]);
    assert_eq!(
        slot(&v, "context.sources")["origin"],
        "env:HONE_NO_INJECTION"
    );
}

#[test]
fn test_s10_phase_compaction_env_var_should_show_in_the_compactor_params() {
    let e = Env::new();
    let v = e.json(&[("HONE_PHASE_COMPACTION", "1")], &["strategies", "--json"]);
    assert_eq!(slot(&v, "context.compactor")["params"]["phase"], true);
}

#[test]
fn test_s10_ignored_project_values_should_appear_as_warnings_naming_the_file() {
    let e = Env::new();
    let project = e.project_config("[memory]\ntop_k = 9\n");

    let v = e.json(&[], &["strategies", "--json"]);

    assert_eq!(
        slot(&v, "memory.recaller")["params"]["top_k"],
        5,
        "a project cannot raise top_k"
    );
    let w = v["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0]["path"], path_str(&project));
    assert!(w[0]["message"].as_str().unwrap().contains("top_k"));

    let out = e.run(&[], &["strategies"]);
    assert!(
        text(&out.stdout).contains(&path_str(&project)),
        "warnings are printed in text mode too"
    );
}

#[test]
fn test_s10_check_should_print_nothing_and_exit_zero_when_valid() {
    let e = Env::new();
    e.user_config("[context.skeleton]\nuse = \"flat\"\n");

    let out = e.run(&[], &["strategies", "--check"]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
}

#[test]
fn test_s10_check_should_exit_two_on_an_invalid_config_naming_the_file_and_key() {
    for (raw, key) in [
        ("[context]\nbogus = 1\n", "bogus"),
        (
            "[context.skeleton]\nuse = \"bogus\"\n",
            "context.skeleton.use",
        ),
        ("[memory]\nrecaller = \"vector-db\"\n", "memory.recaller"),
    ] {
        for in_project in [false, true] {
            let e = Env::new();
            let path = if in_project {
                e.project_config(raw)
            } else {
                e.user_config(raw)
            };

            let out = e.run(&[], &["strategies", "--check"]);

            assert_eq!(out.status.code(), Some(2), "{raw} (project: {in_project})");
            let err = text(&out.stderr);
            assert!(err.contains(&path_str(&path)), "the file is named: {err}");
            assert!(err.contains(key), "{key} named: {err}");
            assert!(out.stdout.is_empty());
        }
    }
}

#[test]
fn test_s10_plain_strategies_should_exit_two_too_on_an_invalid_config() {
    let e = Env::new();
    let path = e.user_config("[context.skeleton]\nuse = \"bogus\"\n");
    for args in [&["strategies"][..], &["strategies", "--json"][..]] {
        let out = e.run(&[], args);
        assert_eq!(out.status.code(), Some(2));
        assert!(
            text(&out.stderr).contains(&path_str(&path)),
            "the config error, not an unknown-subcommand error: {}",
            text(&out.stderr)
        );
        assert!(
            text(&out.stderr).contains("context.skeleton.use")
                && text(&out.stderr).contains("adaptive"),
            "the key and the valid names: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn test_s4_an_unknown_env_value_should_exit_two_naming_the_variable() {
    let e = Env::new();

    let out = e.run(
        &[("HONE_SKELETON_STRATEGY", "bogus")],
        &["strategies", "--check"],
    );

    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(
        err.contains("HONE_SKELETON_STRATEGY") && err.contains("bogus"),
        "{err}"
    );
}

#[test]
fn test_s4_an_unknown_flag_value_should_exit_two_naming_the_flag() {
    let e = Env::new();

    let out = e.run(
        &[],
        &["--skeleton-strategy", "bogus", "strategies", "--check"],
    );

    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(
        err.contains("--skeleton-strategy") && err.contains("bogus"),
        "{err}"
    );
}
