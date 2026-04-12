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
                        .as_secs();
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
}

impl ContextInjector {
    /// Create an injector with explicit limits.
    pub fn new(max_injection_tokens: u32, confidence_threshold: f32) -> Self {
        Self {
            max_injection_tokens,
            confidence_threshold,
            skeleton_cache: None,
        }
    }

    /// Create with sensible defaults (2 000 token budget, 0.4 gap threshold).
    pub fn with_defaults() -> Self {
        Self::new(2_000, 0.4)
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
        let skeleton = self.get_or_refresh_skeleton(working_dir);
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
    // Alternative approach a: skeleton_from_dir
    // -----------------------------------------------------------------------

    /// Get the cached skeleton or regenerate if files have changed.
    fn get_or_refresh_skeleton(&mut self, root: &Path) -> String {
        // Check if we have a valid cache for this root.
        let needs_refresh = match &self.skeleton_cache {
            Some(cache) if cache.root == root => !cache.is_fresh(),
            _ => true,
        };

        if needs_refresh {
            tracing::debug!("skeleton cache miss — regenerating");
            let (skeleton, newest_mtime) = self.skeleton_from_dir_with_mtime(root);
            self.skeleton_cache = Some(SkeletonCache {
                skeleton: skeleton.clone(),
                newest_mtime,
                root: root.to_path_buf(),
            });
            skeleton
        } else {
            tracing::debug!("skeleton cache hit — reusing");
            self.skeleton_cache.as_ref().unwrap().skeleton.clone()
        }
    }

    /// Generate a compact repo skeleton and return (skeleton, newest_mtime).
    fn skeleton_from_dir_with_mtime(&self, root: &Path) -> (String, u64) {
        let mut newest_mtime = 0u64;
        let skeleton = self.skeleton_from_dir_inner(root, &mut newest_mtime);
        (skeleton, newest_mtime)
    }

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
                        .as_secs();
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
// Private helpers
// ---------------------------------------------------------------------------

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
    // Skeleton generation
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
        // Should mention at least one of the Rust files.
        assert!(
            skeleton.contains("auth.rs") || skeleton.contains("db.rs"),
            "skeleton should list source files, got:\n{skeleton}"
        );
        // Should list function names.
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
        let injector = ContextInjector::new(max_tokens, 0.4);

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
}
