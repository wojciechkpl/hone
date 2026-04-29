//! Doom-loop detector.
//!
//! Scans recent assistant tool-call history for two patterns that mean the
//! model is stuck:
//!
//! 1. **Identical-consecutive**: 3+ back-to-back calls with the same
//!    `(tool_name, hash(arguments))` signature.
//! 2. **Repeating-sequence**: a short pattern (length 2–5) of tool calls
//!    that appears at least twice at the tail of the history.
//!
//! When one triggers, [`check_for_doom_loop`] returns a corrective prompt that
//! the agent loop injects as a synthetic user message so the model breaks out.
//!
//! Port of `ml-intern/agent/core/doom_loop.py` — see
//! `docs/ml-intern-vs-hone.md` for the rationale.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use hone_proto::{Message, Role};

/// Default window of recent messages to scan.
pub const DEFAULT_LOOKBACK: usize = 30;
/// Default threshold for identical-consecutive detection.
pub const IDENTICAL_THRESHOLD: usize = 3;

/// Compact identifier for a single tool call, used to spot repetition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolCallSignature {
    pub name: String,
    pub args_hash: u64,
}

impl ToolCallSignature {
    pub fn new(name: impl Into<String>, arguments: &serde_json::Value) -> Self {
        Self {
            name: name.into(),
            args_hash: hash_args(arguments),
        }
    }
}

/// Stable hash of a JSON value. Uses `serde_json::to_string` which sorts map
/// keys alphabetically by default (since `serde_json::Map` is a `BTreeMap`
/// without the `preserve_order` feature), so the same arguments always hash
/// the same way.
///
/// Stability contract: these hashes are only guaranteed stable within a
/// *single process run*. `DefaultHasher`'s algorithm (currently SipHash-1-3)
/// is an implementation detail of the standard library and may change
/// between Rust releases or differ across targets.  That is fine here
/// because doom-loop detection only ever compares signatures against each
/// other inside the same running agent — we never persist these hashes.
fn hash_args(args: &serde_json::Value) -> u64 {
    let serialized = serde_json::to_string(args).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    serialized.hash(&mut hasher);
    hasher.finish()
}

/// Extract tool-call signatures from the last `lookback` messages, in order.
/// Only assistant messages that carry tool calls contribute.
pub fn extract_recent_tool_signatures(
    messages: &[Message],
    lookback: usize,
) -> Vec<ToolCallSignature> {
    let start = messages.len().saturating_sub(lookback);
    let mut out = Vec::new();
    for msg in &messages[start..] {
        if !matches!(msg.role, Role::Assistant) {
            continue;
        }
        for call in &msg.tool_calls {
            out.push(ToolCallSignature::new(&call.name, &call.arguments));
        }
    }
    out
}

/// Return the tool name if the last `threshold` signatures are all identical.
/// Equivalent to ml-intern's `detect_identical_consecutive`.
pub fn detect_identical_consecutive(
    signatures: &[ToolCallSignature],
    threshold: usize,
) -> Option<String> {
    if signatures.len() < threshold || threshold < 2 {
        return None;
    }
    let mut count = 1usize;
    for i in 1..signatures.len() {
        if signatures[i] == signatures[i - 1] {
            count += 1;
            if count >= threshold {
                return Some(signatures[i].name.clone());
            }
        } else {
            count = 1;
        }
    }
    None
}

/// Detect a repeating tail pattern: some window of length 2..=5 that shows up
/// at least twice in a row at the end of the signature stream.
pub fn detect_repeating_sequence(
    signatures: &[ToolCallSignature],
) -> Option<Vec<ToolCallSignature>> {
    let n = signatures.len();
    for seq_len in 2..=5 {
        let min_required = seq_len * 2;
        if n < min_required {
            continue;
        }
        let tail_start = n - min_required;
        let pattern = &signatures[tail_start..tail_start + seq_len];

        // Count full pattern repetitions walking backwards from the tail.
        let mut reps = 0usize;
        let mut start = n.saturating_sub(seq_len);
        loop {
            if start + seq_len > n {
                break;
            }
            let chunk = &signatures[start..start + seq_len];
            if chunk == pattern {
                reps += 1;
            } else {
                break;
            }
            if start < seq_len {
                break;
            }
            start -= seq_len;
        }

        if reps >= 2 {
            return Some(pattern.to_vec());
        }
    }
    None
}

/// Scan `messages` for a doom-loop pattern and, if found, return a corrective
/// prompt to inject as a synthetic user message before the next LLM call.
pub fn check_for_doom_loop(messages: &[Message]) -> Option<String> {
    let signatures = extract_recent_tool_signatures(messages, DEFAULT_LOOKBACK);
    if signatures.len() < IDENTICAL_THRESHOLD {
        return None;
    }

    if let Some(name) = detect_identical_consecutive(&signatures, IDENTICAL_THRESHOLD) {
        tracing::warn!(tool = %name, "doom loop detected: identical consecutive calls");
        return Some(format!(
            "[SYSTEM: DOOM LOOP DETECTED] You have called '{name}' with the same \
             arguments multiple times in a row, getting the same result each time. \
             STOP repeating this approach — it is not working. Step back and try a \
             fundamentally different strategy. Consider: using a different tool, \
             changing your arguments significantly, or telling the user what you \
             are stuck on and asking for guidance."
        ));
    }

    if let Some(pattern) = detect_repeating_sequence(&signatures) {
        let names: Vec<&str> = pattern.iter().map(|s| s.name.as_str()).collect();
        let desc = names.join(" → ");
        tracing::warn!(pattern = %desc, "doom loop detected: repeating sequence");
        return Some(format!(
            "[SYSTEM: DOOM LOOP DETECTED] You are stuck in a repeating cycle of tool \
             calls: [{desc}]. This pattern has repeated multiple times without \
             progress. STOP this cycle and try a fundamentally different approach. \
             Consider breaking the problem down differently, using alternative \
             tools, or telling the user what you are stuck on."
        ));
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hone_proto::{Content, Message, Role, ToolCallRequest};
    use serde_json::json;

    fn asst_with_tools(calls: &[(&str, serde_json::Value)]) -> Message {
        Message {
            role: Role::Assistant,
            content: Content::Text(String::new()),
            tool_calls: calls
                .iter()
                .enumerate()
                .map(|(i, (name, args))| ToolCallRequest {
                    id: format!("tc_{i}"),
                    name: (*name).to_string(),
                    arguments: args.clone(),
                })
                .collect(),
            tool_call_id: None,
        }
    }

    fn sig(name: &str, args: serde_json::Value) -> ToolCallSignature {
        ToolCallSignature::new(name, &args)
    }

    #[test]
    fn identical_args_produce_identical_hashes() {
        let a = hash_args(&json!({"path": "a.rs", "n": 1}));
        let b = hash_args(&json!({"path": "a.rs", "n": 1}));
        assert_eq!(a, b);
    }

    #[test]
    fn different_args_produce_different_hashes() {
        let a = hash_args(&json!({"path": "a.rs"}));
        let b = hash_args(&json!({"path": "b.rs"}));
        assert_ne!(a, b);
    }

    #[test]
    fn map_key_order_does_not_affect_hash() {
        // serde_json::Map is a BTreeMap by default — keys are always sorted
        // at serialization time.
        let a = hash_args(&json!({"a": 1, "b": 2}));
        let b = hash_args(&json!({"b": 2, "a": 1}));
        assert_eq!(a, b);
    }

    #[test]
    fn extract_ignores_non_assistant_messages() {
        let messages = vec![
            Message::user("hi"),
            asst_with_tools(&[("read_file", json!({"p": 1}))]),
            Message::tool_result("tc_0".into(), "ok"),
            Message::user("continue"),
        ];
        let sigs = extract_recent_tool_signatures(&messages, 30);
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].name, "read_file");
    }

    #[test]
    fn identical_consecutive_fires_on_three_in_a_row() {
        let signatures = vec![
            sig("read_file", json!({"p": "a"})),
            sig("read_file", json!({"p": "a"})),
            sig("read_file", json!({"p": "a"})),
        ];
        assert_eq!(
            detect_identical_consecutive(&signatures, 3),
            Some("read_file".into())
        );
    }

    #[test]
    fn identical_consecutive_does_not_fire_on_two() {
        let signatures = vec![
            sig("read_file", json!({"p": "a"})),
            sig("read_file", json!({"p": "a"})),
        ];
        assert_eq!(detect_identical_consecutive(&signatures, 3), None);
    }

    #[test]
    fn identical_consecutive_resets_on_different_call() {
        let signatures = vec![
            sig("read_file", json!({"p": "a"})),
            sig("read_file", json!({"p": "a"})),
            sig("edit_file", json!({"p": "a"})),
            sig("read_file", json!({"p": "a"})),
        ];
        // Streak broken by edit_file; no 3 in a row.
        assert_eq!(detect_identical_consecutive(&signatures, 3), None);
    }

    #[test]
    fn repeating_sequence_detects_ab_ab() {
        let signatures = vec![
            sig("read_file", json!({"p": "a"})),
            sig("edit_file", json!({"p": "a"})),
            sig("read_file", json!({"p": "a"})),
            sig("edit_file", json!({"p": "a"})),
        ];
        let result = detect_repeating_sequence(&signatures);
        let pattern = result.expect("pattern should fire");
        assert_eq!(pattern.len(), 2);
        assert_eq!(pattern[0].name, "read_file");
        assert_eq!(pattern[1].name, "edit_file");
    }

    #[test]
    fn repeating_sequence_needs_at_least_two_full_reps() {
        // Only one full rep of [read, edit] at the tail.
        let signatures = vec![
            sig("list", json!({})),
            sig("read_file", json!({"p": "a"})),
            sig("edit_file", json!({"p": "a"})),
        ];
        assert!(detect_repeating_sequence(&signatures).is_none());
    }

    #[test]
    fn check_returns_prompt_for_identical_consecutive() {
        let messages = vec![
            asst_with_tools(&[("read_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r1"),
            asst_with_tools(&[("read_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r2"),
            asst_with_tools(&[("read_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r3"),
        ];
        let prompt = check_for_doom_loop(&messages).expect("should fire");
        assert!(prompt.contains("DOOM LOOP DETECTED"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn check_returns_none_for_healthy_history() {
        let messages = vec![
            asst_with_tools(&[("read_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r1"),
            asst_with_tools(&[("edit_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r2"),
            asst_with_tools(&[("run_tests", json!({}))]),
            Message::tool_result("tc_0".into(), "r3"),
        ];
        assert!(check_for_doom_loop(&messages).is_none());
    }

    #[test]
    fn check_returns_prompt_for_repeating_sequence() {
        let messages = vec![
            asst_with_tools(&[("read_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r1"),
            asst_with_tools(&[("edit_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r2"),
            asst_with_tools(&[("read_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r3"),
            asst_with_tools(&[("edit_file", json!({"p": "a"}))]),
            Message::tool_result("tc_0".into(), "r4"),
        ];
        let prompt = check_for_doom_loop(&messages).expect("should fire");
        assert!(prompt.contains("repeating cycle"));
        assert!(prompt.contains("read_file"));
        assert!(prompt.contains("edit_file"));
    }
}
