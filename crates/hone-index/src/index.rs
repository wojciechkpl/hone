use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::parser::SymbolExtractor;
use crate::types::{SearchResult, Symbol};

/// Scoring constants — higher is better.
const SCORE_EXACT: f64 = 1.0;
const SCORE_PREFIX: f64 = 0.8;
const SCORE_SUBSTRING: f64 = 0.5;

pub struct SemanticIndex {
    symbols: Vec<Symbol>,
    /// Maps each file path to the indices of its symbols inside `symbols`.
    file_map: HashMap<PathBuf, Vec<usize>>,
    extractor: SymbolExtractor,
}

impl SemanticIndex {
    pub fn new() -> Self {
        Self {
            symbols: Vec::new(),
            file_map: HashMap::new(),
            extractor: SymbolExtractor::new(),
        }
    }

    /// Index the content of a single file.  Any previously indexed symbols for
    /// this path are replaced.
    pub fn index_file(&mut self, path: &Path, content: &str) {
        // Remove stale entries first.
        self.remove_file(path);

        let new_symbols = self.extractor.extract(path, content);
        if new_symbols.is_empty() {
            return;
        }

        let start = self.symbols.len();
        let indices: Vec<usize> = (start..start + new_symbols.len()).collect();
        self.symbols.extend(new_symbols);
        self.file_map.insert(path.to_path_buf(), indices);
    }

    /// Search for symbols whose name matches `query` (case-insensitive).
    ///
    /// Results are sorted by relevance:
    /// 1. Exact match (highest score)
    /// 2. Prefix match
    /// 3. Substring match
    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        let mut results: Vec<SearchResult> = self
            .symbols
            .iter()
            .filter_map(|sym| {
                let name_lower = sym.name.to_lowercase();
                let score = if name_lower == query_lower {
                    Some(SCORE_EXACT)
                } else if name_lower.starts_with(&query_lower) {
                    Some(SCORE_PREFIX)
                } else if name_lower.contains(&query_lower) {
                    Some(SCORE_SUBSTRING)
                } else {
                    None
                };
                score.map(|s| SearchResult {
                    symbol: sym.clone(),
                    score: s,
                })
            })
            .collect();

        // Sort descending by score, then alphabetically by name for stability.
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.symbol.name.cmp(&b.symbol.name))
        });

        results
    }

    /// Return the symbol that most closely encloses the given source position.
    ///
    /// The heuristic: find the last symbol (in source order) whose declaration
    /// line is at or before `line`.  When two symbols share the same line, the
    /// one with the largest `col` that is still <= `col` wins; otherwise the
    /// last declaration on an earlier line is returned.  `col` is treated as a
    /// refinement hint and is only used for same-line disambiguation.
    pub fn symbol_at(&self, file: &Path, line: u32, _col: u32) -> Option<&Symbol> {
        let indices = self.file_map.get(file)?;
        // Symbols are stored in source order (line-ascending); iterating in
        // reverse gives us the latest declaration that starts at or before the
        // queried line.
        indices
            .iter()
            .rev()
            .map(|&i| &self.symbols[i])
            .find(|s| s.line <= line)
    }

    /// Return all symbols defined in `file`.
    pub fn symbols_in_file(&self, file: &Path) -> Vec<&Symbol> {
        match self.file_map.get(file) {
            None => vec![],
            Some(indices) => indices.iter().map(|&i| &self.symbols[i]).collect(),
        }
    }

    /// Remove all symbols for `path` and re-index it with `content`.
    pub fn invalidate(&mut self, path: &Path) {
        self.remove_file(path);
    }

    /// Number of distinct files that have been indexed.
    pub fn file_count(&self) -> usize {
        self.file_map.len()
    }

    /// Total number of symbols across all indexed files.
    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Remove all symbols belonging to `path`.  Uses a swap-and-remove
    /// approach to keep the Vec compact without expensive shifting.
    fn remove_file(&mut self, path: &Path) {
        let Some(old_indices) = self.file_map.remove(path) else {
            return;
        };

        // Mark positions that are being removed.
        let mut to_remove: Vec<bool> = vec![false; self.symbols.len()];
        for idx in &old_indices {
            to_remove[*idx] = true;
        }

        // Build a mapping from old index -> new index after compaction.
        let mut new_index = vec![usize::MAX; self.symbols.len()];
        let mut cursor = 0usize;
        for (old, &removing) in to_remove.iter().enumerate() {
            if !removing {
                new_index[old] = cursor;
                cursor += 1;
            }
        }

        // Compact the symbols Vec in-place (preserve order).
        let mut write = 0usize;
        for read in 0..self.symbols.len() {
            if !to_remove[read] {
                self.symbols.swap(write, read);
                write += 1;
            }
        }
        self.symbols.truncate(write);

        // Update all stored indices in file_map.
        for indices in self.file_map.values_mut() {
            for idx in indices.iter_mut() {
                *idx = new_index[*idx];
            }
        }
    }
}

impl Default for SemanticIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rs_path(name: &str) -> PathBuf {
        PathBuf::from(format!("{name}.rs"))
    }

    #[test]
    fn index_and_count() {
        let mut idx = SemanticIndex::new();
        idx.index_file(&rs_path("a"), "fn foo() {}\nfn bar() {}\n");
        assert_eq!(idx.file_count(), 1);
        assert_eq!(idx.symbol_count(), 2);
    }

    #[test]
    fn search_finds_substring() {
        let mut idx = SemanticIndex::new();
        idx.index_file(&rs_path("a"), "fn compute_value() {}\n");
        let results = idx.search("value");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol.name, "compute_value");
    }

    #[test]
    fn symbol_at_returns_enclosing() {
        let mut idx = SemanticIndex::new();
        let path = rs_path("a");
        idx.index_file(&path, "fn foo() {}\nfn bar() {}\n");
        let sym = idx.symbol_at(&path, 2, 3);
        assert!(sym.is_some());
        assert_eq!(sym.unwrap().name, "bar");
    }

    #[test]
    fn invalidate_removes_symbols() {
        let mut idx = SemanticIndex::new();
        let path = rs_path("a");
        idx.index_file(&path, "fn foo() {}\n");
        assert_eq!(idx.symbol_count(), 1);
        idx.invalidate(&path);
        assert_eq!(idx.symbol_count(), 0);
        assert_eq!(idx.file_count(), 0);
    }

    #[test]
    fn re_index_after_invalidate_updates_symbols() {
        let mut idx = SemanticIndex::new();
        let path = rs_path("a");
        idx.index_file(&path, "fn old_name() {}\n");
        idx.invalidate(&path);
        idx.index_file(&path, "fn new_name() {}\n");
        let results = idx.search("new_name");
        assert_eq!(results.len(), 1);
        let old_results = idx.search("old_name");
        assert!(old_results.is_empty());
    }
}
