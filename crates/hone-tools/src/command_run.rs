//! Run a command so that it ends with the call that started it.
//!
//! Dropping a `tokio::process::Command::output()` future leaves the process
//! (and anything it started) running. This runner puts the child in its own
//! process group on unix and sends `SIGKILL` to the whole group when the
//! caller's future is dropped or the timeout passes, so `sh -c "cargo test"`
//! does not keep building after Esc. A process-wide registry of the running
//! groups lets [`kill_running_commands`] end them all when hone exits. Other
//! long-lived children that hone starts in their own group (MCP servers)
//! register theirs with a [`ProcessGroupGuard`].
//!
//! Elsewhere only the direct child is killed (`kill_on_drop`).

use std::io;
use std::process::{Output, Stdio};
use std::time::Duration;

/// Why [`run_command`] did not produce an [`Output`].
#[derive(Debug)]
pub enum RunCommandError {
    /// The timeout passed first; the command was killed.
    Timeout,
    /// Spawning or waiting failed.
    Io(io::Error),
}

/// Run `cmd` to completion with stdin closed and stdout/stderr captured.
///
/// If the returned future is dropped, or `timeout` passes (`None` = no limit),
/// the command's whole process group is killed on unix. A command that
/// finishes on its own is left alone, including anything it left running.
pub async fn run_command(
    mut cmd: std::process::Command,
    timeout: Option<Duration>,
) -> Result<Output, RunCommandError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own group (pgid == pid): the sandbox wrapper, `sh` and everything
        // they start belong to it. Set on the std command so any `pre_exec`
        // hook or program rewrite done by the sandbox is kept.
        cmd.process_group(0);
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);

    // A failed spawn never reaches the guard: nothing is registered or killed.
    let child = cmd.spawn().map_err(RunCommandError::Io)?;
    // Armed until the child has exited; fires on drop (cancellation, timeout).
    let mut guard = child.id().map(|pid| GroupGuard::new(pid, false));

    let wait = child.wait_with_output();
    let result = match timeout {
        Some(d) => match tokio::time::timeout(d, wait).await {
            Ok(r) => r.map_err(RunCommandError::Io),
            Err(_elapsed) => return Err(RunCommandError::Timeout),
        },
        None => wait.await.map_err(RunCommandError::Io),
    };
    // The command ended by itself: leave its leftovers alone. After a failed
    // wait the child's state is unknown, so the guard stays armed and kills
    // the group when it drops.
    if result.is_ok() {
        if let Some(g) = guard.as_mut() {
            g.disarm();
        }
    }
    result
}

/// A process group created outside [`run_command`] (the child was spawned with
/// `process_group(0)`, so its group id is its pid), tracked in the same
/// registry: [`kill_running_commands`] and [`kill_commands_and_reraise`] reach
/// it (as an *MCP group*: [`kill_running_commands`] stops it with `SIGTERM`
/// first, [`kill_shell_commands`] leaves it alone). Dropping the guard (or
/// [`kill`](Self::kill)) sends `SIGKILL` to the whole group and deregisters it.
/// Off unix it does nothing.
pub struct ProcessGroupGuard(GroupGuard);

impl ProcessGroupGuard {
    /// Track the group led by the child with process id `pid`.
    pub fn new(pid: u32) -> Self {
        Self(GroupGuard::new(pid, true))
    }

    /// Kill the whole group now (idempotent) and stop tracking it. Call it
    /// whenever hone lets go of the child, also after the leader exited by
    /// itself: a launcher's own child may outlive it.
    pub fn kill(&mut self) {
        self.0.kill();
    }
}

/// Stop every process group that is currently running, for an abrupt exit.
/// Shell-command groups (commands, test runs) get `SIGKILL` at once. Groups
/// registered with a [`ProcessGroupGuard`] (MCP servers) get `SIGTERM` first, so
/// they can clean up (close a browser they started, ...); those still running
/// about a second later get `SIGKILL`. Returns when all are gone, and does not
/// wait when there is no MCP group. Synchronous (a short sleep loop): safe to
/// call from a signal-handling task or right before `process::exit`. A no-op
/// off unix.
pub fn kill_running_commands() {
    #[cfg(unix)]
    unix::kill_all();
}

/// Whether a panic on this thread ends hone: always with `panic = "abort"`,
/// otherwise only on the main thread (a thread or task panic is contained).
#[doc(hidden)]
pub fn panic_ends_process() -> bool {
    cfg!(panic = "abort") || std::thread::current().name() == Some("main")
}

/// The panic hook's cleanup: stop every registered group like
/// [`kill_running_commands`], but wait at most `lock_wait` for the registry
/// (the panicking thread may hold it). Returns `false`, having stopped nothing,
/// if the registry stayed locked. Runs once; concurrent callers wait for it and
/// return `true`. Off unix: `true`, does nothing.
#[doc(hidden)]
pub fn stop_groups_on_panic(lock_wait: Duration) -> bool {
    #[cfg(unix)]
    {
        unix::stop_on_panic(lock_wait)
    }
    #[cfg(not(unix))]
    {
        let _ = lock_wait;
        true
    }
}

/// Install a panic hook that runs the previous hook first, then, when the
/// panic ends hone ([`panic_ends_process`]), calls [`stop_groups_on_panic`]
/// with a 100 ms registry wait, so no command or MCP server outlives a crash.
pub fn install_panic_cleanup() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        if panic_ends_process() {
            stop_groups_on_panic(Duration::from_millis(100));
        }
    }));
}

/// Kill only the shell-command groups (commands, test runs), with `SIGKILL`,
/// and leave registered MCP groups running, for an exit that goes on to shut
/// the MCP servers down gracefully itself. A no-op off unix.
pub fn kill_shell_commands() {
    #[cfg(unix)]
    unix::kill_shell();
}

/// For a one-shot run that received `signal` (SIGINT, SIGTERM, SIGHUP): kill
/// the running commands, then restore the signal's default action and raise it
/// again, so the process ends *by* the signal (a calling shell script sees it,
/// as it would without the handler). `exit(128 + signal)` is only the fallback
/// if raising returns (and the only behaviour off unix).
pub fn kill_commands_and_reraise(signal: i32) -> ! {
    kill_running_commands();
    #[cfg(unix)]
    unix::reraise(signal);
    std::process::exit(128 + signal)
}

/// Whether `signal` is ignored right now (`SIG_IGN`: SIGHUP under `nohup`, SIGINT
/// for a background job). Asks the kernel with a null new action, so it changes
/// nothing. Call it BEFORE installing a handler for that signal (tokio's handler
/// hides the original disposition), and install none if it is ignored, so an
/// ignored signal stays ignored. Always `false` off unix.
pub fn signal_is_ignored(signal: i32) -> bool {
    #[cfg(unix)]
    {
        unix::is_ignored(signal)
    }
    #[cfg(not(unix))]
    {
        let _ = signal;
        false
    }
}

#[cfg(unix)]
use unix::GroupGuard;

#[cfg(unix)]
mod unix {
    use std::collections::HashMap;
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};

    /// Running groups: group id -> is it an MCP server group (else a command).
    pub(super) static RUNNING: Mutex<Option<HashMap<i32, bool>>> = Mutex::new(None);

    /// How long an abrupt exit waits for MCP groups to leave after `SIGTERM`.
    const TERM_GRACE: Duration = Duration::from_secs(1);

    /// Kill process group `pgid`. Group ids 0 (our own group) and 1 (init) and
    /// negatives (`killpg` semantics differ) are never signalled.
    fn kill_group(pgid: i32) {
        if pgid <= 1 {
            return;
        }
        // SAFETY: `killpg` takes plain integers and touches no memory; `pgid`
        // is a group we created (it is the pid of a child we spawned with
        // `process_group(0)`) and is > 1. A failure (ESRCH: group gone) is
        // ignored on purpose.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }

    /// Send `signal` to group `pgid` (same rules as [`kill_group`]).
    fn signal_group(pgid: i32, signal: i32) {
        if pgid <= 1 {
            return;
        }
        // SAFETY: as for `kill_group`.
        unsafe {
            libc::killpg(pgid, signal);
        }
    }

    /// Registers a command's process group while armed and kills it if dropped
    /// still armed.
    pub(super) struct GroupGuard {
        pgid: i32,
        armed: bool,
    }

    impl GroupGuard {
        pub(super) fn new(pid: u32, mcp: bool) -> Self {
            // pgid == pid because of `process_group(0)`. An out-of-range pid
            // maps to 0, which is never signalled or registered.
            let pgid = i32::try_from(pid).unwrap_or(0);
            let armed = pgid > 1;
            if armed {
                RUNNING
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert_with(HashMap::new)
                    .insert(pgid, mcp);
            }
            Self { pgid, armed }
        }

        /// Kill the whole group now and stop tracking it. Does nothing if the
        /// group was already stopped and deregistered by [`kill_all`]: its id
        /// may be reused by now.
        pub(super) fn kill(&mut self) {
            if std::mem::take(&mut self.armed) && unregister(self.pgid) {
                kill_group(self.pgid);
            }
        }

        /// The command ended by itself: stop tracking, never kill.
        pub(super) fn disarm(&mut self) {
            if std::mem::take(&mut self.armed) {
                unregister(self.pgid);
            }
        }
    }

    impl Drop for GroupGuard {
        fn drop(&mut self) {
            self.kill();
        }
    }

    /// Deregister `pgid`; whether it was registered.
    fn unregister(pgid: i32) -> bool {
        RUNNING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .is_some_and(|map| map.remove(&pgid).is_some())
    }

    /// Whether group `pgid` has no live member we care about.
    ///
    /// macOS: `killpg(pgid, 0)` returns ESRCH for an empty group and EPERM when
    /// only zombies are left (a launcher's reaped-by-nobody leader), so either
    /// means gone; it succeeds while any member is alive. A `waitid` shortcut on
    /// the leader would be wrong here: a shell launcher's leader can exit while
    /// its child, the real server, is still handling SIGTERM.
    #[cfg(target_os = "macos")]
    fn group_gone(pgid: i32) -> bool {
        // SAFETY: `killpg(.., 0)` only checks existence.
        let r = unsafe { libc::killpg(pgid, 0) };
        r == -1
            && matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH | libc::EPERM)
            )
    }

    /// Whether group `pgid` has no live member we care about (other unix).
    ///
    /// There `killpg(pgid, 0)` keeps succeeding while an unreaped zombie
    /// remains, so a leader that exited and only waits to be reaped counts as
    /// gone, checked with `waitid(.., WNOWAIT)` (reaps nothing).
    #[cfg(not(target_os = "macos"))]
    fn group_gone(pgid: i32) -> bool {
        // SAFETY: `waitid` with WNOHANG|WNOWAIT only inspects our child's state
        // (it reaps nothing) and writes into a zeroed `siginfo_t` that lives for
        // the call; `killpg(.., 0)` only checks existence.
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            let r = libc::waitid(
                libc::P_PID,
                pgid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            );
            if r == 0 && info.si_signo == libc::SIGCHLD {
                return true; // the leader exited (a zombie nobody reaped yet)
            }
            libc::killpg(pgid, 0) == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }
    }

    /// Reset `signal` to its default action, unblock it and raise it. Returns
    /// only if that failed.
    pub(super) fn reraise(signal: i32) {
        // SAFETY: plain libc calls on integers and a stack-allocated, properly
        // initialised `sigset_t`; no Rust invariants are involved. Each result
        // is ignored on purpose: on failure we fall through to the caller's
        // `exit` fallback.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, signal);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
            libc::raise(signal);
        }
    }

    pub(super) fn is_ignored(signal: i32) -> bool {
        // SAFETY: `sigaction` with a null new action only reads the current
        // disposition into `old`, a zero-initialised, properly sized struct
        // that lives for the whole call.
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            libc::sigaction(signal, std::ptr::null(), &mut old) == 0
                && old.sa_sigaction == libc::SIG_IGN
        }
    }

    /// SIGKILL the command groups. Entries stay registered: each command's own
    /// guard removes its entry when it ends. The lock is held only while
    /// signalling.
    pub(super) fn kill_shell() {
        if let Some(map) = RUNNING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            for (&pgid, &mcp) in map {
                if !mcp {
                    kill_group(pgid);
                }
            }
        }
    }

    /// SIGKILL the command groups in `map` and return the MCP groups. Callers
    /// hold the registry lock only for this call.
    fn kill_shell_in(map: &Option<HashMap<i32, bool>>) -> Vec<i32> {
        let mut mcp_groups = Vec::new();
        if let Some(map) = map {
            for (&pgid, &mcp) in map {
                if mcp {
                    mcp_groups.push(pgid);
                } else {
                    kill_group(pgid);
                }
            }
        }
        mcp_groups
    }

    pub(super) fn kill_all() {
        let mcp_groups = kill_shell_in(&RUNNING.lock().unwrap_or_else(PoisonError::into_inner));
        stop_mcp(&mcp_groups, None);
    }

    /// Run `f` on the registry. `wait: None` blocks for the lock (poison is
    /// ignored); `Some(d)` polls for at most `d` and returns `None` if the
    /// registry stayed locked (a panicking thread may hold it for good).
    fn with_registry<R>(
        wait: Option<Duration>,
        f: impl FnOnce(&mut Option<HashMap<i32, bool>>) -> R,
    ) -> Option<R> {
        let Some(wait) = wait else {
            return Some(f(&mut RUNNING
                .lock()
                .unwrap_or_else(PoisonError::into_inner)));
        };
        let deadline = Instant::now() + wait;
        loop {
            match RUNNING.try_lock() {
                Ok(mut map) => return Some(f(&mut map)),
                Err(std::sync::TryLockError::Poisoned(e)) => return Some(f(&mut e.into_inner())),
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    /// MCP groups: SIGTERM, wait (without the lock) until they are gone, then
    /// SIGKILL what is left and deregister, so a later guard drop does not
    /// signal a reused group id. With `wait: Some(d)` (the panic path) every
    /// registry access waits at most `d`: if the registry stays locked the
    /// group is still SIGKILLed, just not deregistered. After one such timeout
    /// the remaining accesses only try once, so the waits don't add up.
    fn stop_mcp(mcp_groups: &[i32], mut wait: Option<Duration>) {
        if mcp_groups.is_empty() {
            return;
        }
        for &pgid in mcp_groups {
            signal_group(pgid, libc::SIGTERM);
        }
        let deadline = Instant::now() + TERM_GRACE;
        while Instant::now() < deadline && !mcp_groups.iter().all(|&p| group_gone(p)) {
            std::thread::sleep(Duration::from_millis(20));
        }
        for &pgid in mcp_groups {
            let registered = with_registry(wait, |m| {
                m.as_mut().is_some_and(|m| m.remove(&pgid).is_some())
            });
            if registered.is_none() {
                wait = Some(Duration::ZERO);
            }
            if registered != Some(false) {
                kill_group(pgid);
            }
        }
        // A command may have started while we waited (another task running
        // under the grace period): kill those too, so none outlives the exit.
        with_registry(wait, |m| {
            kill_shell_in(m);
        });
    }

    /// Serialises panic cleanups; `true` once one has finished.
    static PANIC_CLEANUP: Mutex<bool> = Mutex::new(false);

    /// The panic hook's body. Waits at most `lock_wait` for the registry (the
    /// panicking thread may hold it itself); on timeout returns `false`, stops
    /// nothing and latches nothing. The cleanup runs once; other callers block
    /// until it is done and return `true`.
    pub(super) fn stop_on_panic(lock_wait: Duration) -> bool {
        let mut done = PANIC_CLEANUP.lock().unwrap_or_else(PoisonError::into_inner);
        if *done {
            return true;
        }
        let Some(mcp_groups) = with_registry(Some(lock_wait), |m| kill_shell_in(m)) else {
            return false;
        };
        stop_mcp(&mcp_groups, Some(lock_wait));
        *done = true;
        true
    }
}

#[cfg(not(unix))]
struct GroupGuard;

#[cfg(not(unix))]
impl GroupGuard {
    fn new(_pid: u32, _mcp: bool) -> Self {
        Self
    }
    fn disarm(&mut self) {}
    fn kill(&mut self) {}
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod panic_lock_tests {
    //! C4 of `docs/plans/panic-stops-processes-spec.md`: the panic hook's body,
    //! `stop_groups_on_panic(lock_wait) -> bool`, cannot hang when the group
    //! registry is locked, even by the panicking thread itself. These need the
    //! registry lock, so they live in the crate: they take `unix::RUNNING`
    //! directly (the implementer must make it `pub(super)`). Neither test may
    //! see the cleanup succeed, so the "ran once" state (if any) stays unset.
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// The tests share the process-wide registry.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn sleeper() -> (std::process::Child, ProcessGroupGuard) {
        let child = std::process::Command::new("sleep")
            .arg("300")
            .process_group(0)
            .spawn()
            .unwrap();
        let guard = ProcessGroupGuard::new(child.id());
        (child, guard)
    }

    #[test]
    fn test_c4_registry_locked_by_another_thread_should_skip_cleanup_within_a_second() {
        let _s = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut child, mut guard) = sleeper();
        let (locked_tx, locked_rx) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _held = unix::RUNNING
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locked_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(1500));
        });
        locked_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let start = Instant::now();
        let ran = stop_groups_on_panic(Duration::from_millis(100));
        let took = start.elapsed();
        let still_running = child.try_wait().unwrap().is_none();

        holder.join().unwrap();
        guard.kill();
        let _ = child.wait();
        assert!(!ran, "the registry was unavailable: nothing was stopped");
        assert!(
            took < Duration::from_secs(1),
            "waited {took:?} for the lock"
        );
        assert!(still_running, "a skipped cleanup must not kill anything");
    }

    #[test]
    fn test_c4_registry_locked_by_the_calling_thread_should_skip_cleanup_within_a_second() {
        let _s = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut child, mut guard) = sleeper();

        let held = unix::RUNNING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let start = Instant::now();
        let ran = stop_groups_on_panic(Duration::from_millis(100));
        let took = start.elapsed();
        drop(held);
        let still_running = child.try_wait().unwrap().is_none();

        guard.kill();
        let _ = child.wait();
        assert!(!ran, "the registry was unavailable: nothing was stopped");
        assert!(
            took < Duration::from_secs(1),
            "hung {took:?} on our own lock"
        );
        assert!(still_running, "a skipped cleanup must not kill anything");
    }

    // ---- C8: two panics at once cannot hang hone ---------------------------
    //
    // The scenario can deadlock for good today, and a stuck thread would hold
    // the registry for the rest of this test binary, so it runs in a child
    // process (this same test binary, filtered to `c8_scenario`) with a bounded
    // wait. The child test is a no-op unless the parent set `C8_DIR_ENV`.

    const C8_DIR_ENV: &str = "HONE_C8_CHILD_DIR";

    fn ps_alive(pid: &str) -> bool {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        !stat.is_empty() && !stat.starts_with('Z')
    }

    /// Child side. Thread A's cleanup is in the SIGTERM grace; thread B
    /// "panicked while holding the registry" (it locks `RUNNING`, then calls the
    /// hook body) and keeps the lock until both calls were seen. Both calls must
    /// return within a few seconds and the SIGTERM-ignoring server must be gone
    /// (SIGKILLed even though the registry stayed locked).
    #[test]
    fn c8_scenario_child_only_runs_under_the_parent() {
        let Some(dir) = std::env::var_os(C8_DIR_ENV) else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let mut server = std::process::Command::new("sh")
            .args(["-c", "trap '' TERM; exec sleep 300"])
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = server.id();
        std::fs::write(dir.join("server.pid"), pid.to_string()).unwrap();
        std::mem::forget(ProcessGroupGuard::new(pid));
        std::thread::sleep(Duration::from_millis(300)); // `trap` and `exec` ran

        let (tx, rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let tx_a = tx.clone();
        std::thread::spawn(move || {
            let r = stop_groups_on_panic(Duration::from_millis(100));
            let _ = tx_a.send(("A", r));
        });
        std::thread::sleep(Duration::from_millis(300));
        std::thread::spawn(move || {
            let _held = unix::RUNNING
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let r = stop_groups_on_panic(Duration::from_millis(100));
            let _ = tx.send(("B", r));
            let _ = release_rx.recv_timeout(Duration::from_secs(15));
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < 2 {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(m) => got.push(m),
                Err(_) => break,
            }
        }
        let gone = !ps_alive(&pid.to_string());
        let _ = release_tx.send(());
        // SAFETY: killing the group we created above (pid > 1).
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
        let _ = server.wait();
        assert_eq!(got.len(), 2, "DEADLOCK: only {got:?} returned within 5 s");
        assert!(gone, "the SIGTERM-ignoring server must be SIGKILLed anyway");
    }

    #[test]
    fn test_c8_second_panic_while_holding_the_registry_should_not_deadlock_the_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("child.log");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["c8_scenario", "--test-threads=1", "--nocapture"])
            .env(C8_DIR_ENV, dir.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(std::fs::File::options().append(true).open(&log).unwrap())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break Some(s);
            }
            if start.elapsed() > Duration::from_secs(25) {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        // Never leak the server, whatever happened.
        if let Ok(pid) = std::fs::read_to_string(dir.path().join("server.pid")) {
            let _ = std::process::Command::new("kill")
                .args(["-9", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status();
        }
        let out = std::fs::read_to_string(&log).unwrap_or_default();
        let status = status.unwrap_or_else(|| panic!("scenario hung; log:\n{out}"));
        assert!(status.success(), "scenario failed; log:\n{out}");
    }
}
