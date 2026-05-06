//! `GET /files` and `GET /files/content` — read-only filesystem
//! surface for the web UI's `<FilePreviewPanel>`.
//!
//! Both endpoints are scoped to `state.workspace_root`. Every requested
//! path is canonicalized and must `starts_with(workspace_root)` —
//! anything outside is rejected with 400 BAD REQUEST. We deliberately do
//! NOT trust client-supplied absolute paths.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Hard cap on the bytes returned by `/files/content`. Larger files are
/// rejected with 413 Payload Too Large rather than streamed — the
/// browser surface isn't designed to render multi-MB blobs.
const MAX_CONTENT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct FilesQuery {
    /// Path relative to `workspace_root`. Empty / missing means
    /// "list workspace_root itself."
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FileEntry {
    /// Display name (basename).
    pub name: String,
    /// Path relative to `workspace_root`, with `/` separators.
    pub path: String,
    pub is_dir: bool,
    /// Bytes for files; 0 for directories.
    pub size: u64,
}

/// `GET /files?path=<rel>` — directory listing.
pub async fn list_files_handler(
    State(state): State<AppState>,
    Query(q): Query<FilesQuery>,
) -> Result<Json<Vec<FileEntry>>, (StatusCode, String)> {
    let root = &state.workspace_root;
    let target = resolve_relative(root, q.path.as_deref())?;
    if !target.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("not a directory: {}", display_relative(&target, root)),
        ));
    }
    let read_dir = std::fs::read_dir(&target)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("read_dir: {e}")))?;

    let mut entries: Vec<FileEntry> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let name = entry
            .file_name()
            .to_string_lossy()
            .into_owned();
        // Skip hidden + VCS noise to keep the tree usable. The user can
        // surface these via the CLI; the GUI is for navigation, not audit.
        if name.starts_with('.') {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let rel = path
            .strip_prefix(root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        entries.push(FileEntry {
            name,
            path: rel,
            is_dir: metadata.is_dir(),
            size: if metadata.is_file() { metadata.len() } else { 0 },
        });
    }
    // Directories first, then files; alphabetical within each group.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(Json(entries))
}

/// `GET /files/content?path=<rel>` — file read.
pub async fn read_file_handler(
    State(state): State<AppState>,
    Query(q): Query<FilesQuery>,
) -> Result<Response, (StatusCode, String)> {
    let root = &state.workspace_root;
    let path_str = q
        .path
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or((StatusCode::BAD_REQUEST, "path query param required".to_string()))?;
    let target = resolve_relative(root, Some(path_str))?;
    if target.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("path is a directory: {path_str}"),
        ));
    }
    let metadata = std::fs::metadata(&target)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("stat: {e}")))?;
    if metadata.len() > MAX_CONTENT_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("file too large: {} > {MAX_CONTENT_BYTES}", metadata.len()),
        ));
    }
    let body = std::fs::read(&target)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("read: {e}")))?;

    // Sniff binary vs text by looking for NUL bytes in the first 8 KiB.
    // The web UI only shows text; binary files surface a placeholder.
    if is_binary(&body) {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "binary files are not previewable in the web UI".to_string(),
        ));
    }

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(axum::body::Body::from(body))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("response: {e}")))?;
    Ok(response)
}

fn resolve_relative(
    root: &Path,
    path: Option<&str>,
) -> Result<PathBuf, (StatusCode, String)> {
    // Reject absolute paths and `..` segments before touching the
    // filesystem. canonicalize() defends in depth — even if a symlink
    // points outside the workspace, the prefix check below catches it.
    let raw = path.unwrap_or("").trim_start_matches('/');
    if raw.split('/').any(|seg| seg == "..") {
        return Err((StatusCode::BAD_REQUEST, "path contains ..".to_string()));
    }
    let joined = root.join(raw);
    let canonical = joined
        .canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("canonicalize: {e}")))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("root canonicalize: {e}")))?;
    if !canonical.starts_with(&canonical_root) {
        return Err((
            StatusCode::BAD_REQUEST,
            "path escapes workspace_root".to_string(),
        ));
    }
    Ok(canonical)
}

fn display_relative(p: &Path, root: &Path) -> String {
    p.strip_prefix(root)
        .map(|x| x.display().to_string())
        .unwrap_or_else(|_| p.display().to_string())
}

fn is_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    head.contains(&0)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_rejects_absolute_paths_via_traversal() {
        let tmp = TempDir::new().expect("tmp");
        // Absolute paths get treated as relative to root after the
        // leading slash strip — but `/etc/passwd` becomes `etc/passwd`
        // joined to tmp, which canonicalizes outside tmp on most hosts
        // because tmp/etc/passwd doesn't exist. The NOT_FOUND from
        // canonicalize is the actual rejection here.
        let r = resolve_relative(tmp.path(), Some("/etc/passwd"));
        assert!(r.is_err(), "must reject");
    }

    #[test]
    fn resolve_rejects_dot_dot() {
        let tmp = TempDir::new().expect("tmp");
        let r = resolve_relative(tmp.path(), Some("../etc"));
        match r {
            Ok(_) => panic!("must reject"),
            Err((code, _)) => assert_eq!(code, StatusCode::BAD_REQUEST),
        }
    }

    #[test]
    fn resolve_accepts_simple_relative_path() {
        let tmp = TempDir::new().expect("tmp");
        std::fs::write(tmp.path().join("hello.txt"), "hi").expect("write");
        let r = resolve_relative(tmp.path(), Some("hello.txt")).expect("ok");
        assert!(r.ends_with("hello.txt"));
    }

    #[test]
    fn is_binary_detects_nul_bytes() {
        assert!(is_binary(&[1, 2, 0, 3]));
        assert!(!is_binary(b"hello world"));
        assert!(!is_binary(&[]));
    }
}
