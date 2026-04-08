use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Simple sliding-window rate limiter keyed by client identifier.
#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<RateLimiterInner>>,
    max_requests: u32,
    window: Duration,
}

struct RateLimiterInner {
    clients: HashMap<String, Vec<Instant>>,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RateLimiterInner {
                clients: HashMap::new(),
            })),
            max_requests,
            window,
        }
    }

    /// Returns true if the request should be allowed.
    pub async fn check(&self, client_id: &str) -> bool {
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        let window_start = now - self.window;

        let entries = inner.clients.entry(client_id.to_string()).or_default();

        // Remove old entries outside the window.
        entries.retain(|&t| t >= window_start);

        if entries.len() >= self.max_requests as usize {
            return false;
        }

        entries.push(now);
        true
    }

    /// Clean up old entries across all clients (call periodically).
    pub async fn cleanup(&self) {
        let mut inner = self.inner.lock().await;
        let window_start = Instant::now() - self.window;
        inner.clients.retain(|_, entries| {
            entries.retain(|&t| t >= window_start);
            !entries.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limit_allows_within_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(1));
        assert!(limiter.check("client-a").await);
        assert!(limiter.check("client-a").await);
        assert!(limiter.check("client-a").await);
    }

    #[tokio::test]
    async fn test_rate_limit_blocks_over_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(1));
        assert!(limiter.check("client-b").await);
        assert!(limiter.check("client-b").await);
        assert!(limiter.check("client-b").await);
        assert!(!limiter.check("client-b").await, "4th request must be denied");
    }

    #[tokio::test]
    async fn test_rate_limit_different_clients_separate() {
        let limiter = RateLimiter::new(2, Duration::from_secs(1));

        // Fill up client-x's budget.
        assert!(limiter.check("client-x").await);
        assert!(limiter.check("client-x").await);
        assert!(!limiter.check("client-x").await);

        // client-y still has its own fresh budget.
        assert!(limiter.check("client-y").await);
        assert!(limiter.check("client-y").await);
        assert!(!limiter.check("client-y").await);
    }

    #[tokio::test]
    async fn test_rate_limit_window_expires() {
        let limiter = RateLimiter::new(2, Duration::from_millis(50));

        assert!(limiter.check("client-z").await);
        assert!(limiter.check("client-z").await);
        // Budget exhausted.
        assert!(!limiter.check("client-z").await);

        // Wait for the window to slide past all recorded entries.
        tokio::time::sleep(Duration::from_millis(60)).await;

        // After the window has expired, the count resets.
        assert!(
            limiter.check("client-z").await,
            "request must be allowed after window expires"
        );
    }
}
