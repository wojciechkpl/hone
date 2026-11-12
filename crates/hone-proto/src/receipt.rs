//! Context receipts: a record of what Hone put in front of the model for one
//! call. Receipts carry ids, paths, hashes, names, scores and counts, never
//! message, memory or file text.
//!
//! See `docs/plans/context-receipts-spec.md`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::message::Message;

/// A part of the prompt a block of context belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    SystemPrompt,
    Recall,
    Skills,
    Injection,
    History,
    /// Reserved for pluggable retrieval.
    Retrieval,
}

/// The strategy that produced a block (or a compaction step).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyId {
    pub slot: Slot,
    pub name: String,
    pub version: String,
    /// SHA-256 of the strategy's parameters as canonical (key-sorted) JSON.
    pub params_sha256: String,
}

/// Where a piece of context came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceRef {
    Memory {
        id: String,
        version: Option<u64>,
    },
    File {
        path: PathBuf,
        sha256: Option<String>,
        lines: Option<(u32, u32)>,
    },
    Message {
        index: usize,
        sha256: String,
    },
    ToolResult {
        call_id: String,
        sha256: String,
    },
    Skill {
        name: String,
    },
    Command {
        argv: Vec<String>,
        sha256: String,
    },
    Chunk {
        index: String,
        chunk_id: String,
        sha256: String,
    },
    Opaque {
        plugin: String,
        note: String,
    },
}

/// A source with the score the strategy gave it, if any.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoredSource {
    pub source: SourceRef,
    pub score: Option<f32>,
}

/// One block of the request: a slot, the strategy behind it and its sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockReceipt {
    pub slot: Slot,
    pub strategy: StrategyId,
    pub tokens: u32,
    pub sources: Vec<ScoredSource>,
    pub duration_ms: u32,
}

/// One change the context manager made to the history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionStep {
    pub strategy: StrategyId,
    /// What was replaced, with the hashes it had.
    pub replaced: Vec<SourceRef>,
    /// The summary or pointer that took its place; `None` when content was
    /// only removed.
    pub derived: Option<SourceRef>,
}

/// What one model call was given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextReceipt {
    /// 1-based turn number.
    pub turn: u32,
    /// 1-based model call within the turn.
    pub call: u32,
    pub model: String,
    /// See [`request_sha256`].
    pub request_sha256: String,
    pub blocks: Vec<BlockReceipt>,
    /// Candidates a strategy fetched but cut.
    pub dropped: Vec<SourceRef>,
    /// Compaction since the previous receipt.
    pub compaction: Vec<CompactionStep>,
    pub tools_offered: Vec<String>,
    /// Number of non-system messages in the request.
    pub history_len: u32,
    /// SHA-256 (hex) of the newline-joined per-message hashes of those
    /// messages, in request order. The `History` block lists only the messages
    /// new or changed since the agent's previous receipt; this commits to all.
    pub history_root: String,
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Rebuild `value` with every object's keys in sorted order, so the compact
/// output is canonical whether or not serde_json's `preserve_order` is on.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), sorted(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// SHA-256 (hex) of the compact, key-sorted JSON of `value`.
pub fn canonical_json_sha256(value: &Value) -> String {
    sha256_hex(sorted(value).to_string().as_bytes())
}

/// SHA-256 (hex) of the canonical JSON of one message: role, content, tool
/// calls and tool-call id (empty fields omitted, as serde writes them).
pub fn message_sha256(message: &Message) -> String {
    let value = serde_json::to_value(message).unwrap_or(Value::Null);
    canonical_json_sha256(&value)
}

/// SHA-256 (hex) of the canonical JSON of `{"messages": [...], "tools": [names]}`
/// for a completion request, in the order sent.
pub fn request_sha256(messages: &[Message], tool_names: &[String]) -> String {
    let messages: Vec<Value> = messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect();
    canonical_json_sha256(&json!({ "messages": messages, "tools": tool_names }))
}
