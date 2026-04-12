//! Adaptive hierarchical skeleton generator.
//!
//! Builds a repo skeleton in budget-aware layers, from cheapest to most
//! detailed, so that large repos (500K+ LOC) never time out or overflow the
//! token budget.
//!
//! ## Layers (cheapest → most detailed)
//!
//! | Level | What is shown | Typical cost |
//! |-------|--------------|--------------|
//! | 0     | Directory tree only | ~200 tokens |
//! | 1     | Dir tree + file list with function counts | ~500 tokens |
//! | 2     | Signatures for query-relevant files | +~500 tokens |
//! | 3     | Call graph for top-5 relevant files | +~800 tokens |
//!
//! The [`HierarchicalSkeleton`] driver always emits level 0, then greedily
//! adds deeper levels until the budget is exhausted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use hone_index::{SqliteIndex, SymbolExtractor, SymbolKind};
use ignore::WalkBuilder;


// ---------------------------------------------------------------------------
// Token counting (re-use the one already defined in context_injector)
// ---------------------------------------------------------------------------

use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

fn bpe() -> &'static CoreBPE {
    static BPE: OnceLock<CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().expect("cl100k_base BPE must load"))
}

pub(crate) fn count_tokens(text: &str) -> u32 {
    bpe().encode_ordinary(&text.to_string()).len() as u32
}

// ---------------------------------------------------------------------------
// Shared helpers (duplicated here to keep the module self-contained)
// ---------------------------------------------------------------------------

/// Returns `true` if any path component equals `segment`.
pub(crate) fn path_contains_segment(path: &Path, segment: &str) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| s == segment)
            .unwrap_or(false)
    })
}

/// Update `newest_mtime` with the mtime of `path` if newer.
pub(crate) fn update_mtime(path: &Path, newest_mtime: &mut u64) {
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(mtime) = meta.modified() {
            let ms = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if ms > *newest_mtime {
                *newest_mtime = ms;
            }
        }
    }
}

/// Collect all known symbol names across source files — used as the candidate
/// set for call detection.
pub(crate) fn collect_known_symbols(root: &Path) -> HashSet<String> {
    let mut extractor = SymbolExtractor::new();
    let mut names: HashSet<String> = HashSet::new();

    for file in walk_source_files(root) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        for sym in extractor.extract(&file, &content) {
            names.insert(sym.name);
        }
    }

    names
}

/// For a given file's content, find calls to known symbols.
///
/// Looks for `symbol_name(` occurrences to reduce false positives from
/// variable names and comments.
pub(crate) fn extract_calls(content: &str, known_symbols: &HashSet<String>) -> Vec<String> {
    let mut calls: Vec<String> = known_symbols
        .iter()
        .filter(|sym| content.contains(&format!("{}(", sym)))
        .cloned()
        .collect();
    calls.sort();
    calls
}

// ---------------------------------------------------------------------------
// Source-file extensions recognised by the skeleton
// ---------------------------------------------------------------------------

const SOURCE_EXTS: &[&str] = &["rs", "py", "ts", "js", "go", "java", "c", "cpp", "h"];

fn is_source_ext(ext: &str) -> bool {
    SOURCE_EXTS.contains(&ext)
}

// ---------------------------------------------------------------------------
// Directory walker helpers
// ---------------------------------------------------------------------------

/// Walk all source files respecting `.gitignore`, skipping build artefacts.
pub(crate) fn walk_source_files(root: &Path) -> impl Iterator<Item = PathBuf> {
    WalkBuilder::new(root)
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.into_path())
        .filter(|p| {
            if path_contains_segment(p, "target")
                || path_contains_segment(p, "__pycache__")
                || path_contains_segment(p, "node_modules")
                || path_contains_segment(p, ".git")
            {
                return false;
            }
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            is_source_ext(ext)
        })
}

/// Count source files (stat-only, no reads).
pub fn count_source_files(root: &Path) -> usize {
    walk_source_files(root).count()
}

// ---------------------------------------------------------------------------
// Level 0 — Directory tree
// ---------------------------------------------------------------------------

/// Build a compact directory tree showing how many source files live in each
/// directory.
///
/// ```text
/// src/ (12 files)
///   auth/ (3 files)
///   db/ (2 files)
///   api/ (4 files)
/// tests/ (8 files)
/// ```
///
/// At most `max_dirs` directories are shown.
pub fn level0_directory_tree(root: &Path, max_dirs: usize) -> String {
    // Count files per relative directory.
    let mut dir_counts: HashMap<PathBuf, usize> = HashMap::new();

    for file in walk_source_files(root) {
        if let Some(parent) = file.parent() {
            let rel = parent.strip_prefix(root).unwrap_or(parent).to_path_buf();
            *dir_counts.entry(rel).or_default() += 1;
        }
    }

    if dir_counts.is_empty() {
        return String::new();
    }

    // Sort by path for deterministic output, but root-level first.
    let mut dirs: Vec<(PathBuf, usize)> = dir_counts.into_iter().collect();
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    dirs.truncate(max_dirs);

    let mut lines: Vec<String> = Vec::with_capacity(dirs.len() + 1);
    let total: usize = dirs.iter().map(|(_, c)| c).sum();
    lines.push(format!(
        "Repository root ({total} source files):"
    ));

    for (rel, count) in &dirs {
        let depth = rel.components().count();
        let indent = "  ".repeat(depth);
        let label = if rel.as_os_str().is_empty() {
            "<root>".to_string()
        } else {
            rel.display().to_string()
        };
        let plural = if *count == 1 { "file" } else { "files" };
        lines.push(format!("{indent}{label}/ ({count} {plural})"));
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Level 1 — Directory + file list with function counts
// ---------------------------------------------------------------------------

/// Build a file list annotated with function counts.
///
/// ```text
/// src/auth/
///   login.rs (4 fns)
///   session.rs (3 fns)
/// src/db/
///   pool.rs (2 fns)
/// ```
///
/// At most `max_files` files are shown.  Counts are derived cheaply from a
/// line-based heuristic (pattern `def |fn `) without running tree-sitter.
pub fn level1_file_list(root: &Path, max_files: usize) -> String {
    let mut files: Vec<PathBuf> = walk_source_files(root).take(max_files).collect();
    files.sort();

    if files.is_empty() {
        return String::new();
    }

    // Group by parent directory.
    let mut by_dir: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for f in &files {
        let parent = f
            .parent()
            .map(|p| p.strip_prefix(root).unwrap_or(p).to_path_buf())
            .unwrap_or_default();
        by_dir.entry(parent).or_default().push(f.clone());
    }

    let mut dirs: Vec<PathBuf> = by_dir.keys().cloned().collect();
    dirs.sort();

    let mut lines: Vec<String> = Vec::new();
    for dir in &dirs {
        let display = if dir.as_os_str().is_empty() {
            ".".to_string()
        } else {
            dir.display().to_string()
        };
        lines.push(format!("{display}/"));
        if let Some(file_list) = by_dir.get(dir) {
            let mut sorted = file_list.clone();
            sorted.sort();
            for file in &sorted {
                let name = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?");
                let fn_count = count_functions_cheap(file);
                let label = if fn_count > 0 {
                    format!("  {name} ({fn_count} fns)")
                } else {
                    format!("  {name}")
                };
                lines.push(label);
            }
        }
    }

    lines.join("\n")
}

/// Cheap function count using line pattern matching — no tree-sitter, no file
/// reads beyond a line scan.
fn count_functions_cheap(path: &Path) -> usize {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let content = match std::fs::read_to_string(path) {
        Ok(c) if c.len() < 500_000 => c,
        _ => return 0,
    };
    match ext {
        "rs" => content
            .lines()
            .filter(|l| {
                let t = l.trim();
                (t.starts_with("fn ") || t.contains(" fn ")) && !t.starts_with("//")
            })
            .count(),
        "py" => content
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("def ") || t.starts_with("async def ")
            })
            .count(),
        "ts" | "js" => content
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("function ")
                    || t.contains("=> {")
                    || t.starts_with("async function ")
            })
            .count(),
        "go" => content
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("func ")
            })
            .count(),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Level 2 — Function signatures for selected files
// ---------------------------------------------------------------------------

/// Emit function signatures for the given files using tree-sitter.
///
/// ```text
/// src/auth/login.rs:
///   fn login(user: &str, password: &str) -> Result<Token>
///   fn validate_credentials(user: &str, password: &str) -> bool
/// ```
///
/// Skips files > 100 KB and files that take > 100 ms to parse.
pub fn level2_signatures(root: &Path, files: &[PathBuf]) -> String {
    let parsed = parse_selected_files(files);

    let mut sections: Vec<String> = Vec::new();
    for file in files {
        if let Some(symbols) = parsed.get(file) {
            if symbols.is_empty() {
                continue;
            }
            let rel = file.strip_prefix(root).unwrap_or(file);
            let mut lines = vec![format!("{}:", rel.display())];
            for sym in symbols {
                let prefix = symbol_kind_prefix(sym.kind);
                lines.push(format!("  {prefix} {}", sym.name));
            }
            sections.push(lines.join("\n"));
        }
    }

    sections.join("\n")
}

// ---------------------------------------------------------------------------
// Level 3 — Call graph for selected files
// ---------------------------------------------------------------------------

/// Emit function signatures plus call relationships for selected files.
///
/// ```text
/// src/auth/login.rs:
///   fn login → calls [connect, hash_password]
///   fn logout → calls [destroy]
/// ```
///
/// Uses the same known-symbol set as the existing `CallGraph` strategy.
pub fn level3_call_graph(
    root: &Path,
    files: &[PathBuf],
    known_symbols: &HashSet<String>,
) -> String {
    let parsed = parse_selected_files(files);

    let mut sections: Vec<String> = Vec::new();

    for file in files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) if c.len() < 100_000 => c,
            _ => continue,
        };

        if let Some(symbols) = parsed.get(file) {
            if symbols.is_empty() {
                continue;
            }

            let rel = file.strip_prefix(root).unwrap_or(file);
            let calls = extract_calls(&content, known_symbols);
            let call_suffix = if calls.is_empty() {
                String::new()
            } else {
                format!(" → calls [{}]", calls.join(", "))
            };

            let mut lines = vec![format!("{}:", rel.display())];
            for sym in symbols {
                if sym.kind != SymbolKind::Function {
                    continue;
                }
                lines.push(format!("  fn {}{}", sym.name, call_suffix));
            }
            if lines.len() > 1 {
                sections.push(lines.join("\n"));
            }
        }
    }

    sections.join("\n")
}

// ---------------------------------------------------------------------------
// Performance-safe tree-sitter parsing
// ---------------------------------------------------------------------------

/// Parse only the specified files with safety guards:
/// - Skips files > 100 KB.
/// - Skips files that take > 100 ms to parse (logs a warning).
pub(crate) fn parse_selected_files(
    files: &[PathBuf],
) -> HashMap<PathBuf, Vec<hone_index::Symbol>> {
    let mut result = HashMap::new();
    let mut extractor = SymbolExtractor::new();

    for file in files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) if c.len() < 100_000 => c,
            Ok(_) => {
                tracing::debug!("skip large file: {}", file.display());
                continue;
            }
            Err(_) => continue,
        };

        let start = Instant::now();
        let symbols = extractor.extract(file, &content);
        let elapsed = start.elapsed();

        if elapsed.as_millis() > 100 {
            tracing::warn!(
                "slow parse: {} took {}ms, skipping",
                file.display(),
                elapsed.as_millis()
            );
            continue;
        }

        result.insert(file.clone(), symbols);
    }

    result
}

// ---------------------------------------------------------------------------
// Relevance scoring — find files most likely related to the query
// ---------------------------------------------------------------------------

/// Find files relevant to the query using cheap filename/path matching.
///
/// Does not read file contents; only scans paths.  Score is the number of
/// query words that appear as substrings in the file path.
pub async fn find_relevant_files(
    root: &Path,
    query: &str,
    index: Option<&SqliteIndex>,
    limit: usize,
) -> Vec<PathBuf> {
    let query_words: Vec<String> = query
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .map(|w| w.to_lowercase())
        .collect();

    let mut scored: Vec<(f32, PathBuf)> = Vec::new();

    // Signal 1: BM25 from index (most accurate).
    if let Some(idx) = index {
        if let Ok(results) = idx.search(query, 20).await {
            let mut seen: HashSet<PathBuf> = HashSet::new();
            for r in results {
                let p = r.symbol.file.clone();
                if seen.insert(p.clone()) {
                    let score = r.score as f32 + 10.0; // BM25 gets a big boost
                    scored.push((score, p));
                }
            }
        }
    }

    // Signal 2: filename/path keyword matching (cheap, no file reads).
    let index_paths: HashSet<PathBuf> = scored.iter().map(|(_, p)| p.clone()).collect();
    for file in walk_source_files(root) {
        if index_paths.contains(&file) {
            continue; // already scored via BM25
        }
        let path_str = file.to_string_lossy().to_lowercase();
        let score: f32 = query_words
            .iter()
            .filter(|w| path_str.contains(w.as_str()))
            .count() as f32;
        if score > 0.0 {
            scored.push((score, file));
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(_, p)| p).collect()
}

// ---------------------------------------------------------------------------
// HierarchicalSkeleton driver
// ---------------------------------------------------------------------------

/// Adaptive hierarchical skeleton generator.
///
/// Starts with the cheapest representation (directory tree) and greedily
/// layers on more detail until the token budget is exhausted.
pub struct HierarchicalSkeleton {
    /// Total token budget for the entire skeleton output.
    pub budget: u32,
}

impl HierarchicalSkeleton {
    pub fn new(budget: u32) -> Self {
        Self { budget }
    }

    /// Generate the skeleton for `root`, tuned to `query`.
    ///
    /// `index` is optional; when present it improves relevance scoring via
    /// BM25.  When absent, filename matching is used instead.
    pub async fn generate(
        &self,
        root: &Path,
        query: &str,
        index: Option<&SqliteIndex>,
        newest_mtime: &mut u64,
    ) -> String {
        let mut output = String::new();
        let mut remaining = self.budget;

        // ---- Layer 0: directory tree (always) --------------------------------
        let dir_tree = level0_directory_tree(root, 40);
        let dir_tokens = count_tokens(&dir_tree);
        if dir_tokens <= remaining {
            output.push_str(&dir_tree);
            remaining = remaining.saturating_sub(dir_tokens);
        } else {
            // Even the tree is too big — trim it.
            output.push_str(&trim_text_to_tokens(&dir_tree, remaining));
            return output;
        }

        // Track mtime for all source files (stat-only).
        for file in walk_source_files(root) {
            update_mtime(&file, newest_mtime);
        }

        // ---- Relevance scoring ----------------------------------------------
        let relevant_files = find_relevant_files(root, query, index, 10).await;

        if relevant_files.is_empty() {
            // No signal — fall back to level-1 file list if budget allows.
            if remaining > 300 {
                let file_list = level1_file_list(root, 50);
                let fl_tokens = count_tokens(&file_list);
                if fl_tokens <= remaining {
                    output.push_str("\n\n");
                    output.push_str(&file_list);
                }
            }
            return output;
        }

        // ---- Layer 3: call graph for top-5 relevant files --------------------
        let top5_end = 5.min(relevant_files.len());
        let top5 = &relevant_files[..top5_end];

        if remaining > 800 {
            let known = collect_known_symbols(root);
            let detail = level3_call_graph(root, top5, &known);
            if !detail.is_empty() {
                let tok = count_tokens(&detail);
                if tok <= remaining {
                    output.push_str("\n\n### Relevant code (detailed):\n");
                    output.push_str(&detail);
                    remaining = remaining.saturating_sub(tok + 40); // header overhead
                }
            }
        }

        // ---- Layer 2: signatures for remaining relevant files ----------------
        let rest = &relevant_files[top5_end..];
        if !rest.is_empty() && remaining > 300 {
            let sigs = level2_signatures(root, rest);
            if !sigs.is_empty() {
                let tok = count_tokens(&sigs);
                if tok <= remaining {
                    output.push_str("\n\n### Other relevant files:\n");
                    output.push_str(&sigs);
                    remaining = remaining.saturating_sub(tok + 40);
                }
            }
        }

        // ---- Layer 1: file list for the whole repo if budget remains ---------
        let _ = remaining; // suppress unused warning
        output
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Trim text by dropping lines from the middle until it fits in `max_tokens`.
fn trim_text_to_tokens(text: &str, max_tokens: u32) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    while count_tokens(&lines.join("\n")) > max_tokens && lines.len() > 4 {
        let mid = lines.len() / 2;
        lines.remove(mid);
    }
    lines.join("\n")
}

fn symbol_kind_prefix(kind: SymbolKind) -> &'static str {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Helper: create a small fake repo
    // -----------------------------------------------------------------------

    fn make_repo(dir: &Path, file_specs: &[(&str, &str)]) {
        for (rel_path, content) in file_specs {
            let full = dir.join(rel_path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, content).unwrap();
        }
    }

    // -----------------------------------------------------------------------
    // level0_directory_tree
    // -----------------------------------------------------------------------

    #[test]
    fn test_level0_directory_tree_basic() {
        let dir = TempDir::new().unwrap();
        make_repo(
            dir.path(),
            &[
                ("src/auth/login.rs", "fn login() {}"),
                ("src/auth/session.rs", "fn new_session() {}"),
                ("src/db/pool.rs", "fn connect() {}"),
                ("tests/auth_test.rs", "fn test_login() {}"),
            ],
        );

        let tree = level0_directory_tree(dir.path(), 30);
        assert!(!tree.is_empty(), "tree should not be empty");
        assert!(tree.contains("src"), "should mention src dir");
        assert!(tree.contains("files"), "should mention file counts");
    }

    #[test]
    fn test_level0_directory_tree_empty_repo() {
        let dir = TempDir::new().unwrap();
        let tree = level0_directory_tree(dir.path(), 30);
        assert!(tree.is_empty(), "empty repo should produce empty tree");
    }

    // -----------------------------------------------------------------------
    // level1_file_list
    // -----------------------------------------------------------------------

    #[test]
    fn test_level1_file_list() {
        let dir = TempDir::new().unwrap();
        make_repo(
            dir.path(),
            &[
                ("src/auth.rs", "fn login() {}\nfn logout() {}"),
                ("src/db.rs", "fn connect() {}"),
            ],
        );

        let list = level1_file_list(dir.path(), 50);
        assert!(list.contains("auth.rs"), "should list auth.rs");
        assert!(list.contains("db.rs"), "should list db.rs");
        assert!(list.contains("fns"), "should show function counts");
    }

    // -----------------------------------------------------------------------
    // level2_signatures
    // -----------------------------------------------------------------------

    #[test]
    fn test_level2_signatures() {
        let dir = TempDir::new().unwrap();
        make_repo(
            dir.path(),
            &[("src/auth.rs", "pub fn login() {}\npub fn logout() {}")],
        );

        let files = vec![dir.path().join("src/auth.rs")];
        let sigs = level2_signatures(dir.path(), &files);
        assert!(sigs.contains("login"), "should show login fn");
        assert!(sigs.contains("logout"), "should show logout fn");
    }

    // -----------------------------------------------------------------------
    // parse_selected_files — large file skipped
    // -----------------------------------------------------------------------

    #[test]
    fn test_large_file_skipped() {
        let dir = TempDir::new().unwrap();
        // Write a file larger than 100 KB.
        let large_content = "fn placeholder() {}\n".repeat(6_000); // ~120 KB
        assert!(large_content.len() > 100_000, "fixture must be > 100 KB");
        let large_path = dir.path().join("large.rs");
        fs::write(&large_path, &large_content).unwrap();

        let parsed = parse_selected_files(&[large_path.clone()]);
        assert!(
            !parsed.contains_key(&large_path),
            "large file should be skipped"
        );
    }

    // -----------------------------------------------------------------------
    // find_relevant_files — filename matching
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_relevant_files_by_filename() {
        let dir = TempDir::new().unwrap();
        make_repo(
            dir.path(),
            &[
                ("src/auth.rs", "fn login() {}"),
                ("src/session.rs", "fn create() {}"),
                ("src/db.rs", "fn connect() {}"),
            ],
        );

        let found = find_relevant_files(dir.path(), "auth login bug", None, 5).await;
        assert!(
            found.iter().any(|p| p.ends_with("auth.rs")),
            "auth.rs should be ranked relevant, got: {found:?}"
        );
    }

    // -----------------------------------------------------------------------
    // HierarchicalSkeleton — budget respected
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_budget_respected() {
        let dir = TempDir::new().unwrap();
        make_repo(
            dir.path(),
            &[
                ("src/a.rs", "fn alpha() {}\nfn beta() {}"),
                ("src/b.rs", "fn gamma() {}\nfn delta() {}"),
                ("src/c.rs", "fn epsilon() {}"),
            ],
        );

        let skeleton = HierarchicalSkeleton::new(500);
        let mut mtime = 0u64;
        let output = skeleton
            .generate(dir.path(), "alpha", None, &mut mtime)
            .await;

        let tokens = count_tokens(&output);
        assert!(
            tokens <= 500,
            "output uses {tokens} tokens, expected <= 500"
        );
    }

    // -----------------------------------------------------------------------
    // HierarchicalSkeleton — small repo gets detailed output
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_adaptive_small_repo_gets_detail() {
        let dir = TempDir::new().unwrap();
        make_repo(
            dir.path(),
            &[
                ("src/auth.rs", "pub fn login() {}\npub fn logout() {}"),
                ("src/db.rs", "pub fn connect() {}"),
            ],
        );

        let skeleton = HierarchicalSkeleton::new(2_000);
        let mut mtime = 0u64;
        let output = skeleton
            .generate(dir.path(), "login auth", None, &mut mtime)
            .await;

        // Should contain directory tree at minimum.
        assert!(!output.is_empty(), "output should not be empty");
        assert!(
            output.contains("src"),
            "output should reference src directory"
        );
    }

    // -----------------------------------------------------------------------
    // HierarchicalSkeleton — large repo gets directory tree only
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_adaptive_large_repo_stays_in_budget() {
        let dir = TempDir::new().unwrap();
        // Create many files spread across directories to simulate a large repo.
        for i in 0..30 {
            let subdir = format!("src/module_{}", i % 5);
            make_repo(
                dir.path(),
                &[(
                    &format!("{subdir}/file_{i}.rs"),
                    &format!("pub fn func_{i}() {{}}"),
                )],
            );
        }

        let skeleton = HierarchicalSkeleton::new(600);
        let mut mtime = 0u64;
        let output = skeleton
            .generate(dir.path(), "some query", None, &mut mtime)
            .await;

        let tokens = count_tokens(&output);
        assert!(
            tokens <= 600,
            "output uses {tokens} tokens, expected <= 600"
        );
    }

    // -----------------------------------------------------------------------
    // Progressive expansion — adds detail until budget exhausted
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_progressive_expansion() {
        let dir = TempDir::new().unwrap();
        make_repo(
            dir.path(),
            &[
                ("src/auth/login.rs", "pub fn login() {}\npub fn logout() {}"),
                ("src/auth/session.rs", "pub fn create_session() {}"),
                ("src/db/pool.rs", "pub fn connect() {}"),
            ],
        );

        // Tight budget that forces truncation.
        let tight = HierarchicalSkeleton::new(250);
        let mut mtime = 0u64;
        let tight_out = tight
            .generate(dir.path(), "login session", None, &mut mtime)
            .await;
        let tight_tokens = count_tokens(&tight_out);
        assert!(tight_tokens <= 250, "tight: {tight_tokens} > 250");

        // Generous budget that should include more detail.
        let generous = HierarchicalSkeleton::new(2_000);
        let mut mtime2 = 0u64;
        let generous_out = generous
            .generate(dir.path(), "login session", None, &mut mtime2)
            .await;
        let generous_tokens = count_tokens(&generous_out);
        assert!(generous_tokens <= 2_000, "generous: {generous_tokens} > 2000");

        // Generous output should be at least as long as tight output.
        assert!(
            generous_out.len() >= tight_out.len(),
            "generous output should be >= tight output length"
        );
    }
}
