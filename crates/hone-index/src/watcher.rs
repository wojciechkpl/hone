use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::error::IndexError;
use crate::sqlite_index::SqliteIndex;

/// How long to wait after the last event for a given path before re-indexing.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Returns `true` if a *directory* component is one we should skip watching.
///
/// Only directory components that are actual dot-directories (like `.git`,
/// `.cargo`, `.hidden`) are skipped.  The leading `.` check is intentionally
/// applied only to the *last* path segment of directories that appear *inside*
/// the watched root, not to any ancestor directory of the root itself (e.g. a
/// temporary directory under `/var/folders/.../T/.tmpXXX`).
fn is_skipped_dir_name(name: &str) -> bool {
    name.starts_with('.') || name == "target" || name == "node_modules"
}

/// Returns `true` if `path` passes through a directory we want to skip,
/// relative to `root`.
///
/// Only components *beneath* root are examined so that dot-segments in the
/// root path itself (e.g. a temp dir whose name starts with `.tmp`) do not
/// cause all events to be dropped.
fn path_is_skipped(path: &Path, root: &Path) -> bool {
    // Strip the root prefix; if that fails just use the full path.
    let relative = path.strip_prefix(root).unwrap_or(path);
    // Walk each *directory* component of the relative path (all but the final
    // filename) and check whether any are on the skip list.
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        // Skip the final component (the filename itself).
        if components.peek().is_none() {
            break;
        }
        let name = component.as_os_str().to_string_lossy();
        if is_skipped_dir_name(&name) {
            return true;
        }
    }
    false
}

/// Returns `true` if this path has a file extension we care about.
fn has_indexable_ext(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("rs") | Some("py")
    )
}

/// Drives incremental re-indexing of the filesystem as files change.
pub struct IndexWatcher {
    index: Arc<SqliteIndex>,
    root: PathBuf,
}

impl IndexWatcher {
    /// Create a new watcher for `root`, backed by `index`.
    pub fn new(index: Arc<SqliteIndex>, root: PathBuf) -> Self {
        Self { index, root }
    }

    /// Spawn a background tokio task that re-indexes files as they change.
    ///
    /// The returned `WatcherHandle` keeps the background task alive.  Dropping
    /// it (or calling `shutdown`) signals the task to exit.
    pub async fn spawn(self) -> Result<WatcherHandle, IndexError> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // The notify watcher sends raw events through a std::sync::mpsc channel
        // (notify's API is sync).  We bridge that to a tokio::sync::mpsc so the
        // async task can await on it.
        let (event_tx, mut event_rx) =
            tokio::sync::mpsc::unbounded_channel::<notify::Result<Event>>();

        let mut raw_watcher = RecommendedWatcher::new(
            move |res| {
                // Drop send errors — if the receiver is gone we just stop.
                let _ = event_tx.send(res);
            },
            notify::Config::default(),
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        raw_watcher
            .watch(&self.root, RecursiveMode::Recursive)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let index = self.index;
        // On macOS, /var is a symlink to /private/var.  TempDir (and most user
        // code) stores paths under /var/..., but FSEvents delivers events with
        // the canonical /private/var/... prefix.  We keep both forms:
        //   - `canonical_root` for matching against event paths (strip_prefix).
        //   - `original_root` for reconstructing the path as the *caller* would
        //     have stored it in the index, so DB lookups succeed.
        let original_root = self.root.clone();
        let canonical_root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());

        tokio::spawn(async move {
            // Keep the watcher alive for the duration of the task.
            let _watcher = raw_watcher;

            // Debounce state: path -> deadline after which we act.
            // Paths stored here are in the *original* (non-canonical) form so
            // that index operations (index_file / invalidate) match what the
            // caller stored.
            let mut pending: HashMap<PathBuf, (Instant, EventKind)> = HashMap::new();

            let mut shutdown = std::pin::pin!(shutdown_rx);

            loop {
                // Calculate how long until the earliest pending debounce fires.
                let next_deadline = pending
                    .values()
                    .map(|(t, _)| *t)
                    .min()
                    .map(|t| t.saturating_duration_since(Instant::now()));

                let sleep_dur = next_deadline.unwrap_or(Duration::from_secs(60));

                tokio::select! {
                    biased;

                    // Shutdown signal received — exit cleanly.
                    _ = &mut shutdown => break,

                    // A new filesystem event arrived.
                    maybe_event = event_rx.recv() => {
                        let Some(res) = maybe_event else { break };
                        match res {
                            Ok(event) => handle_event(
                                &mut pending,
                                event,
                                &canonical_root,
                                &original_root,
                            ),
                            Err(e) => warn!("notify error: {e}"),
                        }
                    }

                    // Debounce timer elapsed — flush any matured entries.
                    _ = tokio::time::sleep(sleep_dur) => {
                        flush_pending(&mut pending, &index).await;
                    }
                }

                // After each select arm, flush anything that has matured while
                // we were busy (avoids an extra sleep cycle).
                flush_pending(&mut pending, &index).await;
            }
        });

        Ok(WatcherHandle {
            _shutdown: shutdown_tx,
        })
    }
}

/// Accumulate the event into the debounce map.
///
/// `canonical_root` is used for `strip_prefix` matching against the
/// OS-canonical paths in notify events.  `original_root` is the root as
/// provided by the caller; paths inserted into `pending` are re-based onto
/// `original_root` so that later `index_file` / `invalidate` calls use the
/// same prefix that was originally stored in the index.
fn handle_event(
    pending: &mut HashMap<PathBuf, (Instant, EventKind)>,
    event: Event,
    canonical_root: &Path,
    original_root: &Path,
) {
    let deadline = Instant::now() + DEBOUNCE;

    for canonical_path in event.paths {
        // Re-base: strip the canonical prefix and reattach the original root.
        // This ensures the path we store matches what was written into the
        // index by the caller (which used the original, potentially symlinked,
        // root path).
        let relative = match canonical_path.strip_prefix(canonical_root) {
            Ok(rel) => rel,
            Err(_) => {
                // Event path doesn't start with our root — ignore it.
                continue;
            }
        };

        // Check that no directory component along the relative portion is
        // on the skip-list (e.g. .git, target, node_modules).
        if path_is_skipped(&canonical_path, canonical_root) {
            debug!(
                path = %canonical_path.display(),
                "watcher: skipping hidden/excluded path"
            );
            continue;
        }

        if !has_indexable_ext(&canonical_path) {
            continue;
        }

        // Reconstruct the path using the original (non-canonical) root so
        // it matches the form stored in the SQLite index.
        let path = original_root.join(relative);

        // A Remove trumps any earlier Modify/Create so we don't try to read
        // a file that is gone.
        let kind = if matches!(event.kind, EventKind::Remove(_)) {
            EventKind::Remove(notify::event::RemoveKind::Any)
        } else {
            // Treat Create and Modify the same way: re-index.
            EventKind::Modify(notify::event::ModifyKind::Any)
        };

        pending
            .entry(path)
            .and_modify(|(t, k)| {
                *t = deadline;
                // Remove still wins over Modify/Create.
                if matches!(event.kind, EventKind::Remove(_)) {
                    *k = EventKind::Remove(notify::event::RemoveKind::Any);
                }
            })
            .or_insert((deadline, kind));
    }
}

/// Process all debounce entries whose deadline has elapsed.
async fn flush_pending(
    pending: &mut HashMap<PathBuf, (Instant, EventKind)>,
    index: &Arc<SqliteIndex>,
) {
    let now = Instant::now();
    let ready: Vec<(PathBuf, EventKind)> = pending
        .iter()
        .filter(|(_, (deadline, _))| *deadline <= now)
        .map(|(p, (_, k))| (p.clone(), *k))
        .collect();

    for (path, kind) in ready {
        pending.remove(&path);
        if matches!(kind, EventKind::Remove(_)) {
            debug!(path = %path.display(), "watcher: invalidating deleted file");
            if let Err(e) = index.invalidate(&path).await {
                warn!(path = %path.display(), "watcher: invalidate failed: {e}");
            }
        } else {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    debug!(path = %path.display(), "watcher: re-indexing modified file");
                    if let Err(e) = index.index_file(&path, &content).await {
                        warn!(path = %path.display(), "watcher: index_file failed: {e}");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Race between Modify and removal — treat as remove.
                    debug!(path = %path.display(), "watcher: file gone during re-index, invalidating");
                    if let Err(e) = index.invalidate(&path).await {
                        warn!(path = %path.display(), "watcher: invalidate (post-race) failed: {e}");
                    }
                }
                Err(e) => {
                    warn!(path = %path.display(), "watcher: read_to_string failed: {e}");
                }
            }
        }
    }
}

/// Keeps the background watcher task alive.
///
/// Dropping this handle signals the task to exit.
pub struct WatcherHandle {
    _shutdown: oneshot::Sender<()>,
}
