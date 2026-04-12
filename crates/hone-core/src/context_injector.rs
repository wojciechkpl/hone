//! Proactive context injection for the Hone agent loop.
//!
//! Implements a three-tier injection strategy that runs before the first
//! provider call of a new task:
//!
//! - **Tier 1** (always): Repo skeleton — file tree + function/type signatures
//!   extracted by tree-sitter. Target 800–1 500 tokens.
//! - **Tier 2** (confidence-gated): Top-matching function signatures from BM25
//!   retrieval. Only injected when the score gap between rank-1 and rank-2
//!   results exceeds the configured threshold (HIGH confidence).
//! - **Tier 3** (always): Exploration budget hint appended to the injected
//!   block, telling the agent how many tool calls retrieval suggests are
//!   needed.
//!
//! The system is enabled only when `HONE_CONTEXT_INJECTION=1` or the
//! `--inject-context` CLI flag is passed, so existing behaviour is unchanged
//! by default.
//!
//! ## Skeleton strategies
//!
//! Four strategies control how the Tier 1 skeleton is formatted:
//!
//! - **Flat** (original): One line per file with symbol signatures.
//! - **DependencyGraph**: Flat signatures plus import-dependency arrows.
//! - **CallGraph**: Function-level call relationships for the whole repo.
//! - **Hybrid** (default): Dependency arrows for all files; call details for
//!   the top-5 BM25-relevant files only.
//!
//! Select via the `HONE_SKELETON_STRATEGY` env var or `--skeleton-strategy`
//! CLI flag.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use hone_index::{SqliteIndex, SymbolExtractor};
use ignore::WalkBuilder;
use tiktoken_rs::CoreBPE;
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Token counting
// ---------------------------------------------------------------------------

fn bpe() -> &'static CoreBPE {
    static BPE: OnceLock<CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().expect("cl100k_base BPE must load"))
}

fn count_tokens(text: &str) -> u32 {
    bpe().encode_ordinary(&text.to_string()).len() as u32
}

// ---------------------------------------------------------------------------
// SkeletonStrategy
// ---------------------------------------------------------------------------

/// Controls how the Tier 1 repository skeleton is formatted.
///
/// Select via `HONE_SKELETON_STRATEGY` env var or `--skeleton-strategy` CLI flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkeletonStrategy {
    /// Flat file list with signatures (original, cheapest).
    Flat,
    /// File list with import dependency arrows (`→ [mod1, mod2]`).
    DependencyGraph,
    /// Function-level call graph (most detailed, most tokens). Capped at 30 functions.
    CallGraph,
    /// Dependency graph + call details for top-5 BM25-relevant files (default).
    #[default]
    Hybrid,
}

impl SkeletonStrategy {
    /// Parse from an env-var / CLI string (case-insensitive).
    ///
    /// Returns `None` when the string is not recognised so the caller can
    /// fall back to the default or emit an error.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "flat" => Some(Self::Flat),
            "dependency_graph" | "dependency-graph" | "deps" => Some(Self::DependencyGraph),
            "call_graph" | "call-graph" | "calls" => Some(Self::CallGraph),
            "hybrid" => Some(Self::Hybrid),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API types
// ---------------------------------------------------------------------------

/// Confidence level derived from BM25 score distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalConfidence {
    /// Score gap > 0.4 — clear winner. Inject signatures, cap exploration at 3.
    High,
    /// Score gap 0.15–0.4 — ambiguous. Skeleton only, cap exploration at 8.
    Medium,
    /// Score gap < 0.15 — no clear signal. Skeleton only, cap exploration at 15.
    Low,
}

impl RetrievalConfidence {
    /// Suggested upper bound on tool calls (soft cap, not enforced by the agent).
    pub fn exploration_budget(self) -> u32 {
        match self {
            Self::High => 3,
            Self::Medium => 8,
            Self::Low => 15,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// The output of a single context injection run.
#[derive(Debug)]
pub struct InjectionResult {
    /// Text to prepend to the user's message (empty when injection is skipped).
    pub context_block: String,
    /// Estimated token count of `context_block`.
    pub tokens_used: u32,
    /// Confidence level determined from BM25 retrieval.
    pub confidence: RetrievalConfidence,
    /// Suggested exploration budget (tool call soft cap).
    pub exploration_budget: u32,
}

impl InjectionResult {
    #[allow(dead_code)]
    fn empty() -> Self {
        Self {
            context_block: String::new(),
            tokens_used: 0,
            confidence: RetrievalConfidence::Low,
            exploration_budget: RetrievalConfidence::Low.exploration_budget(),
        }
    }
}

// ---------------------------------------------------------------------------
// ContextInjector
// ---------------------------------------------------------------------------

/// Cached repository skeleton with mtime-based invalidation.
///
/// The skeleton is expensive to generate (tree-sitter parsing of every file),
/// so we cache it and only regenerate when the working directory's contents
/// have changed. Staleness is detected by tracking the most recent mtime
/// across all source files at generation time.
#[derive(Debug)]
struct SkeletonCache {
    /// The cached skeleton string.
    skeleton: String,
    /// Newest mtime (as secs since epoch) seen when the skeleton was built.
    newest_mtime: u64,
    /// Working directory the skeleton was built for.
    root: std::path::PathBuf,
    /// Strategy used to produce this skeleton (different strategy → cache miss).
    strategy: SkeletonStrategy,
}

impl SkeletonCache {
    /// Check if the cache is still valid by sampling mtimes.
    ///
    /// Instead of scanning every file (expensive), we check a sample of
    /// up to 20 source files. If any has a newer mtime than when the
    /// skeleton was built, the cache is stale.
    fn is_fresh(&self) -> bool {
        let walker = WalkBuilder::new(&self.root)
            .hidden(true)
            .ignore(true)
            .git_ignore(true)
            .build();

        let mut checked = 0u32;
        for entry in walker {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "rs" | "py" | "ts" | "js" | "go" | "java" | "c" | "cpp" | "h") {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(path) {
                if let Ok(mtime) = meta.modified() {
                    let secs = mtime
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    if secs > self.newest_mtime {
                        return false; // stale
                    }
                }
            }
            checked += 1;
            if checked >= 20 {
                break;
            }
        }
        true
    }
}

/// Core injection engine.
///
/// Construct once per session and call [`ContextInjector::inject`] before each
/// `run_turn` invocation. The skeleton is cached and automatically refreshed
/// when source files change (mtime-based invalidation).
pub struct ContextInjector {
    /// Maximum tokens the entire injected block may consume.
    max_injection_tokens: u32,
    /// BM25 score-gap ratio above which tier-2 snippet injection fires.
    confidence_threshold: f32,
    /// Cached repo skeleton (refreshed on file changes).
    skeleton_cache: Option<SkeletonCache>,
    /// Active skeleton generation strategy.
    pub strategy: SkeletonStrategy,
}

impl ContextInjector {
    /// Create an injector with explicit limits and strategy.
    pub fn new(max_injection_tokens: u32, confidence_threshold: f32) -> Self {
        let strategy = std::env::var("HONE_SKELETON_STRATEGY")
            .ok()
            .and_then(|v| SkeletonStrategy::from_str(&v))
            .unwrap_or_default();
        Self {
            max_injection_tokens,
            confidence_threshold,
            skeleton_cache: None,
            strategy,
        }
    }

    /// Create with sensible defaults (2 000 token budget, 0.4 gap threshold).
    pub fn with_defaults() -> Self {
        Self::new(2_000, 0.4)
    }

    /// Override the skeleton strategy, ignoring the env var.
    pub fn with_strategy(mut self, strategy: SkeletonStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    // -----------------------------------------------------------------------
    // Primary entry point
    // -----------------------------------------------------------------------

    /// Build the injection block for `query` rooted at `working_dir`.
    ///
    /// `index` may be `None` when the SQLite index has not been opened for
    /// this session; in that case tier-2 is skipped and only the skeleton is
    /// returned.
    pub async fn inject(
        &mut self,
        query: &str,
        working_dir: &Path,
        index: Option<&SqliteIndex>,
    ) -> InjectionResult {
        // --- Tier 1: repo skeleton (cached, refreshed on file changes) -------
        let skeleton = self.get_or_refresh_skeleton(working_dir, query, index).await;
        let mut block = format!("## Repository Structure\n{skeleton}");

        // --- Tier 2: confidence-gated snippet injection ----------------------
        let confidence = if let Some(idx) = index {
            self.inject_snippets(query, idx, &mut block).await
        } else {
            RetrievalConfidence::Low
        };

        // --- Tier 3: exploration budget hint ---------------------------------
        let budget = confidence.exploration_budget();
        block.push_str(&format!(
            "\n\n(retrieval confidence: {} — {})",
            confidence.label(),
            match confidence {
                RetrievalConfidence::High =>
                    "the relevant code is likely in the files above",
                RetrievalConfidence::Medium =>
                    "explore further; several files may be relevant",
                RetrievalConfidence::Low =>
                    "no clear signal; use your judgment to explore",
            }
        ));

        let tokens_used = count_tokens(&block);

        // Trim to budget if needed (rare; skeleton + hints are small).
        let block = if tokens_used > self.max_injection_tokens {
            trim_to_token_budget(&block, self.max_injection_tokens)
        } else {
            block
        };

        let tokens_used = count_tokens(&block);

        InjectionResult {
            context_block: block,
            tokens_used,
            confidence,
            exploration_budget: budget,
        }
    }

    // -----------------------------------------------------------------------
    // Tier 2 helper
    // -----------------------------------------------------------------------

    /// Score all files in the index against `query` using BM25, compute the
    /// confidence level, and append relevant signatures to `block` when
    /// confidence is HIGH.
    ///
    /// Returns the computed [`RetrievalConfidence`].
    async fn inject_snippets(
        &self,
        query: &str,
        index: &SqliteIndex,
        block: &mut String,
    ) -> RetrievalConfidence {
        // Search the index for the top symbols matching the query.
        let results = match index.search(query, 10).await {
            Ok(r) => r,
            Err(_) => return RetrievalConfidence::Low,
        };

        if results.is_empty() {
            return RetrievalConfidence::Low;
        }

        // Collect raw scores.
        let scores: Vec<f32> = results.iter().map(|r| r.score as f32).collect();
        let confidence = self.compute_confidence(&scores);

        if confidence == RetrievalConfidence::High {
            // Inject top-3 symbol signatures (names + kinds, not full bodies).
            let top: Vec<String> = results
                .iter()
                .take(3)
                .map(|r| {
                    format!(
                        "{} ({}) in {}",
                        r.symbol.name,
                        symbol_kind_label(r.symbol.kind),
                        r.symbol.file.display()
                    )
                })
                .collect();

            block.push_str(
                "\n\n## Potentially relevant code (for reference, explore further if needed):\n",
            );
            for sig in &top {
                block.push_str(&format!("- {sig}\n"));
            }
        }

        confidence
    }

    /// Compute `RetrievalConfidence` from a slice of BM25 scores (descending).
    fn compute_confidence(&self, scores: &[f32]) -> RetrievalConfidence {
        if scores.is_empty() {
            return RetrievalConfidence::Low;
        }

        let s0 = scores[0];
        if s0 <= 0.0 {
            return RetrievalConfidence::Low;
        }

        let gap_ratio = if scores.len() > 1 {
            (s0 - scores[1]) / s0
        } else {
            // Single result is a clear winner.
            1.0
        };

        if gap_ratio > self.confidence_threshold {
            RetrievalConfidence::High
        } else if gap_ratio > 0.15 {
            RetrievalConfidence::Medium
        } else {
            RetrievalConfidence::Low
        }
    }

    // -----------------------------------------------------------------------
    // Skeleton cache dispatch
    // -----------------------------------------------------------------------

    /// Get the cached skeleton or regenerate if files have changed or strategy differs.
    async fn get_or_refresh_skeleton(
        &mut self,
        root: &Path,
        query: &str,
        index: Option<&SqliteIndex>,
    ) -> String {
        // Cache miss when: no cache, different root, different strategy, or stale mtimes.
        let needs_refresh = match &self.skeleton_cache {
            Some(cache) if cache.root == root && cache.strategy == self.strategy => {
                !cache.is_fresh()
            }
            _ => true,
        };

        if needs_refresh {
            tracing::debug!("skeleton cache miss — regenerating");
            let (skeleton, newest_mtime) =
                self.skeleton_from_strategy_with_mtime(root, query, index).await;
            self.skeleton_cache = Some(SkeletonCache {
                skeleton: skeleton.clone(),
                newest_mtime,
                root: root.to_path_buf(),
                strategy: self.strategy,
            });
            skeleton
        } else {
            tracing::debug!("skeleton cache hit — reusing");
            self.skeleton_cache.as_ref().unwrap().skeleton.clone()
        }
    }

    async fn skeleton_from_strategy_with_mtime(
        &self,
        root: &Path,
        query: &str,
        index: Option<&SqliteIndex>,
    ) -> (String, u64) {
        let mut newest_mtime = 0u64;
        let skeleton = match self.strategy {
            SkeletonStrategy::Flat => self.skeleton_from_dir_inner(root, &mut newest_mtime),
            SkeletonStrategy::DependencyGraph => {
                self.skeleton_dependency_graph_inner(root, &mut newest_mtime)
            }
            SkeletonStrategy::CallGraph => {
                self.skeleton_call_graph_inner(root, &mut newest_mtime)
            }
            SkeletonStrategy::Hybrid => {
                self.skeleton_hybrid_inner(root, query, index, &mut newest_mtime)
                    .await
            }
        };
        (skeleton, newest_mtime)
    }

    // -----------------------------------------------------------------------
    // Strategy 1 — Flat (original)
    // -----------------------------------------------------------------------

    /// Generate a compact repo skeleton: one line per file with its symbols.
    ///
    /// Respects `.gitignore` via the `ignore` crate. Caps at 50 files.
    /// Returns a multi-line string formatted as:
    /// ```text
    /// src/auth.rs: fn login(), fn logout(), struct Session
    /// src/main.rs: fn main()
    /// ```
    pub fn skeleton_from_dir(&self, root: &Path) -> String {
        let mut _mtime = 0u64;
        self.skeleton_from_dir_inner(root, &mut _mtime)
    }

    fn skeleton_from_dir_inner(&self, root: &Path, newest_mtime: &mut u64) -> String {
        let mut extractor = SymbolExtractor::new();
        let mut lines: Vec<String> = Vec::new();
        let mut file_count = 0usize;

        let walker = WalkBuilder::new(root)
            .hidden(true)        // skip hidden files/dirs
            .ignore(true)        // respect .ignore files
            .git_ignore(true)    // respect .gitignore
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();

            // Skip build artefact directories.
            if path_contains_segment(path, "target")
                || path_contains_segment(path, "__pycache__")
                || path_contains_segment(path, "node_modules")
                || path_contains_segment(path, ".git")
            {
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "rs" | "py" | "ts" | "js" | "go") {
                continue;
            }

            if file_count >= 50 {
                break;
            }

            // Track mtime for cache invalidation.
            if let Ok(meta) = std::fs::metadata(path) {
                if let Ok(mtime) = meta.modified() {
                    let secs = mtime
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    if secs > *newest_mtime {
                        *newest_mtime = secs;
                    }
                }
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let symbols = extractor.extract(path, &content);

            let rel = path.strip_prefix(root).unwrap_or(path);

            let sig = if symbols.is_empty() {
                format!("{}", rel.display())
            } else {
                let sig_names: Vec<String> = symbols
                    .iter()
                    .take(10)
                    .map(|s| format!("{} {}()", symbol_kind_prefix(s.kind), s.name))
                    .collect();
                format!("{}: {}", rel.display(), sig_names.join(", "))
            };

            lines.push(sig);
            file_count += 1;
        }

        lines.join("\n")
    }

    // -----------------------------------------------------------------------
    // Strategy 2 — Dependency Graph
    // -----------------------------------------------------------------------

    /// Generate a skeleton that shows file-level dependency arrows.
    ///
    /// Format:
    /// ```text
    /// src/main.rs: fn main() → [auth, api, db]
    /// src/auth.rs: fn login(), fn logout() → [db, crypto]
    /// src/db.rs: fn connect(), fn query()
    /// ```
    pub fn skeleton_dependency_graph(&self, root: &Path) -> String {
        let mut mtime = 0u64;
        self.skeleton_dependency_graph_inner(root, &mut mtime)
    }

    fn skeleton_dependency_graph_inner(&self, root: &Path, newest_mtime: &mut u64) -> String {
        let mut extractor = SymbolExtractor::new();
        let mut lines: Vec<String> = Vec::new();
        let mut file_count = 0usize;

        let walker = WalkBuilder::new(root)
            .hidden(true)
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            if path_contains_segment(path, "target")
                || path_contains_segment(path, "__pycache__")
                || path_contains_segment(path, "node_modules")
                || path_contains_segment(path, ".git")
            {
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "rs" | "py" | "ts" | "js" | "go") {
                continue;
            }

            if file_count >= 50 {
                break;
            }

            update_mtime(path, newest_mtime);

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let symbols = extractor.extract(path, &content);
            let deps = parse_dependencies(path, &content);
            let rel = path.strip_prefix(root).unwrap_or(path);

            let sym_part = if symbols.is_empty() {
                String::new()
            } else {
                let names: Vec<String> = symbols
                    .iter()
                    .take(8)
                    .map(|s| format!("{} {}()", symbol_kind_prefix(s.kind), s.name))
                    .collect();
                names.join(", ")
            };

            let dep_part = if deps.is_empty() {
                String::new()
            } else {
                format!(" → [{}]", deps.join(", "))
            };

            let line = match (sym_part.is_empty(), dep_part.is_empty()) {
                (true, true) => format!("{}", rel.display()),
                (true, false) => format!("{}{}", rel.display(), dep_part),
                (false, true) => format!("{}: {}", rel.display(), sym_part),
                (false, false) => format!("{}: {}{}", rel.display(), sym_part, dep_part),
            };

            lines.push(line);
            file_count += 1;
        }

        lines.join("\n")
    }

    // -----------------------------------------------------------------------
    // Strategy 3 — Call Graph
    // -----------------------------------------------------------------------

    /// Generate a skeleton that traces function-level call relationships.
    ///
    /// Format:
    /// ```text
    /// src/auth.rs:
    ///   fn login(user, password) → calls [connect, hash_password, create]
    ///   fn logout(session) → calls [destroy]
    /// src/db.rs:
    ///   fn connect(url) → calls []
    /// ```
    ///
    /// Caps at 30 functions to stay within token budget.
    pub fn skeleton_call_graph(&self, root: &Path) -> String {
        let mut mtime = 0u64;
        self.skeleton_call_graph_inner(root, &mut mtime)
    }

    fn skeleton_call_graph_inner(&self, root: &Path, newest_mtime: &mut u64) -> String {
        // First pass: collect all known symbol names across the repo.
        let known_symbols = collect_known_symbols(root);

        let mut extractor = SymbolExtractor::new();
        let mut sections: Vec<String> = Vec::new();
        let mut total_fns = 0usize;
        const MAX_FUNCTIONS: usize = 30;

        let walker = WalkBuilder::new(root)
            .hidden(true)
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        'outer: for entry in walker {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            if path_contains_segment(path, "target")
                || path_contains_segment(path, "__pycache__")
                || path_contains_segment(path, "node_modules")
                || path_contains_segment(path, ".git")
            {
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "rs" | "py" | "ts" | "js" | "go") {
                continue;
            }

            update_mtime(path, newest_mtime);

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let symbols = extractor.extract(path, &content);
            if symbols.is_empty() {
                continue;
            }

            let rel = path.strip_prefix(root).unwrap_or(path);
            let mut fn_lines: Vec<String> = Vec::new();

            for sym in &symbols {
                use hone_index::SymbolKind;
                if sym.kind != SymbolKind::Function {
                    continue;
                }
                if total_fns >= MAX_FUNCTIONS {
                    break 'outer;
                }

                let calls = extract_calls(&content, &known_symbols);
                let call_list = if calls.is_empty() {
                    String::new()
                } else {
                    format!(" → calls [{}]", calls.join(", "))
                };
                fn_lines.push(format!("  fn {}{}", sym.name, call_list));
                total_fns += 1;
            }

            if !fn_lines.is_empty() {
                let mut section = format!("{}:", rel.display());
                for fl in fn_lines {
                    section.push('\n');
                    section.push_str(&fl);
                }
                sections.push(section);
            }
        }

        sections.join("\n")
    }

    // -----------------------------------------------------------------------
    // Strategy 4 — Hybrid (default)
    // -----------------------------------------------------------------------

    /// Generate the hybrid skeleton: dependency arrows for all files, plus
    /// function-level call details for the top-5 BM25-relevant files.
    ///
    /// When no `index` is available (or the query is empty), falls back to
    /// the dependency graph skeleton.
    pub fn skeleton_hybrid<'a>(
        &self,
        root: &Path,
        _query: &str,
        top_files: &[&'a Path],
    ) -> String {
        let mut mtime = 0u64;
        self.skeleton_hybrid_with_top_files(root, top_files, &mut mtime)
    }

    async fn skeleton_hybrid_inner(
        &self,
        root: &Path,
        query: &str,
        index: Option<&SqliteIndex>,
        newest_mtime: &mut u64,
    ) -> String {
        // Identify the top-5 BM25-relevant files from the index (if available).
        let top_paths: Vec<PathBuf> = if let Some(idx) = index {
            match idx.search(query, 20).await {
                Ok(results) => {
                    let mut seen: HashSet<PathBuf> = HashSet::new();
                    let mut paths: Vec<PathBuf> = Vec::new();
                    for r in results {
                        let p = r.symbol.file.clone();
                        if seen.insert(p.clone()) {
                            paths.push(p);
                        }
                        if paths.len() >= 5 {
                            break;
                        }
                    }
                    paths
                }
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let top_refs: Vec<&Path> = top_paths.iter().map(|p| p.as_path()).collect();
        self.skeleton_hybrid_with_top_files(root, &top_refs, newest_mtime)
    }

    fn skeleton_hybrid_with_top_files(
        &self,
        root: &Path,
        top_files: &[&Path],
        newest_mtime: &mut u64,
    ) -> String {
        let top_set: HashSet<PathBuf> = top_files.iter().map(|p| p.to_path_buf()).collect();
        let known_symbols = collect_known_symbols(root);

        let mut extractor = SymbolExtractor::new();
        let mut key_lines: Vec<String> = Vec::new();
        let mut other_lines: Vec<String> = Vec::new();
        let mut file_count = 0usize;

        let walker = WalkBuilder::new(root)
            .hidden(true)
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            if path_contains_segment(path, "target")
                || path_contains_segment(path, "__pycache__")
                || path_contains_segment(path, "node_modules")
                || path_contains_segment(path, ".git")
            {
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "rs" | "py" | "ts" | "js" | "go") {
                continue;
            }

            if file_count >= 50 {
                break;
            }

            update_mtime(path, newest_mtime);

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let symbols = extractor.extract(path, &content);
            let deps = parse_dependencies(path, &content);
            let rel = path.strip_prefix(root).unwrap_or(path);
            let dep_suffix = if deps.is_empty() {
                String::new()
            } else {
                format!(" → [{}]", deps.join(", "))
            };

            let is_key = top_set.contains(path);

            if is_key {
                // Key file: show file header + per-function call detail.
                let header = format!("{}:{}", rel.display(), dep_suffix);
                let mut lines = vec![header];
                for sym in &symbols {
                    use hone_index::SymbolKind;
                    if sym.kind != SymbolKind::Function {
                        continue;
                    }
                    let calls = extract_calls(&content, &known_symbols);
                    let call_suffix = if calls.is_empty() {
                        String::new()
                    } else {
                        format!(" → calls [{}]", calls.join(", "))
                    };
                    lines.push(format!("  fn {}{}", sym.name, call_suffix));
                }
                key_lines.push(lines.join("\n"));
            } else {
                // Other file: compact one-liner with signatures + dep arrow.
                let sym_part = if symbols.is_empty() {
                    String::new()
                } else {
                    let names: Vec<String> = symbols
                        .iter()
                        .take(8)
                        .map(|s| format!("{} {}()", symbol_kind_prefix(s.kind), s.name))
                        .collect();
                    format!(": {}", names.join(", "))
                };
                other_lines.push(format!("{}{}{}", rel.display(), sym_part, dep_suffix));
            }

            file_count += 1;
        }

        let mut out = String::new();

        let total_files = file_count;
        // LOC is expensive to compute; use an estimate based on file_count.
        out.push_str(&format!("## Repository ({total_files} files)\n"));

        if !key_lines.is_empty() {
            out.push_str("\n### Key files (high relevance to your query):\n");
            for kl in &key_lines {
                out.push_str(kl);
                out.push('\n');
            }
        }

        if !other_lines.is_empty() {
            out.push_str("\n### Other files:\n");
            for ol in &other_lines {
                out.push_str(ol);
                out.push('\n');
            }
        }

        out
    }

    // -----------------------------------------------------------------------
    // Alternative approach b: dependency_trace
    // -----------------------------------------------------------------------

    /// Parse `use crate::*` / `use super::*` / `use self::*` import statements
    /// from a Rust source file and return paths of referenced modules.
    ///
    /// Simple regex-based approach — not a full name resolver.  Works well for
    /// the common single-crate layout where `use crate::foo` maps to `src/foo.rs`.
    pub fn dependency_trace(&self, file_path: &Path) -> Vec<PathBuf> {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let root = infer_crate_root(file_path);

        let re = regex::Regex::new(r"use\s+(crate|super|self)::([a-zA-Z_][a-zA-Z0-9_]*)").ok();
        let re = match re {
            Some(r) => r,
            None => return Vec::new(),
        };

        let mut paths: Vec<PathBuf> = Vec::new();

        for cap in re.captures_iter(&content) {
            let module = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            if module.is_empty() {
                continue;
            }

            // Try `src/<module>.rs` and `src/<module>/mod.rs`.
            let candidate_file = root.join(format!("{module}.rs"));
            let candidate_mod = root.join(module).join("mod.rs");

            if candidate_file.exists() {
                paths.push(candidate_file);
            } else if candidate_mod.exists() {
                paths.push(candidate_mod);
            }
        }

        paths.sort();
        paths.dedup();
        paths
    }

    // -----------------------------------------------------------------------
    // Alternative approach c: git_blame_recent
    // -----------------------------------------------------------------------

    /// Return the files changed most frequently within the last `days` days,
    /// sorted by change frequency descending.
    ///
    /// Uses `git log --since=N.days --name-only --format=""` and counts how
    /// many commits touched each file.  Files not present on disk are excluded.
    pub async fn git_blame_recent(
        &self,
        dir: &Path,
        days: u32,
    ) -> Vec<(PathBuf, u32)> {
        let since = format!("{days}.days");
        let output = Command::new("git")
            .args(["log", "--since", &since, "--name-only", "--format="])
            .current_dir(dir)
            .output()
            .await;

        let output = match output {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut freq: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let path = dir.join(line);
            if path.exists() {
                *freq.entry(path).or_default() += 1;
            }
        }

        let mut entries: Vec<(PathBuf, u32)> = freq.into_iter().collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries
    }

    // -----------------------------------------------------------------------
    // Alternative approach d: test_localization
    // -----------------------------------------------------------------------

    /// Find test files related to the query and return the source files they
    /// likely test.
    ///
    /// Scans for files whose names contain a keyword derived from `query` and
    /// that live under a `test*` or `tests` directory, or whose filename starts
    /// with `test_`.  Traces each test file back to its probable source via
    /// `dependency_trace`.
    pub fn test_localization(&self, dir: &Path, query: &str) -> Option<Vec<PathBuf>> {
        // Extract a simple keyword from the query (first alphabetic word > 3 chars).
        let keyword = query
            .split_whitespace()
            .find(|w| w.chars().all(|c| c.is_ascii_alphabetic()) && w.len() > 3)?
            .to_lowercase();

        let walker = WalkBuilder::new(dir)
            .hidden(false)
            .git_ignore(true)
            .build();

        let mut source_files: Vec<PathBuf> = Vec::new();

        for entry in walker.flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_lowercase();

            let in_test_dir = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("test"))
                .unwrap_or(false);

            let is_test_file = name.starts_with("test_") || name.contains("_test.") || in_test_dir;

            if is_test_file && name.contains(keyword.as_str()) {
                // Trace imports from this test file to source files.
                let traced = self.dependency_trace(path);
                source_files.extend(traced);
                // Also include the test file itself.
                source_files.push(path.to_path_buf());
            }
        }

        if source_files.is_empty() {
            None
        } else {
            source_files.sort();
            source_files.dedup();
            Some(source_files)
        }
    }
}

// ---------------------------------------------------------------------------
// Import / dependency parsing (free functions, language-dispatched)
// ---------------------------------------------------------------------------

/// Parse file dependencies from import statements.
///
/// Returns module names (not full paths) so that the output is compact and
/// language-agnostic.  The caller formats them as `→ [mod1, mod2]`.
pub fn parse_dependencies(path: &Path, content: &str) -> Vec<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut deps = match ext {
        "rs" => parse_rust_imports(content),
        "py" => parse_python_imports(content),
        "ts" | "js" => parse_js_imports(content),
        "go" => parse_go_imports(content),
        _ => vec![],
    };
    deps.sort();
    deps.dedup();
    deps
}

/// Extract module names from Rust `use` statements.
///
/// Handles:
/// - `use crate::module;`
/// - `use super::module;`
/// - `mod module;`
fn parse_rust_imports(content: &str) -> Vec<String> {
    // Compile once per call — these are called infrequently (one per file).
    let use_re =
        regex::Regex::new(r"use\s+(?:crate|super|self)::([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
    let mod_re = regex::Regex::new(r"\bmod\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*;").unwrap();

    let mut mods: Vec<String> = Vec::new();
    for cap in use_re.captures_iter(content) {
        mods.push(cap[1].to_string());
    }
    for cap in mod_re.captures_iter(content) {
        mods.push(cap[1].to_string());
    }
    mods
}

/// Extract module names from Python import statements.
///
/// Handles:
/// - `import module`
/// - `from module import ...`
/// - `from .module import ...` (relative)
fn parse_python_imports(content: &str) -> Vec<String> {
    let import_re = regex::Regex::new(r"^import\s+([a-zA-Z_][a-zA-Z0-9_.]*)").unwrap();
    let from_re =
        regex::Regex::new(r"^from\s+\.{0,2}([a-zA-Z_][a-zA-Z0-9_]*)\s+import").unwrap();

    let mut mods: Vec<String> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if let Some(cap) = import_re.captures(line) {
            // Take only the top-level package name.
            let top = cap[1].split('.').next().unwrap_or(&cap[1]).to_string();
            mods.push(top);
        } else if let Some(cap) = from_re.captures(line) {
            mods.push(cap[1].to_string());
        }
    }
    mods
}

/// Extract module names from JS/TS import statements.
///
/// Handles:
/// - `import ... from './module'`
/// - `import ... from '../module'`
/// - `require('./module')`
fn parse_js_imports(content: &str) -> Vec<String> {
    let esm_re =
        regex::Regex::new(r#"from\s+['"](?:\./|\.\./)([^'"./][^'"]*)['"]\s*"#).unwrap();
    let cjs_re =
        regex::Regex::new(r#"require\s*\(\s*['"](?:\./|\.\./)([^'"./][^'"]*)['"]\s*\)"#)
            .unwrap();

    let mut mods: Vec<String> = Vec::new();
    for cap in esm_re.captures_iter(content) {
        // Strip path separators and file extensions — keep only the leaf name.
        let raw = &cap[1];
        let leaf = raw.split('/').last().unwrap_or(raw);
        // Strip common extensions.
        let name = leaf
            .trim_end_matches(".ts")
            .trim_end_matches(".js")
            .trim_end_matches(".tsx")
            .trim_end_matches(".jsx");
        mods.push(name.to_string());
    }
    for cap in cjs_re.captures_iter(content) {
        let raw = &cap[1];
        let leaf = raw.split('/').last().unwrap_or(raw);
        let name = leaf
            .trim_end_matches(".ts")
            .trim_end_matches(".js")
            .trim_end_matches(".tsx")
            .trim_end_matches(".jsx");
        mods.push(name.to_string());
    }
    mods
}

/// Extract package names from Go import statements.
///
/// Handles single-line `import "pkg/sub"` and grouped `import (...)`.
fn parse_go_imports(content: &str) -> Vec<String> {
    // Single-line: import "pkg/sub/module"
    let single_re = regex::Regex::new(r#"import\s+"[^"]*/([^"]+)""#).unwrap();
    // Inside grouped import block lines: \t"pkg/sub/module"
    let grouped_re = regex::Regex::new(r#"^\s+"[^"]*/([^"]+)""#).unwrap();

    let mut mods: Vec<String> = Vec::new();
    for cap in single_re.captures_iter(content) {
        mods.push(cap[1].to_string());
    }
    for line in content.lines() {
        if let Some(cap) = grouped_re.captures(line) {
            mods.push(cap[1].to_string());
        }
    }
    mods
}

// ---------------------------------------------------------------------------
// Call graph extraction
// ---------------------------------------------------------------------------

/// Collect all known function/symbol names across source files in `root`.
///
/// Used as the candidate set for call detection via substring search.
fn collect_known_symbols(root: &Path) -> HashSet<String> {
    let mut extractor = SymbolExtractor::new();
    let mut names: HashSet<String> = HashSet::new();

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "rs" | "py" | "ts" | "js" | "go") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for sym in extractor.extract(path, &content) {
            names.insert(sym.name);
        }
    }

    names
}

/// For a given file's content, find calls to known symbols.
///
/// Uses simple substring containment — not a full AST analysis, but fast
/// and sufficient for identifying which other functions are referenced in the
/// same body.
pub fn extract_calls(content: &str, known_symbols: &HashSet<String>) -> Vec<String> {
    let mut calls: Vec<String> = known_symbols
        .iter()
        .filter(|sym| {
            // Require the symbol to appear followed by `(` to reduce false positives
            // from variable names or comments.
            let needle = sym.as_str();
            content.contains(&format!("{needle}("))
        })
        .cloned()
        .collect();
    calls.sort();
    calls
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Update `newest_mtime` with the mtime of `path` if newer.
fn update_mtime(path: &Path, newest_mtime: &mut u64) {
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(mtime) = meta.modified() {
            let secs = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if secs > *newest_mtime {
                *newest_mtime = secs;
            }
        }
    }
}

/// Infer the `src/` directory for a Rust file.
///
/// Walks upward from the file's parent looking for a `src` segment, then
/// returns it.  Falls back to the file's own parent directory.
fn infer_crate_root(file: &Path) -> PathBuf {
    let mut dir = file.parent().unwrap_or(file).to_path_buf();
    loop {
        if dir.file_name().and_then(|n| n.to_str()) == Some("src") {
            return dir;
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p.to_path_buf(),
            _ => break,
        }
    }
    file.parent().unwrap_or(Path::new(".")).to_path_buf()
}

/// Returns `true` if any component of `path` matches `segment`.
fn path_contains_segment(path: &Path, segment: &str) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| s == segment)
            .unwrap_or(false)
    })
}

fn symbol_kind_label(kind: hone_index::SymbolKind) -> &'static str {
    use hone_index::SymbolKind;
    match kind {
        SymbolKind::Function => "fn",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Impl => "impl",
        SymbolKind::Class => "class",
        SymbolKind::Module => "mod",
        SymbolKind::Constant => "const",
        SymbolKind::TypeAlias => "type",
    }
}

fn symbol_kind_prefix(kind: hone_index::SymbolKind) -> &'static str {
    symbol_kind_label(kind)
}

/// Trim `text` by line until it fits within `max_tokens`.
///
/// Drops lines from the middle of the block (never from the first two or last
/// two lines) so that structural markers at the start and the budget hint at
/// the end are always preserved.
fn trim_to_token_budget(text: &str, max_tokens: u32) -> String {
    let mut lines: Vec<&str> = text.lines().collect();

    while count_tokens(&lines.join("\n")) > max_tokens && lines.len() > 4 {
        // Remove the middle line.
        let mid = lines.len() / 2;
        lines.remove(mid);
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Skeleton generation — Flat
    // -----------------------------------------------------------------------

    #[test]
    fn test_skeleton_generation() {
        let dir = TempDir::new().unwrap();

        fs::write(
            dir.path().join("auth.rs"),
            r#"
pub fn login(user: &str) -> bool { true }
pub fn logout(session: &str) {}
pub struct Session { pub id: String }
"#,
        )
        .unwrap();

        fs::write(
            dir.path().join("db.rs"),
            r#"
pub fn connect() -> Pool { todo!() }
pub struct Pool {}
"#,
        )
        .unwrap();

        let injector = ContextInjector::with_defaults();
        let skeleton = injector.skeleton_from_dir(dir.path());

        assert!(!skeleton.is_empty(), "skeleton should not be empty");
        assert!(
            skeleton.contains("auth.rs") || skeleton.contains("db.rs"),
            "skeleton should list source files, got:\n{skeleton}"
        );
        assert!(
            skeleton.contains("login") || skeleton.contains("connect"),
            "skeleton should include function names, got:\n{skeleton}"
        );
    }

    // -----------------------------------------------------------------------
    // BM25 confidence — high gap
    // -----------------------------------------------------------------------

    #[test]
    fn test_bm25_confidence_high() {
        let injector = ContextInjector::with_defaults();
        // A clear winner: score 10.0 vs 1.0 → gap 0.9 (> 0.4)
        let scores = vec![10.0f32, 1.0, 0.5];
        let conf = injector.compute_confidence(&scores);
        assert_eq!(conf, RetrievalConfidence::High);
    }

    // -----------------------------------------------------------------------
    // BM25 confidence — low gap
    // -----------------------------------------------------------------------

    #[test]
    fn test_bm25_confidence_low() {
        let injector = ContextInjector::with_defaults();
        // All scores near each other → gap < 0.15
        let scores = vec![5.0f32, 4.9, 4.8, 4.7];
        let conf = injector.compute_confidence(&scores);
        assert_eq!(conf, RetrievalConfidence::Low);
    }

    // -----------------------------------------------------------------------
    // Token budget
    // -----------------------------------------------------------------------

    #[test]
    fn test_injection_respects_token_budget() {
        let max_tokens = 100u32;

        // Build a very long text and trim it.
        let long_text: String = (0..200)
            .map(|i| format!("line number {i} with some padding text here\n"))
            .collect();

        let trimmed = trim_to_token_budget(&long_text, max_tokens);
        let actual_tokens = count_tokens(&trimmed);
        assert!(
            actual_tokens <= max_tokens,
            "trimmed text uses {actual_tokens} tokens, expected <= {max_tokens}"
        );
    }

    // -----------------------------------------------------------------------
    // git_blame_recent (integration — requires git repo)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_git_blame_recent() {
        // Use the hone repo itself (which is a git repo) so we get real output.
        let dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."));

        let injector = ContextInjector::with_defaults();
        let recent = injector.git_blame_recent(&dir, 365).await;

        // We cannot assert specific files but we can assert basic invariants:
        // each entry has a count >= 1, paths exist on disk, and the list is
        // sorted descending by count.
        for (path, count) in &recent {
            assert!(*count >= 1, "count must be >= 1");
            assert!(path.exists(), "path must exist: {}", path.display());
        }

        // Verify descending sort.
        let counts: Vec<u32> = recent.iter().map(|(_, c)| *c).collect();
        let mut sorted = counts.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(counts, sorted, "results should be sorted by frequency desc");
    }

    // -----------------------------------------------------------------------
    // dependency_trace — Rust use statements
    // -----------------------------------------------------------------------

    #[test]
    fn test_dependency_trace_rust() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();

        // Create a source file that references two modules.
        let main_rs = src.join("main.rs");
        fs::write(
            &main_rs,
            "use crate::auth;\nuse crate::database;\n\nfn main() {}\n",
        )
        .unwrap();

        // Create the referenced module files.
        fs::write(src.join("auth.rs"), "pub fn login() {}\n").unwrap();
        fs::write(src.join("database.rs"), "pub fn connect() {}\n").unwrap();

        let injector = ContextInjector::with_defaults();
        let deps = injector.dependency_trace(&main_rs);

        assert!(
            deps.iter().any(|p| p.ends_with("auth.rs")),
            "should trace auth.rs, got: {deps:?}"
        );
        assert!(
            deps.iter().any(|p| p.ends_with("database.rs")),
            "should trace database.rs, got: {deps:?}"
        );
    }

    // -----------------------------------------------------------------------
    // parse_rust_imports
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_rust_imports_use_crate() {
        let content = "use crate::auth;\nuse crate::db;\nuse std::collections::HashMap;\n";
        let deps = parse_rust_imports(content);
        assert!(deps.contains(&"auth".to_string()), "missing auth: {deps:?}");
        assert!(deps.contains(&"db".to_string()), "missing db: {deps:?}");
        // std is not a crate:: import, so it should not appear.
        assert!(!deps.contains(&"std".to_string()), "std should not appear: {deps:?}");
    }

    #[test]
    fn test_parse_rust_imports_mod_statement() {
        let content = "mod auth;\nmod db;\n\nfn main() {}\n";
        let deps = parse_rust_imports(content);
        assert!(deps.contains(&"auth".to_string()), "missing auth: {deps:?}");
        assert!(deps.contains(&"db".to_string()), "missing db: {deps:?}");
    }

    #[test]
    fn test_parse_rust_imports_super() {
        let content = "use super::utils;\nuse self::helpers;\n";
        let deps = parse_rust_imports(content);
        assert!(deps.contains(&"utils".to_string()), "missing utils: {deps:?}");
        assert!(deps.contains(&"helpers".to_string()), "missing helpers: {deps:?}");
    }

    // -----------------------------------------------------------------------
    // parse_python_imports
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_python_imports_import() {
        let content = "import os\nimport requests\nimport myapp.utils\n";
        let deps = parse_python_imports(content);
        assert!(deps.contains(&"os".to_string()), "missing os: {deps:?}");
        assert!(deps.contains(&"requests".to_string()), "missing requests: {deps:?}");
        assert!(deps.contains(&"myapp".to_string()), "missing myapp: {deps:?}");
    }

    #[test]
    fn test_parse_python_imports_from() {
        let content = "from auth import login, logout\nfrom .db import connect\nfrom ..utils import helper\n";
        let deps = parse_python_imports(content);
        assert!(deps.contains(&"auth".to_string()), "missing auth: {deps:?}");
        assert!(deps.contains(&"db".to_string()), "missing db: {deps:?}");
        assert!(deps.contains(&"utils".to_string()), "missing utils: {deps:?}");
    }

    // -----------------------------------------------------------------------
    // parse_js_imports
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_js_imports_esm() {
        let content = r#"
import { login } from './auth';
import Router from '../router/index';
import { connect } from './db.ts';
"#;
        let deps = parse_js_imports(content);
        assert!(deps.contains(&"auth".to_string()), "missing auth: {deps:?}");
        assert!(deps.contains(&"index".to_string()), "missing index: {deps:?}");
        assert!(deps.contains(&"db".to_string()), "missing db: {deps:?}");
    }

    #[test]
    fn test_parse_js_imports_cjs_require() {
        let content = r#"
const auth = require('./auth');
const db = require('../db/connection');
"#;
        let deps = parse_js_imports(content);
        assert!(deps.contains(&"auth".to_string()), "missing auth: {deps:?}");
        assert!(deps.contains(&"connection".to_string()), "missing connection: {deps:?}");
    }

    #[test]
    fn test_parse_js_imports_skips_external() {
        // External packages (no ./ or ../) should not appear.
        let content = r#"import React from 'react'; import { useState } from 'react';"#;
        let deps = parse_js_imports(content);
        assert!(deps.is_empty(), "external imports should be skipped: {deps:?}");
    }

    // -----------------------------------------------------------------------
    // parse_go_imports
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_go_imports_single_line() {
        let content = r#"import "github.com/myapp/auth""#;
        let deps = parse_go_imports(content);
        assert!(deps.contains(&"auth".to_string()), "missing auth: {deps:?}");
    }

    #[test]
    fn test_parse_go_imports_grouped() {
        let content = r#"
import (
    "fmt"
    "github.com/myapp/auth"
    "github.com/myapp/db"
)
"#;
        let deps = parse_go_imports(content);
        assert!(deps.contains(&"auth".to_string()), "missing auth: {deps:?}");
        assert!(deps.contains(&"db".to_string()), "missing db: {deps:?}");
    }

    // -----------------------------------------------------------------------
    // parse_dependencies dispatch
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_dependencies_dispatches_by_extension() {
        let rs_path = Path::new("main.rs");
        let content = "use crate::db;\nmod auth;\n";
        let deps = parse_dependencies(rs_path, content);
        assert!(deps.contains(&"db".to_string()));
        assert!(deps.contains(&"auth".to_string()));

        // Unknown extension → empty.
        let cpp_path = Path::new("main.cpp");
        let deps_cpp = parse_dependencies(cpp_path, "#include <foo.h>");
        assert!(deps_cpp.is_empty());
    }

    // -----------------------------------------------------------------------
    // extract_calls
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_calls_finds_known_symbols() {
        let known: HashSet<String> = ["connect", "hash_password", "verify"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let body = "fn login() { let pool = connect(); hash_password(pw); }";
        let calls = extract_calls(body, &known);

        assert!(calls.contains(&"connect".to_string()), "missing connect: {calls:?}");
        assert!(calls.contains(&"hash_password".to_string()), "missing hash_password: {calls:?}");
        // verify( does not appear in body → should not be listed.
        assert!(!calls.contains(&"verify".to_string()), "verify should not appear: {calls:?}");
    }

    #[test]
    fn test_extract_calls_empty_when_no_matches() {
        let known: HashSet<String> = ["foo", "bar"].iter().map(|s| s.to_string()).collect();
        let body = "fn do_nothing() {}";
        let calls = extract_calls(body, &known);
        assert!(calls.is_empty(), "expected empty calls, got: {calls:?}");
    }

    // -----------------------------------------------------------------------
    // skeleton_dependency_graph
    // -----------------------------------------------------------------------

    #[test]
    fn test_skeleton_dependency_graph_format() {
        let dir = TempDir::new().unwrap();

        fs::write(
            dir.path().join("main.rs"),
            "use crate::auth;\nuse crate::db;\n\nfn main() {}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("auth.rs"),
            "use crate::db;\npub fn login() {}\npub fn logout() {}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("db.rs"),
            "pub fn connect() {}\npub fn query() {}\n",
        )
        .unwrap();

        let injector = ContextInjector::with_defaults();
        let skeleton = injector.skeleton_dependency_graph(dir.path());

        // main.rs should list its dependencies.
        assert!(skeleton.contains("main.rs"), "missing main.rs:\n{skeleton}");
        // At least one file should have a dependency arrow.
        assert!(skeleton.contains("→ ["), "expected dependency arrows:\n{skeleton}");
        // auth.rs should show login and logout.
        assert!(skeleton.contains("login"), "missing login:\n{skeleton}");
    }

    // -----------------------------------------------------------------------
    // skeleton_call_graph
    // -----------------------------------------------------------------------

    #[test]
    fn test_skeleton_call_graph_format() {
        let dir = TempDir::new().unwrap();

        fs::write(
            dir.path().join("auth.rs"),
            r#"
pub fn connect() -> bool { true }
pub fn login() -> bool { connect() }
"#,
        )
        .unwrap();

        let injector = ContextInjector::with_defaults();
        let skeleton = injector.skeleton_call_graph(dir.path());

        // Should have a file header.
        assert!(skeleton.contains("auth.rs:"), "missing file header:\n{skeleton}");
        // Should list functions with indentation.
        assert!(skeleton.contains("  fn "), "expected indented fn lines:\n{skeleton}");
        // login calls connect, so connect should appear in its calls list.
        assert!(skeleton.contains("connect"), "missing connect call:\n{skeleton}");
    }

    // -----------------------------------------------------------------------
    // skeleton_hybrid
    // -----------------------------------------------------------------------

    #[test]
    fn test_skeleton_hybrid_format() {
        let dir = TempDir::new().unwrap();

        fs::write(
            dir.path().join("auth.rs"),
            "use crate::db;\npub fn login() {}\npub fn logout() {}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("db.rs"),
            "pub fn connect() {}\npub fn query() {}\n",
        )
        .unwrap();

        let auth_path = dir.path().join("auth.rs");
        let top: Vec<&Path> = vec![auth_path.as_path()];

        let injector = ContextInjector::with_defaults();
        let skeleton = injector.skeleton_hybrid(dir.path(), "login auth", &top);

        // Should have repository header.
        assert!(skeleton.contains("## Repository"), "missing header:\n{skeleton}");
        // Should have key files section for auth.rs.
        assert!(skeleton.contains("### Key files"), "missing key files section:\n{skeleton}");
        // auth.rs should have call-level detail (fn entries).
        assert!(skeleton.contains("fn login"), "missing fn login:\n{skeleton}");
        // Other files section for db.rs.
        assert!(skeleton.contains("### Other files"), "missing other files:\n{skeleton}");
        assert!(skeleton.contains("db.rs"), "missing db.rs:\n{skeleton}");
    }

    // -----------------------------------------------------------------------
    // SkeletonStrategy::from_str
    // -----------------------------------------------------------------------

    #[test]
    fn test_skeleton_strategy_from_str() {
        assert_eq!(SkeletonStrategy::from_str("flat"), Some(SkeletonStrategy::Flat));
        assert_eq!(SkeletonStrategy::from_str("Flat"), Some(SkeletonStrategy::Flat));
        assert_eq!(
            SkeletonStrategy::from_str("dependency-graph"),
            Some(SkeletonStrategy::DependencyGraph)
        );
        assert_eq!(
            SkeletonStrategy::from_str("dependency_graph"),
            Some(SkeletonStrategy::DependencyGraph)
        );
        assert_eq!(
            SkeletonStrategy::from_str("deps"),
            Some(SkeletonStrategy::DependencyGraph)
        );
        assert_eq!(
            SkeletonStrategy::from_str("call-graph"),
            Some(SkeletonStrategy::CallGraph)
        );
        assert_eq!(
            SkeletonStrategy::from_str("call_graph"),
            Some(SkeletonStrategy::CallGraph)
        );
        assert_eq!(
            SkeletonStrategy::from_str("calls"),
            Some(SkeletonStrategy::CallGraph)
        );
        assert_eq!(
            SkeletonStrategy::from_str("hybrid"),
            Some(SkeletonStrategy::Hybrid)
        );
        assert_eq!(SkeletonStrategy::from_str("unknown"), None);
        assert_eq!(SkeletonStrategy::from_str(""), None);
    }
}
