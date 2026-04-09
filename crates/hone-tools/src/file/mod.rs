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
                    tail_components.push(
                        ancestor
                            .file_name()
                            .unwrap_or_default()
                            .to_os_string(),
                    );
                    let canon_parent = parent.canonicalize().map_err(|e| {
                        ToolError::InvalidInput {
                            reason: format!(
                                "failed to resolve parent '{}': {e}",
                                parent.display()
                            ),
                        }
                    })?;
                    let mut result = canon_parent;
                    for comp in tail_components.into_iter().rev() {
                        result.push(comp);
                    }
                    break result;
                }
                tail_components.push(
                    ancestor
                        .file_name()
                        .unwrap_or_default()
                        .to_os_string(),
                );
                ancestor = parent;
            } else {
                break joined.clone();
            }
        }
    };

    let canon_root = working_dir.canonicalize().map_err(|e| ToolError::InvalidInput {
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
