#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T10 of `docs/plans/mcp-timeout-spec.md`: the optional per-server
//! `handshake_timeout_ms` in `mcp.json`. Assumed: `McpServerConfig` gains
//! `handshake_timeout_ms: Option<u64>` (`#[serde(default)]`; absent = `None`).
//! Fails to compile until the field exists.

use hone_config::mcp::{McpConfig, McpServerConfig};

fn parse(json: &str) -> McpServerConfig {
    serde_json::from_str(json).unwrap()
}

#[test]
fn test_t10_config_without_the_field_should_parse_unchanged() {
    let cfg = parse(r#"{ "command": "npx", "args": ["-y", "x"] }"#);

    assert_eq!(cfg.handshake_timeout_ms, None);
    assert_eq!(cfg.timeout_ms, 10_000, "the existing default is unchanged");
}

#[test]
fn test_t10_config_with_the_field_should_carry_it() {
    let cfg = parse(r#"{ "command": "npx", "timeout_ms": 5000, "handshake_timeout_ms": 120000 }"#);

    assert_eq!(cfg.handshake_timeout_ms, Some(120_000));
    assert_eq!(cfg.timeout_ms, 5000);
}

#[test]
fn test_t10_zero_should_be_kept_as_zero_meaning_no_handshake_timeout() {
    let cfg = parse(r#"{ "command": "npx", "handshake_timeout_ms": 0 }"#);

    assert_eq!(cfg.handshake_timeout_ms, Some(0));
}

#[test]
fn test_t10_whole_mcp_json_should_parse_with_a_mix_of_servers() {
    let config: McpConfig = serde_json::from_str(
        r#"{ "mcpServers": {
              "a": { "command": "x" },
              "b": { "command": "y", "handshake_timeout_ms": 300 }
        } }"#,
    )
    .unwrap();

    assert_eq!(config.mcp_servers["a"].handshake_timeout_ms, None);
    assert_eq!(config.mcp_servers["b"].handshake_timeout_ms, Some(300));
}
