#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P8 of `docs/plans/mcp-parallel-connect-spec.md`: a server's "MCP server
//! connected" / "failed to start, skipping" line is logged when that server
//! finishes, not when the slowest one does. Today everything is logged after
//! the slowest server (the hung one's handshake timeout) has ended.
//!
//! Events are captured by a small `tracing` layer that records the message,
//! the `server` field and the time of each event. The test runs on the
//! current-thread runtime so `set_default` (thread-local) sees every event.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hone_config::mcp::McpConfig;
use hone_mcp::McpManager;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

#[derive(Clone, Debug)]
struct Seen {
    at: Instant,
    message: String,
    server: String,
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Seen>>>);

#[derive(Default)]
struct Fields {
    message: String,
    server: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let text = format!("{value:?}");
        match field.name() {
            "message" => self.message = text,
            "server" => self.server = text.trim_matches('"').to_owned(),
            _ => {}
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_owned(),
            "server" => self.server = value.to_owned(),
            _ => {}
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut f = Fields::default();
        event.record(&mut f);
        self.0.lock().unwrap().push(Seen {
            at: Instant::now(),
            message: f.message,
            server: f.server,
        });
    }
}

const HEALTHY: &str = r#"
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    result = {"tools": []} if req.get("method") == "tools/list" else {"serverInfo": {"name": "ok", "version": "1"}}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

/// Never answers; its pid is written to argv[1].
const HUNG: &str = r#"
import os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
while True:
    time.sleep(1)
"#;

#[tokio::test]
async fn test_p8_a_healthy_servers_connected_line_should_not_wait_for_the_hung_one() {
    let dir = tempfile::tempdir().unwrap();
    let healthy = dir.path().join("healthy.py");
    let hung = dir.path().join("hung.py");
    let pid_file = dir.path().join("hung.pid");
    std::fs::write(&healthy, HEALTHY).unwrap();
    std::fs::write(&hung, HUNG).unwrap();
    let config: McpConfig = serde_json::from_value(serde_json::json!({
        "mcpServers": {
            "ok": { "command": "python3", "args": [healthy.to_str().unwrap()] },
            "hung": {
                "command": "python3",
                "args": [hung.to_str().unwrap(), pid_file.to_str().unwrap()],
                "timeout_ms": 3000,
                "handshake_timeout_ms": 3000
            }
        }
    }))
    .unwrap();

    let capture = Capture::default();
    let subscriber = Registry::default().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let start = Instant::now();
    let manager = tokio::time::timeout(Duration::from_secs(15), McpManager::from_config(&config))
        .await
        .expect("startup hung");
    let total = start.elapsed();

    let seen = capture.0.lock().unwrap().clone();
    let connected = seen
        .iter()
        .find(|s| s.message.contains("MCP server connected") && s.server == "ok")
        .unwrap_or_else(|| panic!("no `connected` line for `ok`: {seen:?}"));
    let skipped = seen
        .iter()
        .find(|s| s.message.contains("failed to start, skipping") && s.server == "hung")
        .unwrap_or_else(|| panic!("no `skipping` line for `hung`: {seen:?}"));

    assert!(
        total >= Duration::from_millis(2500),
        "the hung server's handshake ran its 3 s timeout (took {total:?})"
    );
    let connected_after = connected.at.duration_since(start);
    assert!(
        connected_after < Duration::from_millis(1500),
        "`ok` connected line logged {connected_after:?} after start: it waited for the hung server"
    );
    assert!(
        connected.at < skipped.at,
        "the healthy server's line comes first"
    );
    assert_eq!(manager.server_count(), 1);
    if let Ok(pid) = std::fs::read_to_string(&pid_file) {
        let _ = std::process::Command::new("kill")
            .args(["-9", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status();
    }
    manager.shutdown().await;
}
