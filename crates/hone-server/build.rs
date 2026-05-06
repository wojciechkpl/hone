//! Build-time stub for the embedded web UI.
//!
//! `rust-embed` requires `apps/webui/dist/` to exist at compile time.
//! Fresh clones won't have the SolidJS build output yet, and CI runs
//! that don't care about the web UI shouldn't have to install Node.
//!
//! This script ensures the directory exists with a minimal `index.html`
//! that explains the situation. Real builds (`make webui` then
//! `cargo build`) overwrite the stub with the actual bundle.

use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("apps").join("webui").join("dist"));
    let Some(dist) = dist else {
        return;
    };

    if dist.join("index.html").is_file() {
        // Real build present — leave it alone.
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&dist) {
        // We deliberately do NOT panic — build failures here would
        // block honed compilation for users who don't care about the
        // SPA. Print to stderr instead and move on (the rust-embed
        // macro will then itself panic if its preconditions aren't met,
        // but that's the right place for the failure to surface).
        eprintln!("hone-server build.rs: could not create {}: {e}", dist.display());
        return;
    }

    let stub = include_str!("build_stub_index.html");
    let _ = std::fs::write(dist.join("index.html"), stub);
}
