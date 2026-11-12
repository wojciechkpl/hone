//! The `retrieval` context source: the workspace's notes and docs, retrieved
//! for each turn's prompt from the document index (`hone_index::DocIndex`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hone_index::{Chunker, DocIndex, FixedChunker, MarkdownChunker};
use hone_proto::{ScoredSource, Slot, SourceRef, StrategyId, canonical_json_sha256};

use crate::context::estimate_text_tokens;
use crate::strategy::{
    Cadence, ContextBlock, ContextSource, StrategyDeps, StrategyError, TurnView,
};

/// How long a turn waits for the index to catch up with the files.
const SYNC_TIMEOUT: Duration = Duration::from_secs(2);

/// What retrieval added to one turn.
#[derive(Debug, Clone)]
pub(crate) struct RetrievalOutcome {
    /// The whole system message, starting with `# Relevant documents`.
    pub text: String,
    pub id: StrategyId,
    pub sources: Vec<ScoredSource>,
    /// Hits that did not fit the token budget.
    pub dropped: Vec<SourceRef>,
}

pub(crate) struct RetrievalSource {
    id: StrategyId,
    top_k: usize,
    max_tokens: u32,
    chunker: String,
    fixed_lines: usize,
    fixed_overlap: usize,
    /// The document database file; `None` = in memory.
    db_path: Option<PathBuf>,
    index: tokio::sync::OnceCell<Option<Arc<DocIndex>>>,
}

impl RetrievalSource {
    /// From the factory parameters `{top_k, max_tokens, chunker, lines, overlap}`.
    pub fn from_params(params: &serde_json::Value, deps: &StrategyDeps) -> Self {
        let top_k = params["top_k"].as_u64().unwrap_or(3) as usize;
        let max_tokens = params["max_tokens"].as_u64().unwrap_or(1200) as u32;
        let chunker = params["chunker"].as_str().unwrap_or("markdown").to_owned();
        let receipt_params =
            serde_json::json!({ "chunker": chunker, "max_tokens": max_tokens, "top_k": top_k });
        Self {
            id: StrategyId {
                slot: Slot::Retrieval,
                name: "fts5-docs".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                params_sha256: canonical_json_sha256(&receipt_params),
            },
            top_k,
            max_tokens,
            chunker,
            fixed_lines: params["lines"].as_u64().unwrap_or(40) as usize,
            fixed_overlap: params["overlap"].as_u64().unwrap_or(5) as usize,
            db_path: deps.docs_db.clone(),
            index: tokio::sync::OnceCell::new(),
        }
    }

    pub fn strategy_id(&self) -> &StrategyId {
        &self.id
    }

    async fn index(&self) -> Option<Arc<DocIndex>> {
        self.index
            .get_or_init(|| async {
                let built = match &self.db_path {
                    Some(path) => DocIndex::open(path).await,
                    None => DocIndex::in_memory().await,
                };
                match built {
                    Ok(idx) => {
                        let fixed: Arc<dyn Chunker> = Arc::new(FixedChunker {
                            lines: self.fixed_lines,
                            overlap: self.fixed_overlap,
                        });
                        let markdown: Arc<dyn Chunker> = if self.chunker == "fixed" {
                            Arc::clone(&fixed)
                        } else {
                            Arc::new(MarkdownChunker::new())
                        };
                        Some(Arc::new(idx.with_chunkers(markdown, fixed)))
                    }
                    Err(e) => {
                        tracing::warn!("document index unavailable: {e}");
                        None
                    }
                }
            })
            .await
            .clone()
    }

    /// Bring the index up to date (bounded) and retrieve for `prompt`. `None`
    /// when there is nothing to add.
    pub async fn retrieve(&self, prompt: &str, working_dir: &Path) -> Option<RetrievalOutcome> {
        let index = self.index().await?;
        match tokio::time::timeout(SYNC_TIMEOUT, index.sync(working_dir)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!("document sync failed: {e}"),
            Err(_) => tracing::warn!("document sync timed out; using what is indexed"),
        }
        let hits = match index.retriever().retrieve_located(prompt, self.top_k).await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("document retrieval failed: {e}");
                return None;
            }
        };

        let mut body = String::new();
        let mut used: u32 = 0;
        let mut sources = Vec::new();
        let mut dropped = Vec::new();
        for h in hits {
            let mut entry = format!("{}:{}-{}", h.path, h.start_line, h.end_line);
            if let Some(heading) = &h.heading {
                entry.push_str(&format!(" \u{2014} {heading}"));
            }
            entry.push('\n');
            entry.push_str(&h.hit.text);
            let cost = estimate_text_tokens(&entry);
            if used.saturating_add(cost) > self.max_tokens {
                dropped.push(h.hit.source);
                continue;
            }
            used += cost;
            body.push_str("\n\n");
            body.push_str(&entry);
            sources.push(ScoredSource {
                source: h.hit.source,
                score: Some(h.hit.score),
            });
        }
        if sources.is_empty() {
            return None;
        }
        Some(RetrievalOutcome {
            text: format!("# Relevant documents{body}"),
            id: self.id.clone(),
            sources,
            dropped,
        })
    }
}

#[async_trait]
impl ContextSource for RetrievalSource {
    fn id(&self) -> &StrategyId {
        &self.id
    }

    fn cadence(&self) -> Cadence {
        Cadence::EachTurn
    }

    async fn gather(&self, view: &TurnView<'_>) -> Result<Vec<ContextBlock>, StrategyError> {
        Ok(self
            .retrieve(view.prompt, view.working_dir)
            .await
            .map(|o| ContextBlock {
                slot: Slot::Retrieval,
                text: o.text,
                sources: o.sources,
            })
            .into_iter()
            .collect())
    }
}
