use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::distributed::DistributedTaskStore;

/// Spawn a background task that reaps expired leases every `interval`.
/// Returns the JoinHandle so the caller can abort on shutdown.
///
/// Per design doc §5.3: default interval 15s, max_attempts 3.
pub fn spawn_lease_reaper(
    store: Arc<DistributedTaskStore>,
    interval: Duration,
    max_attempts: u32,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            match store
                .reap_expired_leases(chrono::Utc::now(), max_attempts)
                .await
            {
                Ok(n) if n > 0 => tracing::info!("reaped {n} expired leases"),
                Ok(_) => {}
                Err(e) => tracing::warn!("reaper error: {e}"),
            }
        }
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use chrono::Utc;

    use crate::distributed::{DistributedTaskStore, WorkerId};
    use hone_core::task_plan::SubTask;

    fn make_subtask(id: &str) -> SubTask {
        SubTask {
            id: id.to_owned(),
            description: format!("task {id}"),
            agent: "test-agent".to_owned(),
            context: String::new(),
            depends_on: vec![],
            acceptance_tests: vec![],
            budget_usd: None,
            scope: vec![],
            requires_capabilities: vec![],
        }
    }

    // ── spawn/abort sanity ────────────────────────────────────────────────────

    #[tokio::test]
    async fn reaper_spawned_handle_is_running() {
        let store = Arc::new(DistributedTaskStore::open_in_memory().await.unwrap());
        let handle = spawn_lease_reaper(store, Duration::from_millis(200), 3);
        // A freshly spawned JoinHandle must not yet be finished.
        assert!(!handle.is_finished(), "reaper handle should be running immediately after spawn");
        handle.abort();
    }

    #[tokio::test]
    async fn reaper_can_be_aborted() {
        let store = Arc::new(DistributedTaskStore::open_in_memory().await.unwrap());
        let handle = spawn_lease_reaper(store, Duration::from_millis(50), 3);
        handle.abort();

        // Allow one yield for the task to notice the abort signal.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(handle.is_finished(), "aborted reaper must be done within 100ms");
    }

    // ── reap behaviour ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reaper_reclaims_expired_lease_within_one_interval() {
        // Use a very short lease (100ms) and a tight reap interval (50ms) so
        // the test finishes in <1 second without sleeping long.
        let store = Arc::new(DistributedTaskStore::open_in_memory().await.unwrap());

        store
            .enqueue("plan-reap", vec![make_subtask("s1")])
            .await
            .unwrap();

        let wid = WorkerId::new("worker-1");
        // Claim with 100ms lease — the store uses the real clock here.
        store
            .claim_one(&wid, &[], Duration::from_millis(100))
            .await
            .unwrap()
            .expect("task must be claimable");

        // Immediately confirm the task is leased.
        let (_, leased, _, _, _) = store.plan_state_counts("plan-reap").await.unwrap();
        assert_eq!(leased, 1, "task should be leased before reaper runs");

        // Spawn reaper with 50ms interval.
        let handle = spawn_lease_reaper(Arc::clone(&store), Duration::from_millis(50), 3);

        // Wait 300ms — enough for the 100ms lease to expire and the 50ms reaper
        // to fire at least once after expiry.
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.abort();

        let (queued, leased, done, failed, _) =
            store.plan_state_counts("plan-reap").await.unwrap();
        assert_eq!(
            queued, 1,
            "expired lease should be re-queued by reaper; got queued={queued} leased={leased} done={done} failed={failed}"
        );
        assert_eq!(leased, 0);
    }

    #[tokio::test]
    async fn reaper_force_fails_when_max_attempts_exceeded() {
        // Exhaust max_attempts=2 via the reaper, then assert the task is `failed`.
        let store = Arc::new(DistributedTaskStore::open_in_memory().await.unwrap());
        let max_attempts = 2u32;

        store
            .enqueue("plan-exhaust", vec![make_subtask("t1")])
            .await
            .unwrap();

        let wid = WorkerId::new("worker-1");

        // Cycle: claim then let the reaper expire it, until the task is permanently failed.
        // We drive this manually (no reaper spawn) to stay deterministic.
        for _ in 0..max_attempts {
            let claimed = store
                .claim_one(&wid, &[], Duration::from_millis(1))
                .await
                .unwrap();
            // If already failed, the loop exits naturally.
            if claimed.is_none() {
                break;
            }
            // Reap immediately in the past (well past expiry).
            let future = Utc::now() + chrono::Duration::seconds(10);
            store
                .reap_expired_leases(future, max_attempts)
                .await
                .unwrap();
        }

        let (queued, leased, done, failed, _) =
            store.plan_state_counts("plan-exhaust").await.unwrap();
        assert_eq!(
            failed, 1,
            "task should be force-failed after {max_attempts} reaps; got q={queued} l={leased} d={done} f={failed}"
        );
        assert_eq!(queued + leased + done, 0);
    }

    #[tokio::test]
    async fn reaper_does_not_touch_unexpired_leases() {
        let store = Arc::new(DistributedTaskStore::open_in_memory().await.unwrap());

        store
            .enqueue("plan-unexpired", vec![make_subtask("s1")])
            .await
            .unwrap();

        let wid = WorkerId::new("worker-1");
        // Claim with a 10-second lease — will not expire during the test.
        store
            .claim_one(&wid, &[], Duration::from_secs(10))
            .await
            .unwrap()
            .expect("task must be claimable");

        // Run the reaper with a 50ms interval for 200ms — not enough time
        // for the 10s lease to expire.
        let handle = spawn_lease_reaper(Arc::clone(&store), Duration::from_millis(50), 3);
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        let (queued, leased, _, _, _) =
            store.plan_state_counts("plan-unexpired").await.unwrap();
        assert_eq!(
            leased, 1,
            "unexpired lease must not be touched by reaper; got queued={queued} leased={leased}"
        );
        assert_eq!(queued, 0);
    }
}
