//! Memory fixtures: JSONL, one memory per line (F11).

use std::path::Path;

use hone_memory::{Memory, MemoryKind};

/// One fixture line: `id`, `kind` and `content` are required, the rest defaults.
#[derive(serde::Deserialize)]
struct Line {
    id: String,
    kind: MemoryKind,
    content: String,
    #[serde(default)]
    tags: Vec<String>,
    weight: Option<f64>,
    created_at: Option<u64>,
    decay_at: Option<u64>,
    description: Option<String>,
}

/// Load a fixture. Blank lines are ignored. An error names the file and the line.
pub fn load(path: &Path) -> Result<Vec<Memory>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut memories = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let line: Line = serde_json::from_str(raw)
            .map_err(|e| format!("{}: line {}: {e}", path.display(), i + 1))?;
        memories.push(Memory {
            id: line.id,
            content: line.content,
            tags: line.tags,
            weight: line.weight.unwrap_or(1.0),
            created_at: line.created_at.unwrap_or(now),
            decay_at: line.decay_at,
            kind: line.kind,
            description: line.description,
            embedding: None,
        });
    }
    Ok(memories)
}
