//! Context management metrics.
//!
//! Tracks per-session counters to measure the effectiveness of prompt
//! caching, context trimming, and other context optimization techniques.

use std::sync::atomic::{AtomicU64, Ordering};

/// Thread-safe counters for context management metrics.
#[derive(Debug, Default)]
pub struct ContextMetrics {
    /// Total input tokens sent to providers (pre-cache).
    input_tokens: AtomicU64,
    /// Output tokens received from providers.
    output_tokens: AtomicU64,
    /// Cache hits reported by providers (Anthropic cache_read_input_tokens).
    cache_hits: AtomicU64,
    /// Cache writes (Anthropic cache_creation_input_tokens).
    cache_writes: AtomicU64,
    /// Number of context trim events.
    trim_events: AtomicU64,
    /// Number of messages dropped by trimming.
    messages_dropped: AtomicU64,
    /// Number of tool results compressed.
    tool_results_compressed: AtomicU64,
    /// Number of duplicate file reads avoided via hash dedup.
    dedup_hits: AtomicU64,
    /// Number of phase transition compactions triggered.
    phase_transitions: AtomicU64,
    /// Number of prompt injection detections (Medium or High risk).
    injection_detections: AtomicU64,
}

impl ContextMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_turn_tokens(&self, input: u32, output: u32) {
        self.input_tokens.fetch_add(input as u64, Ordering::Relaxed);
        self.output_tokens.fetch_add(output as u64, Ordering::Relaxed);
    }

    pub fn record_cache(&self, hits: u32, writes: u32) {
        self.cache_hits.fetch_add(hits as u64, Ordering::Relaxed);
        self.cache_writes.fetch_add(writes as u64, Ordering::Relaxed);
    }

    pub fn record_trim(&self, messages_dropped: u32) {
        self.trim_events.fetch_add(1, Ordering::Relaxed);
        self.messages_dropped.fetch_add(messages_dropped as u64, Ordering::Relaxed);
    }

    pub fn record_compression(&self) {
        self.tool_results_compressed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_dedup_hit(&self) {
        self.dedup_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_phase_transition(&self) {
        self.phase_transitions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_injection_detection(&self) {
        self.injection_detections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            input_tokens: self.input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_writes: self.cache_writes.load(Ordering::Relaxed),
            trim_events: self.trim_events.load(Ordering::Relaxed),
            messages_dropped: self.messages_dropped.load(Ordering::Relaxed),
            tool_results_compressed: self.tool_results_compressed.load(Ordering::Relaxed),
            dedup_hits: self.dedup_hits.load(Ordering::Relaxed),
            phase_transitions: self.phase_transitions.load(Ordering::Relaxed),
            injection_detections: self.injection_detections.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hits: u64,
    pub cache_writes: u64,
    pub trim_events: u64,
    pub messages_dropped: u64,
    pub tool_results_compressed: u64,
    pub dedup_hits: u64,
    pub phase_transitions: u64,
    pub injection_detections: u64,
}

impl MetricsSnapshot {
    /// Cache hit rate as a fraction (0.0 to 1.0). Returns 0.0 when there's no data.
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_writes;
        if total == 0 {
            return 0.0;
        }
        self.cache_hits as f64 / total as f64
    }

    /// Input-to-output token ratio. Higher means more context relative to response.
    pub fn io_ratio(&self) -> f64 {
        if self.output_tokens == 0 {
            return 0.0;
        }
        self.input_tokens as f64 / self.output_tokens as f64
    }

    pub fn format_summary(&self) -> String {
        format!(
            "input={} out={} cache_hit_rate={:.1}% trims={} dropped={} compressed={} dedup={} phases={} injections={}",
            self.input_tokens,
            self.output_tokens,
            self.cache_hit_rate() * 100.0,
            self.trim_events,
            self.messages_dropped,
            self.tool_results_compressed,
            self.dedup_hits,
            self.phase_transitions,
            self.injection_detections,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_start_empty() {
        let m = ContextMetrics::new();
        let snap = m.snapshot();
        assert_eq!(snap.input_tokens, 0);
        assert_eq!(snap.output_tokens, 0);
    }

    #[test]
    fn test_metrics_record_tokens() {
        let m = ContextMetrics::new();
        m.record_turn_tokens(100, 50);
        m.record_turn_tokens(200, 30);
        let snap = m.snapshot();
        assert_eq!(snap.input_tokens, 300);
        assert_eq!(snap.output_tokens, 80);
    }

    #[test]
    fn test_cache_hit_rate() {
        let m = ContextMetrics::new();
        m.record_cache(75, 25);
        let snap = m.snapshot();
        assert!((snap.cache_hit_rate() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn test_io_ratio() {
        let m = ContextMetrics::new();
        m.record_turn_tokens(1000, 200);
        let snap = m.snapshot();
        assert!((snap.io_ratio() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_trim_counter() {
        let m = ContextMetrics::new();
        m.record_trim(10);
        m.record_trim(5);
        let snap = m.snapshot();
        assert_eq!(snap.trim_events, 2);
        assert_eq!(snap.messages_dropped, 15);
    }
}
