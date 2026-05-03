use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::event::{genesis_prev_hash, ProvenanceEntry, ProvenanceEvent};

fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Append-only hash-chained event log.
#[derive(Debug)]
pub struct ProvenanceLog {
    /// In-memory entries (mirror of disk).
    pub(crate) entries: Vec<ProvenanceEntry>,
    /// Optional path; when Some, append() also writes to disk.
    path: Option<PathBuf>,
}

impl ProvenanceLog {
    /// New in-memory log; not persistent.
    pub fn new() -> Self {
        Self {
            entries: vec![],
            path: None,
        }
    }

    /// Open or create a log at `path`. Reads existing entries if the file
    /// is non-empty; verifies the chain on load and errors if tampered.
    pub fn open(path: &Path) -> Result<Self, ProvenanceLogError> {
        let entries = if path.exists() {
            let file = std::fs::File::open(path)?;
            let reader = BufReader::new(file);
            let mut acc: Vec<ProvenanceEntry> = Vec::new();
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let entry: ProvenanceEntry = serde_json::from_str(&line)?;
                acc.push(entry);
            }
            acc
        } else {
            vec![]
        };

        let log = Self {
            entries,
            path: Some(path.to_owned()),
        };

        if !log.entries.is_empty() {
            log.verify()?;
        }

        Ok(log)
    }

    /// Append an event. Generates the entry (timestamp, prev_hash, index)
    /// and persists if `path` is Some. Returns the new entry's index.
    pub fn append(&mut self, event: ProvenanceEvent) -> Result<u64, ProvenanceLogError> {
        let index = self.entries.len() as u64;
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.body_hash())
            .unwrap_or_else(genesis_prev_hash);

        let entry = ProvenanceEntry {
            index,
            timestamp: now_rfc3339(),
            prev_hash,
            event,
        };

        if let Some(ref path) = self.path {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            let line = serde_json::to_string(&entry)?;
            writeln!(file, "{line}")?;
        }

        self.entries.push(entry);
        Ok(index)
    }

    /// Snapshot of all entries in order.
    pub fn entries(&self) -> &[ProvenanceEntry] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Re-verify the chain from genesis. Returns Err on the FIRST broken
    /// link with the bad entry's index.
    pub fn verify(&self) -> Result<(), VerifyError> {
        let mut expected_index: u64 = 0;
        let mut expected_prev = genesis_prev_hash();

        for entry in &self.entries {
            if entry.index != expected_index {
                return Err(VerifyError::IndexOutOfOrder {
                    index: entry.index,
                    expected: expected_index,
                });
            }
            if entry.prev_hash != expected_prev {
                return Err(VerifyError::PrevHashMismatch {
                    index: entry.index,
                    expected: expected_prev,
                    actual: entry.prev_hash.clone(),
                });
            }
            expected_prev = entry.body_hash();
            expected_index += 1;
        }

        Ok(())
    }
}

impl Default for ProvenanceLog {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProvenanceLogError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialise: {0}")]
    Serialise(#[from] serde_json::Error),
    #[error("verify on load: {0}")]
    Verify(#[from] VerifyError),
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("entry {index} prev_hash mismatch: expected {expected}, got {actual}")]
    PrevHashMismatch {
        index: u64,
        expected: String,
        actual: String,
    },
    #[error("entry {index} index out of order: expected {expected}")]
    IndexOutOfOrder { index: u64, expected: u64 },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::genesis_prev_hash;
    use tempfile::TempDir;

    fn user_prompt(text: &str) -> ProvenanceEvent {
        ProvenanceEvent::UserPrompt {
            text: text.to_owned(),
        }
    }

    fn tool_call(name: &str) -> ProvenanceEvent {
        ProvenanceEvent::ToolCall {
            name: name.to_owned(),
            arguments: serde_json::json!({}),
        }
    }

    // ------------------------------------------------------------------
    // In-memory append + verify
    // ------------------------------------------------------------------

    #[test]
    fn new_log_is_empty() {
        let log = ProvenanceLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert_eq!(log.entries().len(), 0);
    }

    #[test]
    fn append_first_entry_uses_genesis_prev_hash() {
        let mut log = ProvenanceLog::new();
        let idx = log.append(user_prompt("first")).expect("append");
        assert_eq!(idx, 0);
        let entry = &log.entries()[0];
        assert_eq!(entry.index, 0);
        assert_eq!(entry.prev_hash, genesis_prev_hash());
    }

    #[test]
    fn append_assigns_sequential_indices() {
        let mut log = ProvenanceLog::new();
        for i in 0..5u64 {
            let idx = log.append(user_prompt(&format!("msg {i}"))).expect("append");
            assert_eq!(idx, i, "index must equal position");
        }
        assert_eq!(log.len(), 5);
    }

    #[test]
    fn append_links_prev_hash_to_previous_body_hash() {
        let mut log = ProvenanceLog::new();
        log.append(user_prompt("first")).expect("append");
        log.append(user_prompt("second")).expect("append");

        let e0 = &log.entries()[0];
        let e1 = &log.entries()[1];
        assert_eq!(
            e1.prev_hash,
            e0.body_hash(),
            "second entry's prev_hash must equal first entry's body_hash"
        );
    }

    #[test]
    fn verify_passes_for_freshly_appended_chain() {
        let mut log = ProvenanceLog::new();
        log.append(user_prompt("a")).expect("append");
        log.append(tool_call("read_file")).expect("append");
        log.append(ProvenanceEvent::SessionEnd {
            reason: "ok".to_owned(),
        })
        .expect("append");
        log.verify().expect("chain must verify clean");
    }

    #[test]
    fn verify_fails_with_prev_hash_mismatch_when_middle_entry_tampered() {
        let mut log = ProvenanceLog::new();
        log.append(user_prompt("first")).expect("append");
        log.append(user_prompt("second")).expect("append");
        log.append(user_prompt("third")).expect("append");

        // Tamper with entry 1's event — this invalidates entry 2's prev_hash.
        log.entries[1].event = ProvenanceEvent::UserPrompt {
            text: "TAMPERED".to_owned(),
        };

        let err = log.verify().expect_err("tampered chain must fail");
        match err {
            VerifyError::PrevHashMismatch { index, .. } => {
                assert_eq!(index, 2, "the broken link is detected at entry 2");
            }
            other => panic!("expected PrevHashMismatch, got: {other}"),
        }
    }

    #[test]
    fn verify_fails_with_index_out_of_order_when_index_skipped() {
        let mut log = ProvenanceLog::new();
        log.append(user_prompt("first")).expect("append");
        log.append(user_prompt("second")).expect("append");

        // Manually corrupt the index of entry 1.
        log.entries[1].index = 5;

        let err = log.verify().expect_err("out-of-order index must fail");
        match err {
            VerifyError::IndexOutOfOrder { index, expected } => {
                assert_eq!(index, 5);
                assert_eq!(expected, 1);
            }
            other => panic!("expected IndexOutOfOrder, got: {other}"),
        }
    }

    #[test]
    fn len_and_is_empty_consistent() {
        let mut log = ProvenanceLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);

        log.append(user_prompt("x")).expect("append");
        assert!(!log.is_empty());
        assert_eq!(log.len(), 1);
    }

    // ------------------------------------------------------------------
    // Persistence
    // ------------------------------------------------------------------

    #[test]
    fn open_creates_empty_log_when_file_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("provenance.jsonl");
        // File does not exist yet.
        let log = ProvenanceLog::open(&path).expect("open on missing file");
        assert!(log.is_empty());
    }

    #[test]
    fn open_loads_existing_chain_and_verifies() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("provenance.jsonl");

        {
            let mut log = ProvenanceLog::open(&path).expect("create");
            log.append(user_prompt("one")).expect("append");
            log.append(tool_call("bash")).expect("append");
            log.append(ProvenanceEvent::SessionEnd {
                reason: "complete".to_owned(),
            })
            .expect("append");
        }

        // Re-open and verify contents survive.
        let log2 = ProvenanceLog::open(&path).expect("reopen");
        assert_eq!(log2.len(), 3);
        log2.verify().expect("reloaded chain must verify");
    }

    #[test]
    fn open_returns_verify_error_on_tampered_file() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("provenance.jsonl");

        {
            let mut log = ProvenanceLog::open(&path).expect("create");
            log.append(user_prompt("a")).expect("append");
            log.append(user_prompt("b")).expect("append");
            log.append(user_prompt("c")).expect("append");
        }

        // Read raw lines, corrupt entry 1's event text, rewrite.
        let content = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
        // Replace the middle line with a modified payload.
        let mut corrupted_entry: serde_json::Value =
            serde_json::from_str(lines[1]).expect("parse line 1");
        corrupted_entry["event"]["text"] = serde_json::json!("INJECTED");
        let new_line = serde_json::to_string(&corrupted_entry).expect("reserialise");
        let tampered = format!("{}\n{}\n{}\n", lines[0], new_line, lines[2]);
        std::fs::write(&path, tampered).expect("write tampered file");

        // Reopening must fail with a verify error.
        let result = ProvenanceLog::open(&path);
        assert!(
            matches!(result, Err(ProvenanceLogError::Verify(_))),
            "expected Verify error on tampered file, got: {result:?}"
        );
    }

    #[test]
    fn append_writes_one_line_per_entry() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("provenance.jsonl");

        let mut log = ProvenanceLog::open(&path).expect("create");
        log.append(user_prompt("event one")).expect("append");
        log.append(tool_call("write_file")).expect("append");

        let bytes = std::fs::read(&path).expect("read file");
        let newline_count = bytes.iter().filter(|&&b| b == b'\n').count();
        assert_eq!(
            newline_count, 2,
            "two appended entries must produce exactly 2 newlines, got {newline_count}"
        );
    }
}
