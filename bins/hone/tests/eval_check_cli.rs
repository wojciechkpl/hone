#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F6 of `docs/plans/eval-profiles-spec.md`: `hone eval <tasks.toml> --check` validates the
//! tasks file, every profile, every `working_dir` and `memory_fixture`, and that the
//! model's provider is configured, without any model call. It prints a summary line
//! containing `<runs> runs` (tasks x profiles x repeats) and exits 0, or prints the errors
//! on stderr and exits 2. Flags used: `--check`, `--repeats N`, `--profile NAME`.
//!
//! The binary runs with a temp HOME and an empty environment. The Ollama endpoint in the
//! user config points at a closed port, so a build without `--check` (the red state)
//! can't reach a real model.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    dir: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (home, dir) = (root.join("home"), root.join("evals"));
        std::fs::create_dir_all(home.join(".config/hone")).unwrap();
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        std::fs::create_dir_all(dir.join("fixtures")).unwrap();
        std::fs::write(dir.join("fixtures/m.jsonl"), "").unwrap();
        std::fs::write(
            home.join(".config/hone/config.toml"),
            "[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        Self { _tmp: tmp, home, dir }
    }

    fn tasks(&self, raw: &str) -> PathBuf {
        let p = self.dir.join("tasks.toml");
        std::fs::write(&p, raw).unwrap();
        p
    }

    fn check(&self, tasks: &Path, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hone"))
            .arg("eval")
            .arg(tasks)
            .arg("--check")
            .args(extra)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

const VALID: &str = r#"
model = "ollama:qwen"

[[tasks]]
id = "t1"
prompt = "p"
success_check = "true"
working_dir = "proj"
memory_fixture = "fixtures/m.jsonl"

[[tasks]]
id = "t2"
prompt = "p"
success_check = "true"
working_dir = "proj"

[profiles.lean.context]
compactor = "trim-only"
"#;

#[test]
fn test_f6_check_should_exit_0_and_summarise_tasks_profiles_and_repeats() {
    let e = Env::new();
    let t = e.tasks(VALID);

    let out = e.check(&t, &["--repeats", "3"]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    let so = text(&out.stdout);
    assert!(
        so.contains("2 tasks") && so.contains("2 profiles") && so.contains("3 repeats"),
        "{so}"
    );
    assert!(so.contains("12 runs"), "{so}");
}

#[test]
fn test_f6_check_should_honour_profile_selection_in_the_run_count() {
    let e = Env::new();
    let t = e.tasks(VALID);

    let out = e.check(&t, &["--profile", "lean"]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    assert!(text(&out.stdout).contains("2 runs"), "{}", text(&out.stdout));
}

#[test]
fn test_f6_an_unknown_profile_key_should_exit_2_naming_the_file_and_the_profile() {
    let e = Env::new();
    let t = e.tasks(&format!("{VALID}\n[profiles.odd.context]\ncolour = \"red\"\n"));

    let out = e.check(&t, &[]);

    assert_eq!(out.status.code(), Some(2));
    let se = text(&out.stderr);
    assert!(
        se.contains("tasks.toml") && se.contains("odd") && se.contains("colour"),
        "{se}"
    );
}

#[test]
fn test_f6_a_missing_working_dir_or_fixture_should_exit_2_naming_it() {
    let e = Env::new();
    let t = e.tasks(&VALID.replace(
        "working_dir = \"proj\"\nmemory",
        "working_dir = \"nowhere\"\nmemory",
    ));
    let out = e.check(&t, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("nowhere"), "{}", text(&out.stderr));

    let t = e.tasks(&VALID.replace("fixtures/m.jsonl", "fixtures/gone.jsonl"));
    let out = e.check(&t, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("gone.jsonl"), "{}", text(&out.stderr));
}

#[test]
fn test_f6_a_model_whose_provider_is_not_configured_should_exit_2_naming_the_provider() {
    let e = Env::new();
    let t = e.tasks(&VALID.replace("ollama:qwen", "openai:gpt-4o"));

    let out = e.check(&t, &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("openai"), "{}", text(&out.stderr));
}

#[test]
fn test_f6_check_should_report_every_error_not_just_the_first() {
    let e = Env::new();
    let t = e.tasks(
        &VALID
            .replace("fixtures/m.jsonl", "fixtures/gone.jsonl")
            .replace("ollama:qwen", "openai:gpt-4o"),
    );

    let out = e.check(&t, &[]);

    assert_eq!(out.status.code(), Some(2));
    let se = text(&out.stderr);
    assert!(se.contains("gone.jsonl") && se.contains("openai"), "{se}");
}
