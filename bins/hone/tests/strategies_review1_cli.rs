#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Review round 1 of `docs/plans/strategy-registry-spec.md` through the `hone`
//! binary (temp HOME and project directory, no real model):
//!
//! - S12: an escape-laden invalid value in a project file never reaches stderr raw,
//!   from `hone "hi"` and from `hone strategies --check`;
//! - S15: an unknown `exclude_kinds` entry exits 2 naming file, key and valid kinds
//!   before any MCP server starts or any model call is made;
//! - S16: `hone strategies` shows every parameter's origin and each slot's registry
//!   origin, and prints floats as written.
//!
//! JSON shape assumed for S16 (additions to the S10 contract): every slot object gains
//! `"registry_origin": "builtin"` and `"param_origins": { "<param>": "<origin>" }` where
//! the origin label is `default`, `flag`, `env:<NAME>` or `file:<path>`; params are
//! `max_tokens`, `gap_threshold` (skeleton), `phase`, `budget_ratio` (compactor),
//! `top_k`, `exclude_kinds` (recaller).

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

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

    /// `base_url` is where the (dummy) OpenAI-compatible provider is said to live.
    fn user_config(&self, base_url: &str, raw: &str) -> PathBuf {
        let p = self.home.join(".config/hone/config.toml");
        let head = format!(
            "default_model = \"openai:dummy-model\"\n[providers.openai]\nbase_url = \"{base_url}\"\napi_key_env = \"HONE_TEST_PROVIDER_VAR\"\n\n"
        );
        // `[providers.*]` must precede the strategy tables, so the head goes first.
        std::fs::write(&p, format!("{head}{raw}")).unwrap();
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
            .env("PATH", "/usr/bin:/bin:/usr/local/bin")
            .env("HONE_TEST_PROVIDER_VAR", "placeholder-value")
            .current_dir(&self.project)
            .stdin(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

fn slot<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["slots"]
        .as_array()
        .expect("slots array")
        .iter()
        .find(|s| s["slot"] == name)
        .unwrap_or_else(|| panic!("no slot {name} in {v}"))
}

// ---- S12 ---------------------------------------------------------------------------------------------------------

#[test]
fn test_s12_an_escape_laden_project_value_should_not_reach_stderr_raw() {
    let evil = format!(
        "\\u001b]0;x\\u0007\\u001b[2K\\r\\nhone: warning: forged line {}",
        "A".repeat(200)
    );
    for args in [&["hi"][..], &["strategies", "--check"][..], &["strategies"][..]] {
        let e = Env::new();
        e.user_config("http://127.0.0.1:9", "");
        let project = e.project_config(&format!("[memory]\nrecaller = \"{evil}\"\n"));

        let out = e.run(&[], args);

        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out.stderr));
        for (name, byte) in [("ESC", 0x1b_u8), ("BEL", 0x07), ("CR", b'\r')] {
            assert!(
                !out.stderr.contains(&byte),
                "{args:?}: raw {name} in stderr: {:?}",
                text(&out.stderr)
            );
        }
        let err = text(&out.stderr);
        assert!(
            !err.lines().any(|l| l.starts_with("hone: warning: forged")),
            "{args:?}: a forged line: {err:?}"
        );
        assert!(err.contains(&path_str(&project)), "{args:?}: the file is named: {err}");
        assert!(err.contains("memory.recaller"), "{args:?}: {err}");
        assert!(
            !err.contains(&"A".repeat(81)),
            "{args:?}: the value is cut to 80 characters: {err}"
        );
    }
}

// ---- S15 -----------------------------------------------------------------------------------------------------------

/// Answers the MCP handshake, then ignores the end of its input. argv[1] = pid file.
const STUBBORN: &str = r#"
import json, os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
while True:
    line = sys.stdin.readline()
    if not line:
        time.sleep(30)
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    result = {"tools": []} if req.get("method") == "tools/list" else {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

fn alive(pid: &str) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

#[test]
fn test_s15_an_unknown_exclude_kind_should_exit_two_before_any_server_or_model_call() {
    for in_project in [false, true] {
        let e = Env::new();
        // A listener standing in for the model: any request would connect to it.
        let model = TcpListener::bind("127.0.0.1:0").unwrap();
        model.set_nonblocking(true).unwrap();
        let base = format!("http://{}", model.local_addr().unwrap());
        let raw = "[memory]\nexclude_kinds = [\"outcome\", \"bogus\"]\n";
        let cfg = if in_project {
            e.user_config(&base, "");
            e.project_config(raw)
        } else {
            e.user_config(&base, raw)
        };
        let pid_file = e.project.join("server.pid");
        let script = e.project.join("stubborn.py");
        std::fs::write(&script, STUBBORN).unwrap();
        std::fs::write(
            e.project.join(".hone/mcp.json"),
            json!({ "mcpServers": { "stubborn": {
                "command": "python3",
                "args": [script.to_str().unwrap(), pid_file.to_str().unwrap()]
            } } })
            .to_string(),
        )
        .unwrap();

        let started = Instant::now();
        let out = e.run(&[], &["hi"]);

        assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
        let err = text(&out.stderr);
        assert!(err.contains(&path_str(&cfg)), "the file is named: {err}");
        assert!(err.contains("memory.exclude_kinds"), "the key is named: {err}");
        assert!(err.contains("bogus"), "{err}");
        for kind in ["conversation", "user", "feedback", "project", "reference", "outcome"] {
            assert!(err.contains(kind), "valid kind {kind} listed: {err}");
        }
        assert!(
            model.accept().is_err(),
            "no model call was made before the config error"
        );
        // The MCP server must not have been started (or, if it was, must be gone).
        std::thread::sleep(Duration::from_millis(300));
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            let pid = pid.trim().to_owned();
            let gone = (0..20).any(|_| {
                std::thread::sleep(Duration::from_millis(100));
                !alive(&pid)
            });
            let _ = Command::new("kill").args(["-9", &pid]).stderr(Stdio::null()).status();
            panic!("an MCP server was started before the config error (gone: {gone})");
        }
        assert!(started.elapsed() < Duration::from_secs(30));
    }
}

#[test]
fn test_s15_strategies_check_should_reject_an_unknown_exclude_kind() {
    let e = Env::new();
    let cfg = e.user_config("http://127.0.0.1:9", "[memory]\nexclude_kinds = [\"bogus\"]\n");

    let out = e.run(&[], &["strategies", "--check"]);

    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(err.contains(&path_str(&cfg)) && err.contains("memory.exclude_kinds"), "{err}");
}

// ---- S16 -----------------------------------------------------------------------------------------------------------

fn s16_env() -> (Env, PathBuf, PathBuf) {
    let e = Env::new();
    let user = e.user_config("http://127.0.0.1:9", "[context]\nbudget_ratio = 0.6\n");
    let project = e.project_config(
        "[context.skeleton]\nmax_tokens = 1000\ngap_threshold = 0.9\n\n[memory]\ntop_k = 3\n",
    );
    (e, user, project)
}

#[test]
fn test_s16_json_should_show_every_parameters_origin() {
    let (e, user, project) = s16_env();

    let out = e.run(&[("HONE_PHASE_COMPACTION", "1")], &["strategies", "--json"]);

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(v["warnings"], json!([]), "these project values only narrow");
    let p = |s: &str| format!("file:{}", path_str(if s == "user" { &user } else { &project }));

    let skeleton = &slot(&v, "context.skeleton")["param_origins"];
    assert_eq!(skeleton["max_tokens"], p("project"));
    assert_eq!(skeleton["gap_threshold"], p("project"));
    let compactor = &slot(&v, "context.compactor")["param_origins"];
    assert_eq!(compactor["phase"], "env:HONE_PHASE_COMPACTION");
    assert_eq!(compactor["budget_ratio"], p("user"));
    let recaller = &slot(&v, "memory.recaller")["param_origins"];
    assert_eq!(recaller["top_k"], p("project"));
    assert_eq!(recaller["exclude_kinds"], "default");
}

#[test]
fn test_s16_json_should_show_each_slots_registry_origin() {
    let (e, _, _) = s16_env();

    let out = e.run(&[], &["strategies", "--json"]);

    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    for name in [
        "context.sources",
        "context.skeleton",
        "context.compactor",
        "memory.recaller",
        "memory.writer",
    ] {
        assert_eq!(slot(&v, name)["registry_origin"], "builtin", "{name}");
    }
}

#[test]
fn test_s16_floats_should_print_as_written() {
    let (e, _, _) = s16_env();

    let out = e.run(&[], &["strategies", "--json"]);

    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(slot(&v, "context.skeleton")["params"]["gap_threshold"], json!(0.9));
    assert_eq!(slot(&v, "context.compactor")["params"]["budget_ratio"], json!(0.6));
    let raw = text(&out.stdout);
    assert!(!raw.contains("0.9000"), "{raw}");
    assert!(!raw.contains("0.6000"), "{raw}");

    let defaults = Env::new();
    let out = defaults.run(&[], &["strategies", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(slot(&v, "context.skeleton")["params"]["gap_threshold"], json!(0.4));
}

#[test]
fn test_s16_text_should_show_parameter_origins_and_floats_as_written() {
    let (e, _, project) = s16_env();

    let out = e.run(&[], &["strategies"]);

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let s = text(&out.stdout);
    assert!(s.contains("0.9") && !s.contains("0.9000"), "{s}");
    assert!(s.contains("0.6") && !s.contains("0.6000"), "{s}");
    assert!(
        s.contains(&path_str(&project)),
        "the file that chose the project's parameters is shown (not only each slot's name):\n{s}"
    );
    assert!(s.contains("builtin"), "the registry origin: {s}");
}
