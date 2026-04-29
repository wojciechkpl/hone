use std::{
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
};

/// Maximum number of history entries to keep in memory and on disk.
const MAX_ENTRIES: usize = 500;

/// Persistent per-session prompt history.
///
/// Entries are stored in `~/.local/state/hone/history`, one per line, oldest
/// first (most recent last).  On navigation the cursor moves backwards through
/// the list so that `Up` shows the most-recently-used entry.
pub struct PromptHistory {
    /// Deduplicated history entries; most recent is at the highest index.
    entries: Vec<String>,
    /// Index into `entries` while browsing.  `None` means we are at the live
    /// input (no history entry selected).
    cursor: Option<usize>,
    /// The live input text that was in the box before the user started
    /// navigating history.  Restored when the cursor is advanced past the end.
    draft: String,
    /// Path of the on-disk history file.
    path: PathBuf,
}

impl PromptHistory {
    /// Load history from `~/.local/state/hone/history`, creating the file and
    /// parent directories if they do not exist.  Silently degrades to an empty
    /// in-memory history if the file cannot be read.
    pub fn load() -> Self {
        let path = history_path();
        let entries = load_entries(&path);
        Self {
            entries,
            cursor: None,
            draft: String::new(),
            path,
        }
    }

    /// Append `prompt` to the history if it is non-empty and differs from the
    /// most-recent entry.  Also appends the line to the on-disk file.
    /// Resets the navigation cursor to `None`.
    pub fn push(&mut self, prompt: &str) {
        if prompt.is_empty() {
            return;
        }
        // Deduplicate: if it already matches the last entry, don't add again.
        if self.entries.last().map(|e| e.as_str()) == Some(prompt) {
            self.cursor = None;
            return;
        }
        // Also remove any earlier duplicate so the entry moves to the end.
        self.entries.retain(|e| e != prompt);
        self.entries.push(prompt.to_string());

        // Trim to cap.
        if self.entries.len() > MAX_ENTRIES {
            let overflow = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..overflow);
        }

        // Persist the line to disk (append mode).
        append_entry(&self.path, prompt);

        self.cursor = None;
    }

    /// Move the cursor toward older entries (Up key behaviour).
    ///
    /// Saves the current live input as the draft on the first call.  Returns
    /// the entry text if there is an older entry to navigate to, or `None`
    /// when the history is empty or already at the oldest entry.
    pub fn prev(&mut self, live: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let new_cursor = match self.cursor {
            // First press: save draft, jump to most-recent entry.
            None => {
                self.draft = live.to_string();
                self.entries.len() - 1
            }
            // Already at oldest entry; stay put.
            Some(0) => return Some(&self.entries[0]),
            Some(i) => i - 1,
        };
        self.cursor = Some(new_cursor);
        Some(&self.entries[new_cursor])
    }

    /// Move the cursor toward newer entries (Down key behaviour).
    ///
    /// Returns `None` when the cursor advances past the newest entry, signalling
    /// that the caller should restore the saved draft.
    pub fn next(&mut self) -> Option<&str> {
        let Some(i) = self.cursor else {
            return None; // Not in history navigation mode.
        };
        if i + 1 >= self.entries.len() {
            // Went past the end — exit history navigation.
            self.cursor = None;
            None
        } else {
            let new_cursor = i + 1;
            self.cursor = Some(new_cursor);
            Some(&self.entries[new_cursor])
        }
    }

    /// The saved live-input text from before history navigation began.
    pub fn draft(&self) -> &str {
        &self.draft
    }

    /// Returns `true` when the cursor is currently in history-navigation mode.
    pub fn is_navigating(&self) -> bool {
        self.cursor.is_some()
    }

    /// Search for entries containing `query` (case-insensitive), returning up
    /// to `limit` matches ordered most-recent first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&str> {
        let q = query.to_lowercase();
        self.entries
            .iter()
            .rev()
            .filter(|e| e.to_lowercase().contains(&q))
            .take(limit)
            .map(String::as_str)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn history_path() -> PathBuf {
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join(".local")
        .join("state")
        .join("hone")
        .join("history")
}

fn load_entries(path: &PathBuf) -> Vec<String> {
    // Ensure parent directories exist.
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Ensure the file exists.
    if !path.exists() {
        let _ = fs::File::create(path);
        return Vec::new();
    }

    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let reader = io::BufReader::new(file);
    let mut entries: Vec<String> = reader
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| !l.is_empty())
        .collect();

    // Cap at MAX_ENTRIES (keep the most recent).
    if entries.len() > MAX_ENTRIES {
        let overflow = entries.len() - MAX_ENTRIES;
        entries.drain(..overflow);
    }

    entries
}

fn append_entry(path: &PathBuf, entry: &str) {
    let file = fs::OpenOptions::new().create(true).append(true).open(path);
    if let Ok(mut f) = file {
        let _ = writeln!(f, "{entry}");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_history(dir: &TempDir) -> PromptHistory {
        let path = dir.path().join("history");
        PromptHistory {
            entries: Vec::new(),
            cursor: None,
            draft: String::new(),
            path,
        }
    }

    #[test]
    fn push_adds_entries_and_deduplicates() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        h.push("first");
        h.push("second");
        h.push("first"); // duplicate — should move to end
        assert_eq!(h.entries, vec!["second", "first"]);
    }

    #[test]
    fn push_same_as_last_is_noop() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        h.push("same");
        h.push("same");
        assert_eq!(h.entries.len(), 1);
    }

    #[test]
    fn prev_navigates_to_most_recent() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        h.push("alpha");
        h.push("beta");
        let entry = h.prev("live input");
        assert_eq!(entry, Some("beta"));
    }

    #[test]
    fn prev_saves_draft() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        h.push("alpha");
        h.prev("my draft");
        assert_eq!(h.draft(), "my draft");
    }

    #[test]
    fn prev_then_next_returns_none_and_restores_draft() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        h.push("alpha");
        h.prev("draft");
        let restored = h.next();
        // Went past end → None
        assert_eq!(restored, None);
        assert!(!h.is_navigating());
    }

    #[test]
    fn navigation_wraps_correctly_two_entries() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        h.push("alpha");
        h.push("beta");
        assert_eq!(h.prev(""), Some("beta"));
        assert_eq!(h.prev(""), Some("alpha"));
        // Already at oldest — stays at alpha.
        assert_eq!(h.prev(""), Some("alpha"));
        // Go forward to beta.
        assert_eq!(h.next(), Some("beta"));
        // One more forward exits navigation.
        assert_eq!(h.next(), None);
    }

    #[test]
    fn search_returns_matching_entries_most_recent_first() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        h.push("fix the bug");
        h.push("add feature");
        h.push("fix typo");
        let results = h.search("fix", 10);
        assert_eq!(results, vec!["fix typo", "fix the bug"]);
    }

    #[test]
    fn search_respects_limit() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        for i in 0..20 {
            h.push(&format!("entry {i}"));
        }
        let results = h.search("entry", 5);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn push_resets_cursor() {
        let dir = TempDir::new().unwrap();
        let mut h = make_history(&dir);
        h.push("alpha");
        h.prev(""); // enter navigation mode
        assert!(h.is_navigating());
        h.push("beta");
        assert!(!h.is_navigating());
    }
}
