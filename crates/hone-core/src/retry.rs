//! Exponential backoff retry wrapper for async operations.

use std::future::Future;
use std::time::Duration;

/// Configuration for retry with exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    pub fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            ..Self::default()
        }
    }
}

/// Run an async operation with exponential backoff retry.
///
/// Retries only when `should_retry(&error)` returns true. On success,
/// returns immediately. On final failure, returns the last error.
pub async fn retry<F, Fut, T, E>(
    config: &RetryConfig,
    mut operation: F,
    mut should_retry: impl FnMut(&E) -> bool,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempt = 0;
    let mut delay = config.initial_delay;

    loop {
        attempt += 1;
        match operation().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                if attempt >= config.max_attempts || !should_retry(&e) {
                    return Err(e);
                }
                tracing::debug!(
                    attempt,
                    max_attempts = config.max_attempts,
                    delay_ms = delay.as_millis(),
                    "retrying after error"
                );
                tokio::time::sleep(delay).await;
                delay = std::cmp::min(
                    Duration::from_millis((delay.as_millis() as f64 * config.multiplier) as u64),
                    config.max_delay,
                );
            }
        }
    }
}

/// Determine if an error string looks transient and worth retrying.
pub fn is_transient_error(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();
    lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("timed out")
        || lower.contains("temporarily unavailable")
        || lower.contains("503")
        || lower.contains("502")
        || lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("dns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_retry_succeeds_immediately() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let result: Result<&str, &str> = retry(
            &RetryConfig::default(),
            || {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, &str>("success")
                }
            },
            |_e| true,
        )
        .await;

        assert_eq!(result, Ok("success"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_retries_on_transient() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            multiplier: 2.0,
        };

        let result: Result<&str, String> = retry(
            &config,
            || {
                let c = count_clone.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err::<&str, String>("connection timed out".to_string())
                    } else {
                        Ok("success")
                    }
                }
            },
            |e| is_transient_error(e),
        )
        .await;

        assert_eq!(result, Ok("success"));
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_gives_up_after_max_attempts() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let config = RetryConfig {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            multiplier: 2.0,
        };

        let result: Result<&str, String> = retry(
            &config,
            || {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<&str, String>("timed out".to_string())
                }
            },
            |_e| true,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_does_not_retry_permanent_errors() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let result: Result<&str, String> = retry(
            &RetryConfig::default(),
            || {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<&str, String>("invalid argument".to_string())
                }
            },
            |e| is_transient_error(e),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1); // no retries
    }

    #[test]
    fn test_is_transient_classifies_network() {
        assert!(is_transient_error("connection refused"));
        assert!(is_transient_error("request timed out"));
        assert!(is_transient_error("HTTP 503"));
        assert!(is_transient_error("Rate limit exceeded"));
    }

    #[test]
    fn test_is_transient_rejects_permanent() {
        assert!(!is_transient_error("invalid api key"));
        assert!(!is_transient_error("401 unauthorized"));
        assert!(!is_transient_error("file not found"));
    }
}
