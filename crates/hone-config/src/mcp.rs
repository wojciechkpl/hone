use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Configuration for a single MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Executable to spawn.
    pub command: String,
    /// Arguments passed to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Additional env vars. Values may reference `${VAR}` for env interpolation.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// When `true`, skip this server at startup.
    #[serde(default)]
    pub disabled: bool,
    /// Per-request timeout in milliseconds.
    #[serde(default = "McpServerConfig::default_timeout_ms")]
    pub timeout_ms: u64,
}

impl McpServerConfig {
    fn default_timeout_ms() -> u64 {
        10_000
    }
}

/// Top-level structure of `.hone/mcp.json` and `~/.config/hone/mcp.json`.
///
/// The format is intentionally compatible with Claude Code's `.claude/mcp.json`
/// so users can copy or symlink their existing configs without modification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

impl McpConfig {
    /// Load and merge global + project config files.
    ///
    /// Project settings win over global settings when server names collide.
    /// Files that are missing or malformed are silently skipped — a bad config
    /// must never prevent Hone from starting.
    pub fn load(global_path: Option<&Path>, project_path: Option<&Path>) -> Self {
        let mut merged = Self::default();
        for path in [global_path, project_path].into_iter().flatten() {
            match std::fs::read_to_string(path) {
                Ok(text) => match serde_json::from_str::<McpConfig>(&text) {
                    Ok(cfg) => {
                        merged.mcp_servers.extend(cfg.mcp_servers);
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "failed to parse MCP config, skipping"
                        );
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Not found is expected when the file doesn't exist yet.
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to read MCP config, skipping"
                    );
                }
            }
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_temp_json(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn empty_when_no_files() {
        let cfg = McpConfig::load(None, None);
        assert!(cfg.mcp_servers.is_empty());
    }

    #[test]
    fn parses_minimal_server() {
        let json = r#"{"mcpServers": {"fs": {"command": "npx", "args": ["-y", "server-fs"]}}}"#;
        let f = write_temp_json(json);
        let cfg = McpConfig::load(Some(f.path()), None);

        assert_eq!(cfg.mcp_servers.len(), 1);
        let server = cfg.mcp_servers.get("fs").unwrap();
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, ["-y", "server-fs"]);
        assert!(!server.disabled);
        assert_eq!(server.timeout_ms, 10_000);
    }

    #[test]
    fn project_overrides_global_on_collision() {
        let global = write_temp_json(
            r#"{"mcpServers": {"shared": {"command": "global-cmd"}, "only-global": {"command": "og"}}}"#,
        );
        let project = write_temp_json(
            r#"{"mcpServers": {"shared": {"command": "project-cmd"}, "only-project": {"command": "op"}}}"#,
        );
        let cfg = McpConfig::load(Some(global.path()), Some(project.path()));

        // Project wins for "shared".
        assert_eq!(cfg.mcp_servers["shared"].command, "project-cmd");
        // Both non-colliding servers are present.
        assert!(cfg.mcp_servers.contains_key("only-global"));
        assert!(cfg.mcp_servers.contains_key("only-project"));
        assert_eq!(cfg.mcp_servers.len(), 3);
    }

    #[test]
    fn missing_file_is_silently_skipped() {
        let cfg = McpConfig::load(
            Some(Path::new("/no/such/file.json")),
            None,
        );
        assert!(cfg.mcp_servers.is_empty());
    }

    #[test]
    fn malformed_file_is_silently_skipped() {
        let f = write_temp_json("not json at all {{{");
        let cfg = McpConfig::load(Some(f.path()), None);
        assert!(cfg.mcp_servers.is_empty());
    }

    #[test]
    fn disabled_field_defaults_to_false() {
        let json = r#"{"mcpServers": {"s": {"command": "cmd"}}}"#;
        let f = write_temp_json(json);
        let cfg = McpConfig::load(Some(f.path()), None);
        assert!(!cfg.mcp_servers["s"].disabled);
    }

    #[test]
    fn env_vars_are_parsed() {
        let json = r#"{
            "mcpServers": {
                "gh": {
                    "command": "npx",
                    "env": {"GITHUB_TOKEN": "${GITHUB_PERSONAL_ACCESS_TOKEN}"}
                }
            }
        }"#;
        let f = write_temp_json(json);
        let cfg = McpConfig::load(Some(f.path()), None);
        let server = &cfg.mcp_servers["gh"];
        assert_eq!(
            server.env.get("GITHUB_TOKEN").map(String::as_str),
            Some("${GITHUB_PERSONAL_ACCESS_TOKEN}")
        );
    }

    #[test]
    fn explicit_timeout_is_respected() {
        let json = r#"{"mcpServers": {"s": {"command": "cmd", "timeout_ms": 5000}}}"#;
        let f = write_temp_json(json);
        let cfg = McpConfig::load(Some(f.path()), None);
        assert_eq!(cfg.mcp_servers["s"].timeout_ms, 5_000);
    }

    #[test]
    fn claude_code_compatible_format() {
        // This is the exact format from the design doc, lifted from Claude Code's .claude/mcp.json.
        let json = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
                },
                "github": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"}
                }
            }
        }"#;
        let f = write_temp_json(json);
        let cfg = McpConfig::load(Some(f.path()), None);
        assert_eq!(cfg.mcp_servers.len(), 2);
        assert!(cfg.mcp_servers.contains_key("filesystem"));
        assert!(cfg.mcp_servers.contains_key("github"));
    }
}
