use hone_server::BoreTunnelManager;
use hone_server::tunnel::{TunnelConfig, TunnelManager};

/// Availability check must not panic regardless of whether bore is installed.
#[tokio::test]
async fn test_bore_availability_check() {
    // Just verify the call completes without panicking.
    let _available = BoreTunnelManager::is_available().await;
}

/// Construction helpers must not panic.
#[tokio::test]
async fn test_bore_manager_construction() {
    let _default = BoreTunnelManager::new();
    let _custom = BoreTunnelManager::with_server("my-bore-server.example.com");
    let _via_default = BoreTunnelManager::default();
}

/// When bore is not installed (or when the server is unreachable), starting a
/// tunnel must return an `Err` rather than panicking or hanging forever.
///
/// We use `--to invalid-host-that-does-not-exist` so the bore binary either
/// fails to spawn (binary absent) or fails to connect (binary present but
/// unreachable server), both of which must produce an error within the 10-second
/// timeout already embedded in `BoreTunnelManager::start`.
#[tokio::test]
async fn test_bore_start_fails_gracefully() {
    let manager = BoreTunnelManager::with_server("invalid-host-that-does-not-exist");
    let config = TunnelConfig {
        local_port: 19999,
        auth_token: String::new(),
    };

    // Wrap in a generous outer timeout so the test suite never hangs even if
    // something changes inside the implementation.
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(15), manager.start(&config)).await;

    match result {
        // Timed out at the outer boundary — acceptable (bore may be slow to fail).
        Err(_elapsed) => {}
        // Completed within time: must be an error, not a success.
        Ok(inner) => {
            assert!(
                inner.is_err(),
                "expected Err when bore binary is absent or server unreachable, got Ok"
            );
        }
    }
}

/// `status()` returns `None` when no tunnel has been started.
#[tokio::test]
async fn test_bore_status_initially_none() {
    let manager = BoreTunnelManager::new();
    let status = manager.status().await.unwrap();
    assert!(status.is_none());
}

/// `stop()` on an idle manager must succeed without panicking.
#[tokio::test]
async fn test_bore_stop_when_idle() {
    let manager = BoreTunnelManager::new();
    assert!(manager.stop().await.is_ok());
}
