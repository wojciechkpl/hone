#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! S20 of `docs/plans/strategy-registry-spec.md`: `honed` prints the warnings of
//! its strategy selection (values of the project's `.hone/config.toml` that were
//! ignored) to stderr at startup, naming the file, whatever `RUST_LOG` says
//! (here: unset, so the tracing subscriber would print nothing below `error`).
//!
//! The strategies are resolved when the worker pool is configured, so the test runs
//! `honed` in worker mode (`HONE_WORKER_COORDINATOR` pointing at a closed port and a
//! placeholder API key; the pool just fails to reach the coordinator).

use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Honed {
    child: Child,
}

impl Drop for Honed {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn test_s20_honed_should_print_config_warnings_to_stderr_without_rust_log() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, wd) = (root.join("home"), root.join("wd"));
    std::fs::create_dir_all(home.join(".config/hone")).unwrap();
    std::fs::create_dir_all(wd.join(".hone")).unwrap();
    let project = wd.join(".hone/config.toml");
    // Above the default of 5: ignored with a warning (a project file can only lower it).
    std::fs::write(&project, "[memory]\ntop_k = 9\n").unwrap();
    let err_path = root.join("honed.err");
    let port = free_port();

    let mut h = Honed {
        child: Command::new(env!("CARGO_BIN_EXE_honed"))
            .arg(port.to_string())
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin:/usr/local/bin")
            .env("HONE_EPHEMERAL", "1")
            .env("HONE_BIND", "127.0.0.1")
            .env("NO_COLOR", "1")
            .env("HONE_DISTRIBUTED_DB", root.join("dist.db"))
            .env("HONE_WORKER_COORDINATOR", "http://127.0.0.1:9")
            .env("OPENAI_API_KEY", "placeholder-value")
            .current_dir(&wd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&err_path).unwrap())
            .spawn()
            .expect("spawn honed"),
    };

    let deadline = Instant::now() + Duration::from_secs(20);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        if let Some(s) = h.child.try_wait().unwrap() {
            panic!(
                "honed exited early: {s:?}; stderr: {}",
                std::fs::read_to_string(&err_path).unwrap_or_default()
            );
        }
        assert!(Instant::now() < deadline, "honed never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    let _ = h.child.kill();
    let _ = h.child.wait();

    let err = std::fs::read_to_string(&err_path).unwrap();
    assert!(
        err.contains(&project.display().to_string()),
        "the warning names the project file:\n{err}"
    );
    assert!(err.contains("top_k"), "{err}");
}
