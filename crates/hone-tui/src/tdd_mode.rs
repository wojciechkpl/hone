//! TDD watch loop for the Hone TUI.
//!
//! When the user types `:tdd [<command>]`, Hone enters TDD mode:
//!
//! 1. A file-watcher is started on the current working directory.
//! 2. When any source file changes, the configured test command is run.
//! 3. Results (pass / fail + output) are sent back to the TUI as messages via
//!    a [`tokio::sync::mpsc`] channel.
//! 4. The user can exit with `:tdd off` or `:tdd stop`.
//!
//! ## Design
//!
//! [`TddWatcher`] owns the watch loop task.  Drop it (or call [`TddWatcher::stop`])
//! to cancel the watcher.  The [`TddEvent`] enum carries both test results and
//! informational notices back to the TUI.
//!
//! The implementation uses `notify` (already a workspace dependency via
//! `hone-index`) through a simple polling loop rather than re-exporting the
//! `IndexWatcher` from `hone-index` — TDD mode only needs "did anything change"
//! and does not index symbols.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;
use tokio::sync::watch;

/// Default debounce: wait this long after the last event before re-running tests.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Events produced by the TDD watch loop.
#[derive(Debug, Clone)]
pub enum TddEvent {
    /// TDD mode started; the given command will be run on file changes.
    Started { command: String },
    /// A file changed; tests are about to run.
    RunningTests { changed_path: PathBuf },
    /// Tests passed.
    Passed { output: String },
    /// Tests failed.
    Failed { output: String },
    /// The watch loop encountered an error (non-fatal — watch continues).
    WatchError(String),
    /// TDD mode has stopped.
    Stopped,
}

/// A running TDD watcher.  Drop to stop.
pub struct TddWatcher {
    /// Used to signal the background task to shut down.
    stop_tx: watch::Sender<bool>,
    /// Holds the background task alive.
    _task: tokio::task::JoinHandle<()>,
}

impl TddWatcher {
    /// Start watching `root` for changes and running `command` on each change.
    ///
    /// Events are sent to `event_tx`.
    pub fn start(
        root: PathBuf,
        command: String,
        event_tx: mpsc::Sender<TddEvent>,
    ) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let cmd = command.clone();
        let _ = event_tx.try_send(TddEvent::Started { command });

        let task = tokio::spawn(watch_loop(root, cmd, event_tx, stop_rx));

        TddWatcher { stop_tx, _task: task }
    }

    /// Signal the background task to shut down.
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}

impl Drop for TddWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Watch loop
// ---------------------------------------------------------------------------

async fn watch_loop(
    root: PathBuf,
    command: String,
    event_tx: mpsc::Sender<TddEvent>,
    mut stop_rx: watch::Receiver<bool>,
) {
    // File-change events from notify land in this channel.
    let (notify_tx, mut notify_rx) = mpsc::channel::<PathBuf>(64);

    // Set up the notify watcher on a blocking thread so it can use the
    // synchronous notify API.
    let root_for_watcher = root.clone();
    let notify_tx_clone = notify_tx.clone();
    let mut _watcher: Option<RecommendedWatcher> = None;

    match notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            if matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                for path in event.paths {
                    if is_source_file(&path) {
                        let _ = notify_tx_clone.try_send(path);
                    }
                }
            }
        }
    }) {
        Ok(mut watcher) => {
            if let Err(e) = watcher.watch(&root_for_watcher, RecursiveMode::Recursive) {
                let _ = event_tx
                    .send(TddEvent::WatchError(format!("watcher start failed: {e}")))
                    .await;
                let _ = event_tx.send(TddEvent::Stopped).await;
                return;
            }
            _watcher = Some(watcher);
        }
        Err(e) => {
            let _ = event_tx
                .send(TddEvent::WatchError(format!("watcher init failed: {e}")))
                .await;
            let _ = event_tx.send(TddEvent::Stopped).await;
            return;
        }
    }

    // Debounce: track when the last event arrived.
    let mut last_event: Option<(Instant, PathBuf)> = None;

    loop {
        tokio::select! {
            // Stop signal.
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break;
                }
            }

            // New file-change event.
            Some(path) = notify_rx.recv() => {
                last_event = Some((Instant::now(), path));
            }

            // Debounce timer: check every DEBOUNCE whether we have a pending event.
            _ = tokio::time::sleep(DEBOUNCE) => {
                if let Some((ts, ref path)) = last_event {
                    if ts.elapsed() >= DEBOUNCE {
                        let path = path.clone();
                        last_event = None;

                        let _ = event_tx.send(TddEvent::RunningTests {
                            changed_path: path,
                        }).await;

                        let output = run_command(&command, &root).await;
                        let success = output.success;
                        let text = output.text;

                        let tdd_event = if success {
                            TddEvent::Passed { output: text }
                        } else {
                            TddEvent::Failed { output: text }
                        };
                        let _ = event_tx.send(tdd_event).await;
                    }
                }
            }
        }
    }

    let _ = event_tx.send(TddEvent::Stopped).await;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` for source files we care about.
fn is_source_file(path: &Path) -> bool {
    // Skip hidden files and build artefacts.
    for component in path.components() {
        let name = component.as_os_str().to_string_lossy();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            return false;
        }
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(ext, "rs" | "py" | "ts" | "js" | "go" | "java" | "c" | "cpp" | "h")
}

struct CmdOutput {
    text: String,
    success: bool,
}

async fn run_command(command: &str, working_dir: &Path) -> CmdOutput {
    use tokio::process::Command;

    let result = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .output()
        .await;

    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let mut text = String::new();
            if !stdout.is_empty() {
                text.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            CmdOutput {
                text: text.trim_end().to_owned(),
                success: out.status.success(),
            }
        }
        Err(e) => CmdOutput {
            text: format!("failed to run command: {e}"),
            success: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_file_detection_rust() {
        assert!(is_source_file(Path::new("/project/src/main.rs")));
        assert!(is_source_file(Path::new("/project/src/lib.rs")));
    }

    #[test]
    fn source_file_detection_python() {
        assert!(is_source_file(Path::new("/project/app.py")));
    }

    #[test]
    fn source_file_detection_skips_target() {
        assert!(!is_source_file(Path::new("/project/target/debug/build/main.rs")));
    }

    #[test]
    fn source_file_detection_skips_hidden() {
        assert!(!is_source_file(Path::new("/project/.git/hooks/pre-commit")));
    }

    #[test]
    fn source_file_detection_skips_node_modules() {
        assert!(!is_source_file(Path::new("/project/node_modules/react/index.js")));
    }

    #[test]
    fn source_file_detection_unknown_ext() {
        assert!(!is_source_file(Path::new("/project/README.md")));
        assert!(!is_source_file(Path::new("/project/data.json")));
    }

    #[tokio::test]
    async fn tdd_watcher_can_be_created_and_stopped() {
        let (tx, mut rx) = mpsc::channel(16);
        let dir = tempfile::tempdir().unwrap();
        let watcher = TddWatcher::start(dir.path().to_path_buf(), "echo ok".to_string(), tx);

        // Should receive a Started event quickly.
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for Started event")
            .expect("channel closed");

        assert!(matches!(event, TddEvent::Started { .. }));

        // Stop the watcher.
        watcher.stop();

        // Should eventually receive a Stopped event.
        let mut got_stopped = false;
        for _ in 0..20 {
            if let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                if matches!(e, TddEvent::Stopped) {
                    got_stopped = true;
                    break;
                }
            }
        }
        assert!(got_stopped, "expected TddEvent::Stopped after stop()");
    }
}
