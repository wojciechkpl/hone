use std::collections::HashMap;
use std::path::Path;

use crate::error::IndexError;
use crate::index::SemanticIndex;
use crate::types::SearchResult;
use crate::walker::walk_and_index;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MultiRepoResult {
    pub repo: String,
    pub result: SearchResult,
}

pub struct MultiRepoIndex {
    repos: HashMap<String, SemanticIndex>,
}

impl MultiRepoIndex {
    pub fn new() -> Self {
        Self {
            repos: HashMap::new(),
        }
    }

    /// Add a repo by walking its directory and indexing all files.
    pub fn add_repo(&mut self, name: &str, root: &Path) -> Result<(), IndexError> {
        let mut index = SemanticIndex::new();
        walk_and_index(root, &mut index)?;
        self.repos.insert(name.to_owned(), index);
        Ok(())
    }

    /// Remove a repo and all its indexed symbols.
    pub fn remove_repo(&mut self, name: &str) {
        self.repos.remove(name);
    }

    /// Search across all repos, or optionally scoped to one repo.
    /// Results are sorted by score descending across all repos.
    pub fn search(&self, query: &str, repo: Option<&str>) -> Vec<MultiRepoResult> {
        let mut results: Vec<MultiRepoResult> = match repo {
            Some(name) => match self.repos.get(name) {
                Some(index) => index
                    .search(query)
                    .into_iter()
                    .map(|result| MultiRepoResult {
                        repo: name.to_owned(),
                        result,
                    })
                    .collect(),
                None => vec![],
            },
            None => self
                .repos
                .iter()
                .flat_map(|(name, index)| {
                    index.search(query).into_iter().map(move |result| MultiRepoResult {
                        repo: name.clone(),
                        result,
                    })
                })
                .collect(),
        };

        results.sort_by(|a, b| {
            b.result
                .score
                .partial_cmp(&a.result.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.result.symbol.name.cmp(&b.result.symbol.name))
        });

        results
    }

    /// Number of indexed repos.
    pub fn repo_count(&self) -> usize {
        self.repos.len()
    }

    /// Total symbols across all repos.
    pub fn total_symbol_count(&self) -> usize {
        self.repos.values().map(|idx| idx.symbol_count()).sum()
    }
}

impl Default for MultiRepoIndex {
    fn default() -> Self {
        Self::new()
    }
}
