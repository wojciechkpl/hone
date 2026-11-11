//! Run a command so that it ends with the call that started it.
//!
//! Dropping a `tokio::process::Command::output()` future leaves the process
//! (and anything it started) running. This runner puts the child in its own
//! process group on unix and sends `SIGKILL` to the whole group when the
//! caller's future is dropped or the timeout passes, so `sh -c "cargo test"`
//! does not keep building after Esc. A process-wide registry of the running
//! groups lets [`kill_running_commands`] end them all when hone exits.
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
    let mut guard = child.id().map(GroupGuard::new);

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

/// Kill every command group that is currently running. Synchronous and cheap;
/// safe to call from a signal-handling task. A no-op off unix.
pub fn kill_running_commands() {
    #[cfg(unix)]
    unix::kill_all();
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
    use std::collections::HashSet;
    use std::sync::{Mutex, PoisonError};

    static RUNNING: Mutex<Option<HashSet<i32>>> = Mutex::new(None);

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

    /// Registers a command's process group while armed and kills it if dropped
    /// still armed.
    pub(super) struct GroupGuard {
        pgid: i32,
        armed: bool,
    }

    impl GroupGuard {
        pub(super) fn new(pid: u32) -> Self {
            // pgid == pid because of `process_group(0)`. An out-of-range pid
            // maps to 0, which is never signalled or registered.
            let pgid = i32::try_from(pid).unwrap_or(0);
            let armed = pgid > 1;
            if armed {
                RUNNING
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert_with(HashSet::new)
                    .insert(pgid);
            }
            Self { pgid, armed }
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
            if std::mem::take(&mut self.armed) {
                kill_group(self.pgid);
                unregister(self.pgid);
            }
        }
    }

    fn unregister(pgid: i32) {
        if let Some(set) = RUNNING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            set.remove(&pgid);
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

    pub(super) fn kill_all() {
        // Entries stay registered: each command's own guard removes its entry
        // when it ends. The lock is held only while signalling.
        if let Some(set) = RUNNING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            for &pgid in set {
                kill_group(pgid);
            }
        }
    }
}

#[cfg(not(unix))]
struct GroupGuard;

#[cfg(not(unix))]
impl GroupGuard {
    fn new(_pid: u32) -> Self {
        Self
    }
    fn disarm(&mut self) {}
}
