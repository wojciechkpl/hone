mod edit;
mod glob;
mod grep;
mod multi_edit;
mod read;
mod write;

use std::path::{Path, PathBuf};

use crate::error::ToolError;

pub use edit::EditFileTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use multi_edit::MultiEditTool;
pub use read::ReadFileTool;
pub use write::WriteFileTool;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod proptests {
    use super::*;
    use proptest::prelude::*;
    use tempfile::TempDir;

    proptest! {
        #[test]
        fn paths_without_dotdot_resolve_inside_working_dir(
            segments in prop::collection::vec("[a-z]{1,10}", 1..4)
        ) {
            let dir = TempDir::new().unwrap();
            let path = segments.join("/");
            // Paths without .. should either succeed (if parent exists) or fail gracefully.
            let result = resolve_within_working_dir(dir.path(), &path);
            // Should never panic; on success the result must be inside the working dir.
            match result {
                Ok(resolved) => {
                    let canon_root = dir.path().canonicalize().unwrap();
                    prop_assert!(
                        resolved.starts_with(&canon_root),
                        "resolved {} should start with {}",
                        resolved.display(),
                        canon_root.display()
                    );
                }
                Err(_) => {} // graceful failure is OK (parent dirs don't exist)
            }
        }

        #[test]
        fn dotdot_paths_are_always_rejected(
            prefix in "[a-z]{1,5}",
            suffix in "[a-z]{1,5}",
        ) {
            let dir = TempDir::new().unwrap();
            // Create the prefix dir so canonicalization can proceed past it.
            let sub = dir.path().join(&prefix);
            let _ = std::fs::create_dir_all(&sub);
            let path = format!("{prefix}/../../{suffix}");
            let result = resolve_within_working_dir(dir.path(), &path);
            // A path that escapes the working dir must be rejected.
            prop_assert!(
                result.is_err(),
                "path with .. escaping should be rejected: {}",
                path
            );
        }

        #[test]
        fn resolve_never_panics(path in "\\PC{0,200}") {
            let dir = TempDir::new().unwrap();
            // Arbitrary printable input must never cause a panic.
            let _ = resolve_within_working_dir(dir.path(), &path);
        }
    }
}

/// Resolve `user_path` relative to `working_dir` and verify the result stays
/// within `working_dir`.
///
/// Returns the canonicalized absolute path on success, or `ToolError::InvalidInput`
/// if the resolved path escapes the working directory (e.g. `../../etc/passwd`).
///
/// For paths that don't exist yet (write_file creating new files), we
/// canonicalize the existing ancestor and verify containment.
pub(crate) fn resolve_within_working_dir(
    working_dir: &Path,
    user_path: &str,
) -> Result<PathBuf, ToolError> {
    let joined = working_dir.join(user_path);

    // Try to canonicalize the full path (works when file exists).
    // If the file doesn't exist, walk up to find the first existing ancestor,
    // canonicalize that, and re-append the remaining components.
    let resolved = if joined.exists() {
        joined.canonicalize().map_err(|e| ToolError::InvalidInput {
            reason: format!("failed to resolve path '{}': {e}", joined.display()),
        })?
    } else {
        // Find the deepest existing ancestor.
        let mut ancestor = joined.as_path();
        let mut tail_components = Vec::new();
        loop {
            if let Some(parent) = ancestor.parent() {
                if parent.exists() {
                    tail_components.push(ancestor.file_name().unwrap_or_default().to_os_string());
                    let canon_parent =
                        parent.canonicalize().map_err(|e| ToolError::InvalidInput {
                            reason: format!("failed to resolve parent '{}': {e}", parent.display()),
                        })?;
                    let mut result = canon_parent;
                    for comp in tail_components.into_iter().rev() {
                        result.push(comp);
                    }
                    break result;
                }
                tail_components.push(ancestor.file_name().unwrap_or_default().to_os_string());
                ancestor = parent;
            } else {
                break joined.clone();
            }
        }
    };

    let canon_root = working_dir
        .canonicalize()
        .map_err(|e| ToolError::InvalidInput {
            reason: format!(
                "failed to canonicalize working directory '{}': {e}",
                working_dir.display()
            ),
        })?;

    if !resolved.starts_with(&canon_root) {
        return Err(ToolError::InvalidInput {
            reason: format!(
                "path '{}' resolves to '{}' which is outside the working directory '{}'",
                user_path,
                resolved.display(),
                canon_root.display(),
            ),
        });
    }

    Ok(resolved)
}
