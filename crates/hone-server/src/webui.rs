//! Static asset serving for the SolidJS web UI.
//!
//! In release builds the contents of `apps/webui/dist/` are embedded
//! directly into the binary via `rust-embed`. In debug builds the
//! crate falls back to filesystem reads for fast iteration. Both modes
//! transparently 404 when the dist directory is missing — it's not
//! required for honed to start.
//!
//! Routes:
//! - `GET /ui` and `GET /ui/` → serve `index.html`
//! - `GET /ui/<path>` → serve a hashed asset, with the SPA fallback
//!   (any path that doesn't match an asset returns `index.html` so
//!   client-side routing works on first navigation)

use axum::body::Body;
use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../apps/webui/dist/"]
struct WebUiAssets;

const INDEX: &str = "index.html";

/// `GET /ui` — serve the SPA shell. Equivalent to a `/ui/` request.
pub async fn serve_index() -> Response {
    serve_asset_or_index(INDEX)
}

/// `GET /ui/{*path}` — serve a hashed asset. If the path doesn't match
/// any embedded file, fall through to `index.html` so the SPA can route
/// client-side. The same fallback handles refresh-on-deep-link.
pub async fn serve_asset(Path(path): Path<String>) -> Response {
    let normalised = normalize_request_path(&path);
    serve_asset_or_index(&normalised)
}

fn serve_asset_or_index(path: &str) -> Response {
    if let Some(file) = WebUiAssets::get(path) {
        return embedded_file_response(path, file.data.as_ref());
    }
    // SPA fallback: index.html.
    if let Some(file) = WebUiAssets::get(INDEX) {
        return embedded_file_response(INDEX, file.data.as_ref());
    }
    // Web UI was not built into this binary. The desktop app + TUI
    // still work; this just means the browser surface is unavailable.
    (
        StatusCode::NOT_FOUND,
        "web UI is not bundled into this honed binary. Run `make webui` and rebuild.",
    )
        .into_response()
}

fn embedded_file_response(path: &str, body: &[u8]) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if path.starts_with("assets/") {
        // Hashed asset: long cache.
        "public, max-age=31536000, immutable"
    } else {
        // index.html and the favicon: short cache so deploys are seen.
        "public, max-age=60"
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.essence_str())
        .header(header::CACHE_CONTROL, HeaderValue::from_static(cache_control))
        .body(Body::from(body.to_vec()));
    if response.is_err() {
        // Header value construction failed — fall back to a plain 200.
        response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(body.to_vec()));
    }
    response.unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "response build failed").into_response()
    })
}

/// Strip a leading `/`, reject `..`, and treat empty-string as `index.html`.
fn normalize_request_path(p: &str) -> String {
    let trimmed = p.trim_start_matches('/');
    if trimmed.is_empty() || trimmed.contains("..") {
        return INDEX.to_owned();
    }
    trimmed.to_owned()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_leading_slash() {
        assert_eq!(normalize_request_path("/assets/x.js"), "assets/x.js");
        assert_eq!(normalize_request_path("assets/x.js"), "assets/x.js");
    }

    #[test]
    fn normalize_falls_back_to_index_for_empty_or_traversal() {
        assert_eq!(normalize_request_path(""), INDEX);
        assert_eq!(normalize_request_path("/"), INDEX);
        assert_eq!(normalize_request_path("/../etc/passwd"), INDEX);
        assert_eq!(normalize_request_path("a/../b"), INDEX);
    }

    #[tokio::test]
    async fn serve_index_returns_200_or_404_depending_on_bundle() {
        let resp = serve_index().await;
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND,
            "unexpected status: {}",
            resp.status()
        );
    }
}
