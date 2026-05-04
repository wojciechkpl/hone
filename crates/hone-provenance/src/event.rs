use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One observable event in the agent run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProvenanceEvent {
    UserPrompt { text: String },
    ModelCompletion { model: String, output: String },
    ToolCall { name: String, arguments: Value },
    ToolResult { name: String, result: Value, is_error: bool },
    SessionEnd { reason: String },
}

/// One entry in the chain. `prev_hash` links to the previous entry; the
/// FIRST entry has `prev_hash = "0".repeat(64)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    /// Sequential index, starting at 0.
    pub index: u64,
    /// Wall-clock UTC RFC3339.
    pub timestamp: String,
    /// SHA-256 hex digest (64 chars) of the previous entry's serialised body.
    pub prev_hash: String,
    /// The event payload.
    pub event: ProvenanceEvent,
}

impl ProvenanceEntry {
    /// SHA-256 over the JSON-serialised entry body. Used as the
    /// `prev_hash` of the NEXT entry. Pure function for direct testing.
    pub fn body_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(self).expect("entry serialises");
        let hash = Sha256::digest(&bytes);
        hex::encode(hash)
    }
}

/// The 64-char zero-string used as the prev_hash of the first entry.
pub fn genesis_prev_hash() -> String {
    "0".repeat(64)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(event: ProvenanceEvent) -> ProvenanceEntry {
        ProvenanceEntry {
            index: 0,
            timestamp: "2026-05-02T12:00:00Z".to_owned(),
            prev_hash: "0".repeat(64),
            event,
        }
    }

    #[test]
    fn body_hash_is_64_char_hex() {
        let entry = sample_entry(ProvenanceEvent::UserPrompt {
            text: "hello".to_owned(),
        });
        let h = entry.body_hash();
        assert_eq!(h.len(), 64, "SHA-256 hex must be 64 chars");
        assert!(
            h.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must be all hex digits, got: {h}"
        );
    }

    #[test]
    fn body_hash_changes_when_event_changes() {
        let e1 = sample_entry(ProvenanceEvent::UserPrompt {
            text: "hello".to_owned(),
        });
        let e2 = sample_entry(ProvenanceEvent::UserPrompt {
            text: "world".to_owned(),
        });
        assert_ne!(
            e1.body_hash(),
            e2.body_hash(),
            "different events must yield different hashes"
        );
    }

    #[test]
    fn body_hash_stable_for_identical_entries() {
        let e = sample_entry(ProvenanceEvent::SessionEnd {
            reason: "done".to_owned(),
        });
        assert_eq!(
            e.body_hash(),
            e.body_hash(),
            "repeated calls must return the same hash"
        );
    }

    #[test]
    fn genesis_prev_hash_is_64_zeros() {
        let h = genesis_prev_hash();
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c == '0'), "genesis hash must be all zeros");
    }

    #[test]
    fn event_serde_roundtrip_user_prompt() {
        let ev = ProvenanceEvent::UserPrompt {
            text: "test prompt".to_owned(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: ProvenanceEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }

    #[test]
    fn event_serde_roundtrip_tool_call() {
        let ev = ProvenanceEvent::ToolCall {
            name: "read_file".to_owned(),
            arguments: serde_json::json!({ "path": "/tmp/foo.txt" }),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: ProvenanceEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }

    #[test]
    fn event_serde_roundtrip_tool_result_with_is_error_true() {
        let ev = ProvenanceEvent::ToolResult {
            name: "read_file".to_owned(),
            result: serde_json::json!("permission denied"),
            is_error: true,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: ProvenanceEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
        // Confirm the flag survived the round-trip.
        if let ProvenanceEvent::ToolResult { is_error, .. } = back {
            assert!(is_error);
        } else {
            panic!("expected ToolResult variant");
        }
    }
}
