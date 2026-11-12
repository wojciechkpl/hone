#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F13, F17, F20 and the CLI side of F22 of `docs/plans/eval-profiles-spec.md` (review
//! round 1), through the `hone` binary with a temp HOME and an empty environment. No model
//! is called: the Ollama endpoint points at a closed port and every case here stops before
//! an agent is built.
//!
//! Contract pinned:
//! - F13: `claude-cli:*` and `gemini-cli:*` models are rejected, with and without `--check`,
//!   exit 2, a message naming the model's prefix; the CLI program is never launched.
//! - F17: `--check` resolves profiles over the user's config (an invalid user value exits 2
//!   naming the file and value), and requires a model (flag, tasks file, or configured
//!   default): none -> exit 2 mentioning `model`. A run in which every outcome is
//!   `infra_error` (here: the copy of the task directory fails on an unreadable subdirectory)
//!   exits with a non-zero code (the report is still written).
//! - F20: config values echoed in errors are escaped: no control byte but `\n` on stderr.
//! - F22: `--repeats 0` exits 2 naming `repeats`; `--profile` twice with `--ablate` exits 2
//!   mentioning `ablate`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    dir: PathBuf,
}

impl Env {
    fn new(user_config: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (home, dir) = (root.join("home"), root.join("evals"));
        std::fs::create_dir_all(home.join(".config/hone")).unwrap();
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        std::fs::write(
            home.join(".config/hone/config.toml"),
            format!("{user_config}\n"),
        )
        .unwrap();
        Self { _tmp: tmp, root, home, dir }
    }

    fn with_ollama() -> Self {
        Self::new("[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n")
    }

    fn tasks(&self, raw: &str) -> PathBuf {
        let p = self.dir.join("tasks.toml");
        std::fs::write(&p, raw).unwrap();
        p
    }

    fn run(&self, path_prefix: Option<&Path>, tasks: &Path, args: &[&str]) -> Output {
        let path = match path_prefix {
            Some(p) => format!("{}:/usr/bin:/bin", p.display()),
            None => "/usr/bin:/bin".to_owned(),
        };
        Command::new(env!("CARGO_BIN_EXE_hone"))
            .arg("eval")
            .arg(tasks)
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", path)
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

const VALID: &str = r#"
model = "ollama:qwen"

[[tasks]]
id = "t1"
prompt = "p"
success_check = "true"
working_dir = "proj"

[profiles.lean.context]
compactor = "trim-only"
"#;

// ---- F13 ---------------------------------------------------------------------------------------

fn fake_cli_dir(e: &Env) -> (PathBuf, PathBuf) {
    let bin = e.root.join("fakebin");
    let marker = e.root.join("cli-ran.marker");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["claude", "gemini"] {
        let p = bin.join(name);
        std::fs::write(&p, format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display())).unwrap();
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    (bin, marker)
}

#[test]
fn test_f13_agent_cli_models_should_be_refused_with_and_without_check_and_never_launched() {
    for model in ["claude-cli:x", "gemini-cli:x"] {
        for check in [true, false] {
            let e = Env::with_ollama();
            let (bin, marker) = fake_cli_dir(&e);
            let t = e.tasks(&VALID.replace("ollama:qwen", model));
            let mut args = vec!["-o", "out"];
            if check {
                args.push("--check");
            }

            let out = e.run(Some(&bin), &t, &args);

            assert_eq!(out.status.code(), Some(2), "{model} check={check}: {}", text(&out.stderr));
            let prefix = model.split(':').next().unwrap();
            assert!(text(&out.stderr).contains(prefix), "{model}: {}", text(&out.stderr));
            assert!(!marker.exists(), "{model} check={check}: the CLI program must never run");
        }
    }
}

#[test]
fn test_f13_the_model_flag_should_be_refused_too() {
    let e = Env::with_ollama();
    let (bin, marker) = fake_cli_dir(&e);
    let t = e.tasks(VALID);

    let out = e.run(Some(&bin), &t, &["--check", "--model", "claude-cli:x"]);

    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(!marker.exists());
}

// ---- F17 ---------------------------------------------------------------------------------------

#[test]
fn test_f17_check_should_report_an_invalid_value_in_the_users_config() {
    let e = Env::new("[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n\n[context.skeleton]\nuse = \"bogus-skeleton\"\n");
    let t = e.tasks(VALID);

    let out = e.run(None, &t, &["--check"]);

    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stdout));
    let err = text(&out.stderr);
    assert!(err.contains("bogus-skeleton") && err.contains("config.toml"), "{err}");
}

#[test]
fn test_f17_check_should_require_a_model() {
    let e = Env::new("");
    let t = e.tasks(&VALID.replace("model = \"ollama:qwen\"\n", ""));

    let out = e.run(None, &t, &["--check"]);

    assert_eq!(out.status.code(), Some(2), "stdout: {}", text(&out.stdout));
    assert!(text(&out.stderr).to_lowercase().contains("model"), "{}", text(&out.stderr));
}

#[test]
fn test_f17_a_run_where_every_outcome_is_infra_error_should_exit_non_zero() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::with_ollama();
    // An unreadable subdirectory: the file checks pass, the copy of every run fails
    // (`infra_error`) before any agent is built, so no model is involved.
    let secret = e.dir.join("proj/locked");
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
    let t = e.tasks(
        "model = \"ollama:qwen\"\n\n[[tasks]]\nid = \"t1\"\nprompt = \"p\"\nsuccess_check = \"true\"\nworking_dir = \"proj\"\n\n[[tasks]]\nid = \"t2\"\nprompt = \"p\"\nsuccess_check = \"true\"\nworking_dir = \"proj\"\n",
    );

    let out = e.run(None, &t, &["-o", "out"]);
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o755)).unwrap();

    let code = out.status.code();
    assert!(code.is_some_and(|c| c != 0), "exit {code:?}: {}", text(&out.stdout));
    let runs: Vec<PathBuf> = std::fs::read_dir(e.dir.join("out")).unwrap().map(|d| d.unwrap().path()).collect();
    assert_eq!(runs.len(), 1, "the run happened: {}", text(&out.stderr));
    assert!(runs[0].join("report.json").is_file(), "the report is still written");
    let o: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(runs[0].join("default/t1/r1/outcome.json")).unwrap()).unwrap();
    assert_eq!(o["status"], "infra_error", "harness: this is an infra_error run: {o}");
}

// ---- F20 ---------------------------------------------------------------------------------------

#[test]
fn test_f20_an_escape_laden_profile_value_should_not_reach_stderr_raw() {
    let e = Env::with_ollama();
    let t = e.tasks(&format!(
        "{VALID}\n[profiles.odd.context.skeleton]\nuse = \"bad\\u001b]0;PWNED\\u0007\\u001b[2Kvalue\\r\\nforged\"\n"
    ));

    let out = e.run(None, &t, &["--check"]);

    assert_eq!(out.status.code(), Some(2));
    let bad: Vec<u8> = out.stderr.iter().copied().filter(|b| (*b < 0x20 && *b != b'\n') || *b == 0x7f).collect();
    assert!(bad.is_empty(), "raw control bytes {bad:?} in {:?}", text(&out.stderr));
    assert!(text(&out.stderr).contains("PWNED"), "the value is still shown, escaped: {}", text(&out.stderr));
    assert!(!text(&out.stderr).lines().any(|l| l.starts_with("forged")), "no forged line");
}

// ---- F22 ---------------------------------------------------------------------------------------

#[test]
fn test_f22_repeats_zero_should_exit_2_naming_repeats() {
    let e = Env::with_ollama();
    let t = e.tasks(VALID);

    let out = e.run(None, &t, &["--check", "--repeats", "0"]);

    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stdout));
    assert!(text(&out.stderr).contains("repeats"), "{}", text(&out.stderr));
}

#[test]
fn test_f22_two_profiles_with_ablate_should_exit_2() {
    let e = Env::with_ollama();
    let t = e.tasks(VALID);

    let out = e.run(None, &t, &["--check", "--profile", "lean", "--profile", "default", "--ablate", "context.skeleton"]);

    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stdout));
    assert!(text(&out.stderr).contains("ablate"), "{}", text(&out.stderr));
}
