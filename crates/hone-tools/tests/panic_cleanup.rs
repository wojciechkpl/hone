//! C1-C4 of `docs/plans/panic-stops-processes-spec.md`: a panic that ends hone
//! stops the registered process groups; a panic that does not end hone stops
//! nothing.
//!
//! `harness = false` (see `Cargo.toml`): the panic must happen on a real main
//! thread, so this binary re-executes itself. With no `--child-*` argument it
//! is the parent: it runs each scenario in a child process, then checks exit
//! status, stderr, marker files and that every pid is gone, all with bounded
//! waits. Whatever the outcome, every pid a scenario recorded is SIGKILLed at
//! the end, so a failing run leaks nothing.
//!
//! API this file requires (new; the file fails to compile until it exists):
//! - `hone_tools::install_panic_cleanup()`: installs a panic hook that chains
//!   the previous hook (runs it FIRST), then, if `panic_ends_process()`, calls
//!   `stop_groups_on_panic(Duration::from_millis(100))`.
//! - `hone_tools::panic_ends_process() -> bool`: `cfg!(panic = "abort")` or the
//!   current thread is the main thread.
//! - `hone_tools::stop_groups_on_panic(lock_wait: Duration) -> bool`: the hook's
//!   body. Waits at most `lock_wait` for the group registry; if it cannot get it
//!   returns `false` and stops nothing (and does not latch: a later call may
//!   still run). Otherwise it does what `kill_running_commands()` does (command
//!   groups SIGKILL at once, MCP groups SIGTERM then SIGKILL after ~1 s) and
//!   returns `true`. Callable from several threads at once: the cleanup runs
//!   once, and every other caller returns `true` only after it has finished.

#[cfg(not(unix))]
fn main() {}

#[cfg(unix)]
fn main() {
    unix_tests::main();
}

#[cfg(unix)]
mod unix_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use std::os::unix::process::CommandExt;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Barrier, OnceLock};
    use std::time::{Duration, Instant};

    /// argv[1] = name, argv[2] = directory, argv[3] = mode.
    /// The server installs its SIGTERM behaviour, then writes `<name>.pid`.
    /// `stubborn` ignores SIGTERM; `polite` takes ~300 ms to clean up, writes
    /// `<name>.term` and exits.
    const SERVER: &str = r#"
import os, signal, sys, time
name, d, mode = sys.argv[1], sys.argv[2], sys.argv[3]
if mode == "polite":
    def bye(*a):
        time.sleep(0.3)
        open(os.path.join(d, name + ".term"), "w").write("x")
        sys.exit(0)
    signal.signal(signal.SIGTERM, bye)
else:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
open(os.path.join(d, name + ".pid"), "w").write(str(os.getpid()))
while True:
    time.sleep(1)
"#;

    const BOUND: Duration = Duration::from_secs(20);

    // ---- shared helpers -------------------------------------------------

    fn alive(pid: &str) -> bool {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .expect("ps");
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        !stat.is_empty() && !stat.starts_with('Z')
    }

    fn wait_for_file(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(s) = std::fs::read_to_string(path) {
                if !s.trim().is_empty() {
                    return s.trim().to_owned();
                }
            }
            assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn read(dir: &Path, name: &str) -> Option<String> {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .map(|s| s.trim().to_owned())
    }

    // ---- child side -----------------------------------------------------

    /// Start a scripted server in its own group, tracked in the registry.
    fn start_server(dir: &Path, name: &str, mode: &str) -> (hone_tools::ProcessGroupGuard, String) {
        let script = dir.join("server.py");
        std::fs::write(&script, SERVER).unwrap();
        let child = Command::new("python3")
            .arg(&script)
            .args([name, dir.to_str().unwrap(), mode])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3");
        let guard = hone_tools::ProcessGroupGuard::new(child.id());
        let pid = wait_for_file(&dir.join(format!("{name}.pid")));
        (guard, pid)
    }

    /// A shell-command group, registered by `run_command` on a helper thread.
    /// Returns the sleeper's pid.
    fn start_command(dir: &Path) -> String {
        let pidfile = dir.join("cmd.pid");
        let script = format!("echo $$ > '{}'; exec sleep 300", pidfile.display());
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let mut cmd = Command::new("sh");
            cmd.args(["-c", &script]);
            let _ = rt.block_on(hone_tools::run_command(cmd, None));
        });
        wait_for_file(&dir.join("cmd.pid"))
    }

    static DIR: OnceLock<PathBuf> = OnceLock::new();

    /// Start the three process groups and leave them registered even if this
    /// thread unwinds (no guard drop may do the hook's work).
    fn forget_all(dir: &Path) {
        let (g1, _) = start_server(dir, "stubborn", "stubborn");
        let (g2, _) = start_server(dir, "polite", "polite");
        let _ = start_command(dir);
        std::mem::forget((g1, g2));
    }

    fn child(mode: &str, dir: &Path) {
        DIR.set(dir.to_path_buf()).unwrap();
        match mode {
            "main-panic" => {
                hone_tools::install_panic_cleanup();
                forget_all(dir);
                panic!("boom-c1");
            }
            "prev-hook" => {
                // A hook installed BEFORE ours must still run, and run before
                // anything is signalled.
                std::panic::set_hook(Box::new(|info| {
                    eprintln!("CUSTOM-PREV-HOOK {info}");
                    let dir = DIR.get().unwrap();
                    let all = ["stubborn", "polite", "cmd"]
                        .iter()
                        .all(|n| read(dir, &format!("{n}.pid")).is_some_and(|p| alive(&p)));
                    std::fs::write(
                        dir.join("prev-saw-all-alive"),
                        if all { "yes" } else { "no" },
                    )
                    .unwrap();
                }));
                hone_tools::install_panic_cleanup();
                forget_all(dir);
                panic!("boom-c2");
            }
            "thread-panic" => {
                hone_tools::install_panic_cleanup();
                let (g1, p1) = start_server(dir, "stubborn", "stubborn");
                let (g2, p2) = start_server(dir, "polite", "polite");
                let r = std::thread::spawn(|| panic!("boom-c3")).join();
                assert!(r.is_err());
                let ok = alive(&p1) && alive(&p2);
                std::fs::write(dir.join("alive-after"), if ok { "yes" } else { "no" }).unwrap();
                drop((g1, g2)); // the normal exit path kills them
            }
            "task-panic" => {
                hone_tools::install_panic_cleanup();
                let (g1, p1) = start_server(dir, "stubborn", "stubborn");
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .unwrap();
                let err = rt
                    .block_on(async { tokio::spawn(async { panic!("boom-c3-task") }).await })
                    .unwrap_err();
                assert!(err.is_panic());
                let ok = alive(&p1);
                std::fs::write(dir.join("alive-after"), if ok { "yes" } else { "no" }).unwrap();
                drop(g1);
            }
            "rule" => {
                let main_side = hone_tools::panic_ends_process();
                let thread_side = std::thread::spawn(hone_tools::panic_ends_process)
                    .join()
                    .unwrap();
                std::fs::write(
                    dir.join("rule"),
                    format!("main={main_side} thread={thread_side}"),
                )
                .unwrap();
            }
            "concurrent" => {
                let (g1, p1) = start_server(dir, "stubborn", "stubborn");
                let (g2, p2) = start_server(dir, "polite", "polite");
                std::mem::forget((g1, g2));
                let barrier = Arc::new(Barrier::new(4));
                let handles: Vec<_> = (0..4)
                    .map(|i| {
                        let (b, d, p1, p2) =
                            (barrier.clone(), dir.to_path_buf(), p1.clone(), p2.clone());
                        std::thread::spawn(move || {
                            b.wait();
                            let ret = hone_tools::stop_groups_on_panic(Duration::from_millis(100));
                            let line = format!(
                                "{ret} {} {} {}",
                                d.join("polite.term").exists(),
                                !alive(&p2),
                                !alive(&p1),
                            );
                            std::fs::write(d.join(format!("result-{i}")), line).unwrap();
                        })
                    })
                    .collect();
                for h in handles {
                    h.join().unwrap();
                }
            }
            other => panic!("unknown child mode {other}"),
        }
    }

    // ---- parent side ----------------------------------------------------

    struct Run {
        code: Option<i32>,
        stderr: String,
        elapsed: Duration,
        dir: tempfile::TempDir,
        /// (pid, still alive when the child had exited)
        alive_pids: Vec<(String, bool)>,
    }

    impl Run {
        fn file(&self, name: &str) -> Option<String> {
            read(self.dir.path(), name)
        }
        fn pid_alive(&self, name: &str) -> bool {
            let pid = self
                .file(&format!("{name}.pid"))
                .unwrap_or_else(|| panic!("no pid recorded for {name}"));
            self.alive_pids.iter().find(|(p, _)| *p == pid).unwrap().1
        }
    }

    /// Run a child scenario; always kills every recorded pid afterwards.
    fn run_child(mode: &str) -> Run {
        let dir = tempfile::tempdir().unwrap();
        let err_path = dir.path().join("child.stderr");
        let mut c = Command::new(std::env::current_exe().unwrap())
            .args([&format!("--child-{mode}"), dir.path().to_str().unwrap()])
            .env("RUST_BACKTRACE", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&err_path).unwrap())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(s) = c.try_wait().unwrap() {
                break Some(s);
            }
            if start.elapsed() > BOUND {
                let _ = c.kill();
                let _ = c.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let elapsed = start.elapsed();
        let mut alive_pids = Vec::new();
        for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let n = entry.file_name().to_string_lossy().into_owned();
            if n.ends_with(".pid") {
                let pid = std::fs::read_to_string(entry.path())
                    .unwrap()
                    .trim()
                    .to_owned();
                alive_pids.push((pid.clone(), alive(&pid)));
            }
        }
        for (pid, _) in &alive_pids {
            let _ = Command::new("kill")
                .args(["-9", pid])
                .stderr(Stdio::null())
                .status();
        }
        let stderr = std::fs::read_to_string(&err_path).unwrap_or_default();
        assert!(
            status.is_some(),
            "child timed out after {BOUND:?}; stderr: {stderr}"
        );
        Run {
            code: status.unwrap().code(),
            stderr,
            elapsed,
            dir,
            alive_pids,
        }
    }

    // ---- tests ----------------------------------------------------------

    /// C1: a main-thread panic stops command groups at once and MCP groups
    /// with SIGTERM (the polite one gets to clean up) then SIGKILL.
    fn c1_main_thread_panic_should_stop_commands_and_mcp_servers() {
        let r = run_child("main-panic");
        assert!(
            r.file("polite.term").is_some(),
            "polite server was not SIGTERMed first; stderr: {}",
            r.stderr
        );
        assert!(!r.pid_alive("polite"), "polite server gone");
        assert!(!r.pid_alive("stubborn"), "SIGTERM-ignoring server SIGKILLed");
        assert!(!r.pid_alive("cmd"), "shell-command group killed");
    }

    /// C1: the stubborn server costs about the 1 s grace, not a hang.
    fn c1_panic_cleanup_should_finish_within_the_sigterm_grace() {
        let r = run_child("main-panic");
        assert!(
            r.elapsed < Duration::from_secs(8),
            "child took {:?}",
            r.elapsed
        );
        assert!(!r.pid_alive("stubborn"));
    }

    /// C2: message printed, exit code 101 as without the hook.
    fn c2_main_thread_panic_should_still_print_message_and_exit_101() {
        let r = run_child("main-panic");
        assert!(r.stderr.contains("boom-c1"), "stderr: {}", r.stderr);
        assert_eq!(r.code, Some(101), "stderr: {}", r.stderr);
    }

    /// C2: the previously installed hook runs, and before any group is signalled.
    fn c2_previous_hook_should_run_first_and_chain() {
        let r = run_child("prev-hook");
        assert!(r.stderr.contains("CUSTOM-PREV-HOOK"), "stderr: {}", r.stderr);
        assert_eq!(
            r.file("prev-saw-all-alive").as_deref(),
            Some("yes"),
            "previous hook ran after the kill (or not at all)"
        );
        assert_eq!(r.code, Some(101));
        assert!(
            !r.pid_alive("stubborn") && !r.pid_alive("polite") && !r.pid_alive("cmd"),
            "cleanup still ran"
        );
    }

    /// C3: a joined std::thread's panic stops nothing.
    fn c3_joined_thread_panic_should_leave_groups_alone() {
        let r = run_child("thread-panic");
        assert_eq!(
            r.file("alive-after").as_deref(),
            Some("yes"),
            "groups were killed by a thread panic; stderr: {}",
            r.stderr
        );
        assert_eq!(r.code, Some(0), "stderr: {}", r.stderr);
        assert!(
            r.stderr.contains("boom-c3"),
            "previous hook still prints; stderr: {}",
            r.stderr
        );
    }

    /// C3: a tokio task's panic (JoinError) stops nothing.
    fn c3_tokio_task_panic_should_leave_groups_alone() {
        let r = run_child("task-panic");
        assert_eq!(
            r.file("alive-after").as_deref(),
            Some("yes"),
            "stderr: {}",
            r.stderr
        );
        assert_eq!(r.code, Some(0), "stderr: {}", r.stderr);
    }

    /// C1/C3 rule: in an unwinding build only the main thread's panic ends hone.
    fn c1_panic_ends_process_should_be_true_on_main_and_false_on_threads_when_unwinding() {
        let r = run_child("rule");
        assert_eq!(
            r.file("rule").as_deref(),
            Some("main=true thread=false"),
            "stderr: {}",
            r.stderr
        );
    }

    /// C4: simultaneous cleanups run once; every caller returns only after the
    /// SIGTERM grace work is done (marker written, both servers gone).
    fn c4_concurrent_cleanups_should_all_wait_for_the_one_that_runs() {
        let r = run_child("concurrent");
        for i in 0..4 {
            assert_eq!(
                r.file(&format!("result-{i}")).as_deref(),
                Some("true true true true"),
                "caller {i} returned early or without success; stderr: {}",
                r.stderr
            );
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        if let Some(mode) = args.get(1).and_then(|a| a.strip_prefix("--child-")) {
            child(mode, Path::new(&args[2]));
            return;
        }
        let tests: Vec<(&str, fn())> = vec![
            (
                "c1_main_thread_panic_should_stop_commands_and_mcp_servers",
                c1_main_thread_panic_should_stop_commands_and_mcp_servers,
            ),
            (
                "c1_panic_cleanup_should_finish_within_the_sigterm_grace",
                c1_panic_cleanup_should_finish_within_the_sigterm_grace,
            ),
            (
                "c2_main_thread_panic_should_still_print_message_and_exit_101",
                c2_main_thread_panic_should_still_print_message_and_exit_101,
            ),
            (
                "c2_previous_hook_should_run_first_and_chain",
                c2_previous_hook_should_run_first_and_chain,
            ),
            (
                "c3_joined_thread_panic_should_leave_groups_alone",
                c3_joined_thread_panic_should_leave_groups_alone,
            ),
            (
                "c3_tokio_task_panic_should_leave_groups_alone",
                c3_tokio_task_panic_should_leave_groups_alone,
            ),
            (
                "c1_panic_ends_process_should_be_true_on_main_and_false_on_threads_when_unwinding",
                c1_panic_ends_process_should_be_true_on_main_and_false_on_threads_when_unwinding,
            ),
            (
                "c4_concurrent_cleanups_should_all_wait_for_the_one_that_runs",
                c4_concurrent_cleanups_should_all_wait_for_the_one_that_runs,
            ),
        ];
        let mut failed = 0;
        for (name, f) in tests {
            let ok = catch_unwind(AssertUnwindSafe(f)).is_ok();
            println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
            if !ok {
                failed += 1;
            }
        }
        println!("\nresult: {failed} failed");
        std::process::exit(i32::from(failed > 0));
    }
}
