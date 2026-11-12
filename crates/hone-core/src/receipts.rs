//! Building [`ContextReceipt`]s for the agent loop.
//!
//! A receipt names what a model call was given: ids, paths, hashes, names,
//! scores and counts, never text. See `docs/plans/context-receipts-spec.md`.

use hone_proto::{
    BlockReceipt, CompactionStep, ContextReceipt, Message, Role, ScoredSource, Slot, SourceRef,
    StrategyId, canonical_json_sha256, request_sha256, sha256_hex,
};

use crate::agent::{RECALL_LIMIT, RECALL_RRF_K, SEMANTIC_RECALL_FETCH};
use crate::context::estimate_text_tokens;

/// What recall did for the current turn.
#[derive(Debug, Clone, Default)]
pub(crate) struct RecallInfo {
    /// Recalled memory ids, in prompt order.
    pub ids: Vec<String>,
    /// An embedder ran, so the result is a BM25 + semantic fusion.
    pub semantic: bool,
    /// Candidates that were fetched and then cut.
    pub dropped: Vec<String>,
    pub duration_ms: u32,
}

/// What turn-1 context injection added.
#[derive(Debug, Clone)]
pub(crate) struct InjectionInfo {
    /// `SkeletonStrategy::as_str`.
    pub strategy: String,
    pub tokens: u32,
    /// Tier-2 hits: (path, score), best first.
    pub hits: Vec<(String, f32)>,
}

/// The per-turn inputs of the context blocks (the history comes from the
/// context manager and the request itself).
#[derive(Debug, Clone, Default)]
pub(crate) struct TurnContextInfo {
    pub recall: RecallInfo,
    pub skills: Vec<String>,
    pub injection: Option<InjectionInfo>,
}

fn strategy(slot: Slot, name: &str, params: &serde_json::Value) -> StrategyId {
    StrategyId {
        slot,
        name: name.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        params_sha256: canonical_json_sha256(params),
    }
}

fn block(
    slot: Slot,
    name: &str,
    params: &serde_json::Value,
    tokens: u32,
    sources: Vec<ScoredSource>,
    duration_ms: u32,
) -> BlockReceipt {
    BlockReceipt {
        slot,
        strategy: strategy(slot, name, params),
        tokens,
        sources,
        duration_ms,
    }
}

fn unscored(source: SourceRef) -> ScoredSource {
    ScoredSource {
        source,
        score: None,
    }
}

/// The receipt for the request made of `messages` (as sent, system prompt
/// first) and `tool_names`. `recall_at` and `skills_at` are the positions of
/// the recall and skills system messages in `messages`, when present.
///
/// `hashes` holds the hash of every message of `messages` (cached by the
/// context manager). The `History` block lists only the non-system messages
/// whose hash is not in `prev` (the previous receipt's), then `prev` is set to
/// the hashes of this request.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_receipt(
    turn: u32,
    call: u32,
    model: &str,
    messages: &[Message],
    tool_names: &[String],
    recall_at: Option<usize>,
    skills_at: Option<usize>,
    info: &TurnContextInfo,
    compaction: Vec<CompactionStep>,
    hashes: &[String],
    prev: &mut std::collections::HashSet<String>,
) -> ContextReceipt {
    let tokens_of = |i: usize| {
        messages
            .get(i)
            .map_or(0, |m| estimate_text_tokens(&m.content.as_text()))
    };

    let mut blocks = Vec::new();
    blocks.push(block(
        Slot::SystemPrompt,
        "system_prompt",
        &serde_json::json!({}),
        tokens_of(0),
        vec![],
        0,
    ));

    if let Some(at) = recall_at {
        let (name, params) = if info.recall.semantic {
            (
                "bm25+semantic:rrf",
                serde_json::json!({
                    "exclude": ["outcome"],
                    "limit": RECALL_LIMIT,
                    "rrf_k": RECALL_RRF_K,
                    "semantic_fetch": SEMANTIC_RECALL_FETCH,
                }),
            )
        } else {
            (
                "bm25",
                serde_json::json!({ "exclude": ["outcome"], "limit": RECALL_LIMIT }),
            )
        };
        let sources = info
            .recall
            .ids
            .iter()
            .map(|id| {
                unscored(SourceRef::Memory {
                    id: id.clone(),
                    version: None,
                })
            })
            .collect();
        blocks.push(block(
            Slot::Recall,
            name,
            &params,
            tokens_of(at),
            sources,
            info.recall.duration_ms,
        ));
    }

    if let Some(at) = skills_at {
        let sources = info
            .skills
            .iter()
            .map(|name| unscored(SourceRef::Skill { name: name.clone() }))
            .collect();
        blocks.push(block(
            Slot::Skills,
            "skills:trigger",
            &serde_json::json!({}),
            tokens_of(at),
            sources,
            0,
        ));
    }

    if let Some(inj) = &info.injection {
        let sources = inj
            .hits
            .iter()
            .map(|(path, score)| ScoredSource {
                source: SourceRef::File {
                    path: std::path::PathBuf::from(path),
                    sha256: None,
                    lines: None,
                },
                score: Some(*score),
            })
            .collect();
        blocks.push(block(
            Slot::Injection,
            &format!("skeleton:{}", inj.strategy),
            &serde_json::json!({ "strategy": inj.strategy }),
            inj.tokens,
            sources,
            0,
        ));
    }

    // History: the non-system messages of the request that are new or changed
    // since the previous receipt, with their request positions.
    let mut history_tokens = 0u32;
    let mut sources = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for (index, m) in messages.iter().enumerate() {
        if m.role == Role::System {
            continue;
        }
        let Some(hash) = hashes.get(index) else {
            continue;
        };
        history_tokens = history_tokens.saturating_add(estimate_text_tokens(&m.content.as_text()));
        current.push(hash);
        if !prev.contains(hash) {
            sources.push(unscored(SourceRef::Message {
                index,
                sha256: hash.clone(),
            }));
        }
    }
    let history_len = current.len() as u32;
    let history_root = sha256_hex(current.join("\n").as_bytes());
    *prev = current.into_iter().map(str::to_owned).collect();
    if history_len > 0 {
        blocks.push(block(
            Slot::History,
            "history",
            &serde_json::json!({}),
            history_tokens,
            sources,
            0,
        ));
    }

    let dropped = info
        .recall
        .dropped
        .iter()
        .map(|id| SourceRef::Memory {
            id: id.clone(),
            version: None,
        })
        .collect();

    ContextReceipt {
        turn,
        call,
        model: model.to_owned(),
        request_sha256: request_sha256(messages, tool_names),
        blocks,
        dropped,
        compaction,
        tools_offered: tool_names.to_vec(),
        history_len,
        history_root,
    }
}
