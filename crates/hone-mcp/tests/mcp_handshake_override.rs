#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T7 of `docs/plans/mcp-timeout-spec.md`: `with_handshake_timeout`.
//! New API assumed (this file fails to compile until it exists):
//! `StdioMcpClient::with_handshake_timeout(self, Duration) -> Self`, overriding
//! the default handshake timeout (max(request timeout, 60 s)) for `connect()`.

use std::time::{Duration, Instant};

use hone_mcp::{McpClient, McpError, StdioMcpClient};

/// Never answers anything.
const SILENT: &str = r#"
import sys
for line in sys.stdin:
    pass
"#;

#[tokio::test]
async fn test_t7_handshake_timeout_should_bound_a_silent_initialize() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("silent.py");
    std::fs::write(&script, SILENT).unwrap();
    let mut client = StdioMcpClient::spawn("python3", &[script.to_str().unwrap()])
        .await
        .unwrap()
        .with_request_timeout(Duration::from_secs(30))
        .with_handshake_timeout(Duration::from_millis(400));

    let start = Instant::now();
    let err = tokio::time::timeout(Duration::from_secs(5), client.connect())
        .await
        .expect("hung: the handshake timeout did not fire")
        .expect_err("no handshake");

    assert!(
        matches!(
            err,
            McpError::Timeout {
                timeout_ms: 400,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(start.elapsed() < Duration::from_millis(1500), "promptly");
}
