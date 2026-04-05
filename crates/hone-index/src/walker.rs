use std::path::Path;

use walkdir::WalkDir;

use crate::error::IndexError;
use crate::index::SemanticIndex;

/// Walk `root` recursively, index every `.rs` and `.py` file found, and
/// populate `index`.  Hidden directories and any directory named `target` are
/// skipped.
pub fn walk_and_index(root: &Path, index: &mut SemanticIndex) -> Result<(), IndexError> {
    for entry in WalkDir::new(root)
        .into_iter()
        // depth == 0 is the root itself; never prune it regardless of its name.
        .filter_entry(|e| e.depth() == 0 || !is_skipped(e))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "rs" && ext != "py" {
            continue;
        }

        let content = std::fs::read_to_string(path)?;
        index.index_file(path, &content);
    }

    Ok(())
}

fn is_skipped(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    // Skip hidden files/dirs (starting with '.') and `target` directories.
    if name.starts_with('.') {
        return true;
    }
    if entry.file_type().is_dir() && name == "target" {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn walks_and_indexes_rs_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("lib.rs"), "pub fn example() {}\n").unwrap();
        fs::write(dir.path().join("notes.txt"), "not a source file").unwrap();

        let mut idx = SemanticIndex::new();
        walk_and_index(dir.path(), &mut idx).unwrap();

        assert_eq!(idx.file_count(), 1);
        assert!(!idx.search("example").is_empty());
    }

    #[test]
    fn skips_target_directory() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("generated.rs"), "fn generated() {}\n").unwrap();
        fs::write(dir.path().join("main.rs"), "fn real() {}\n").unwrap();

        let mut idx = SemanticIndex::new();
        walk_and_index(dir.path(), &mut idx).unwrap();

        assert_eq!(idx.file_count(), 1);
        assert!(idx.search("generated").is_empty());
        assert!(!idx.search("real").is_empty());
    }

    #[test]
    fn skips_hidden_directories() {
        let dir = TempDir::new().unwrap();
        let hidden = dir.path().join(".git");
        fs::create_dir(&hidden).unwrap();
        fs::write(hidden.join("hook.rs"), "fn hook() {}\n").unwrap();
        fs::write(dir.path().join("src.rs"), "fn visible() {}\n").unwrap();

        let mut idx = SemanticIndex::new();
        walk_and_index(dir.path(), &mut idx).unwrap();

        assert_eq!(idx.file_count(), 1);
        assert!(idx.search("hook").is_empty());
    }
}
