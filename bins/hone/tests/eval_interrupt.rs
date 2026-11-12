#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F14 of `docs/plans/eval-profiles-spec.md` (review round 1): SIGINT / SIGTERM to a real
//! `hone eval` process whose agent is running a long shell command.
//!
//! Setup (no network): a tiny Ollama-protocol server on 127.0.0.1 answers EVERY model
//! request with a `shell` tool call `sleep <N>` (311 for the SIGINT test, 312 for SIGTERM, so
//! the two tests can run in parallel), so the agent of the single task sits in
//! that command. `TMPDIR` points at a test directory, so the run's working-directory copy
//! (`$TMPDIR/hone-eval-*`) is observable. The tests wait (polling, up to 60 s) until the
//! `sleep 311` process exists, signal the `hone` process, and wait up to 60 s for it to exit.
//!
//! Contract: exit 130 (SIGINT) / 143 (SIGTERM); no `sleep 311` process remains; no
//! `hone-eval-*` entry remains in `TMPDIR`; the run directory holds `report.json` and
//! `report.md`; `manifest.json` has `interrupted: true` and a string `ended_at`; the
//! unfinished run's `outcome.json` has `status: "interrupted"`.
//!
//! Timing: polling makes this as deterministic as a signal test can be; it can still fail on
//! a heavily loaded machine (the 60 s waits are generous).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn serve(mut stream: TcpStream, secs: u32) {
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
        json!({"model":"m","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"shell","arguments":{"command":format!("sleep {secs}")}}}]},"done":false}),
        json!({"model":"m","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":3,"eval_count":2}),
    );
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn mock_server(secs: u32) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            std::thread::spawn(move || serve(s, secs));
        }
    });
    url
}

fn sleeping(secs: u32) -> bool {
    Command::new("pgrep")
        .args(["-f", &format!("sleep {secs}")])
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Setup {
    secs: u32,
    _tmp: tempfile::TempDir,
    root: PathBuf,
    tmpdir: PathBuf,
    out: PathBuf,
    child: std::process::Child,
}

fn start(secs: u32) -> Setup {
    assert!(!sleeping(secs), "harness: no stray `sleep {secs}` before the test");
    let url = mock_server(secs);
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, dir, tmpdir, out) = (root.join("home"), root.join("evals"), root.join("tmpdir"), root.join("out"));
    for d in [home.join(".config/hone"), dir.join("proj"), tmpdir.clone()] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        home.join(".config/hone/config.toml"),
        format!("[providers.ollama]\nbase_url = \"{url}\"\n"),
    )
    .unwrap();
    std::fs::write(dir.join("proj/data.txt"), "data\n").unwrap();
    std::fs::write(
        dir.join("tasks.toml"),
        "model = \"ollama:m\"\n\n[[tasks]]\nid = \"t\"\nprompt = \"sleep\"\nsuccess_check = \"true\"\nworking_dir = \"proj\"\ntimeout_secs = 600\n",
    )
    .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_hone"))
        .arg("eval")
        .arg(dir.join("tasks.toml"))
        .args(["-o"])
        .arg(&out)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", &tmpdir)
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    Setup { secs, _tmp: tmp, root, tmpdir, out, child }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Never leave the test's sleep behind, whatever failed.
        let _ = Command::new("pkill").args(["-f", &format!("sleep {}", self.secs)]).status();
    }
}

fn copies_in(tmpdir: &Path) -> Vec<String> {
    std::fs::read_dir(tmpdir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("hone-eval-"))
        .collect()
}

fn interrupt(signal: &str, expected_code: i32, secs: u32) {
    let mut s = start(secs);
    wait_for("the agent's sleep", || sleeping(secs));
    assert!(!copies_in(&s.tmpdir).is_empty(), "harness: the working-directory copy exists while the run is live");

    let kill = Command::new("kill").arg(format!("-{signal}")).arg(s.child.id().to_string()).status().unwrap();
    assert!(kill.success());
    let mut status = None;
    wait_for("hone eval to exit", || {
        status = s.child.try_wait().unwrap();
        status.is_some()
    });

    assert_eq!(status.unwrap().code(), Some(expected_code), "exit code for {signal}");
    wait_for("the sleep to be gone", || !sleeping(secs));
    assert!(!sleeping(secs), "no process of the run remains");
    assert_eq!(copies_in(&s.tmpdir), Vec::<String>::new(), "no working-directory copy remains");

    let runs: Vec<PathBuf> = std::fs::read_dir(&s.out).unwrap().map(|d| d.unwrap().path()).collect();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let run = &runs[0];
    assert!(run.join("report.json").is_file() && run.join("report.md").is_file(), "reports for what finished");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(run.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["interrupted"], true, "{manifest}");
    assert!(manifest["ended_at"].is_string(), "{manifest}");
    let outcome: Value = serde_json::from_str(&std::fs::read_to_string(run.join("default/t/r1/outcome.json")).unwrap()).unwrap();
    assert_eq!(outcome["status"], "interrupted", "{outcome}");
    let _ = &s.root;
}

#[test]
fn test_f14_sigint_should_clean_up_and_exit_130() {
    interrupt("INT", 130, 311);
}

#[test]
fn test_f14_sigterm_should_clean_up_and_exit_143() {
    interrupt("TERM", 143, 312);
}
