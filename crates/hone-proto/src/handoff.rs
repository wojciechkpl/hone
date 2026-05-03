use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandoffRequest {
    pub target_agent: String,
    pub payload: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_agent: Option<String>,
}

#[derive(Default, Clone)]
pub struct HandoffBuffer {
    inner: Arc<Mutex<Vec<HandoffRequest>>>,
}

impl HandoffBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, request: HandoffRequest) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.push(request);
        }
    }

    pub fn drain(&self) -> Vec<HandoffRequest> {
        match self.inner.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(_) => Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(target: &str, payload: &str) -> HandoffRequest {
        HandoffRequest {
            target_agent: target.into(),
            payload: payload.into(),
            reason: "test".into(),
            origin_agent: None,
        }
    }

    #[test]
    fn handoff_request_serde_roundtrip() {
        let r = HandoffRequest {
            target_agent: "code-reviewer".into(),
            payload: "review src/auth.rs".into(),
            reason: "needs second look".into(),
            origin_agent: Some("debugger".into()),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: HandoffRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn buffer_starts_empty() {
        let b = HandoffBuffer::new();
        assert_eq!(b.len(), 0);
        assert!(b.is_empty());
    }

    #[test]
    fn buffer_record_appends() {
        let b = HandoffBuffer::new();
        b.record(req("a", "x"));
        b.record(req("b", "y"));
        assert_eq!(b.len(), 2);
        assert!(!b.is_empty());
    }

    #[test]
    fn buffer_drain_returns_in_insertion_order_and_clears() {
        let b = HandoffBuffer::new();
        b.record(req("first", "1"));
        b.record(req("second", "2"));
        b.record(req("third", "3"));
        let v = b.drain();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].target_agent, "first");
        assert_eq!(v[1].target_agent, "second");
        assert_eq!(v[2].target_agent, "third");
        assert!(b.is_empty());
    }

    #[test]
    fn buffer_drain_when_empty_returns_empty_vec() {
        let b = HandoffBuffer::new();
        assert!(b.drain().is_empty());
    }

    #[test]
    fn buffer_clone_shares_storage() {
        let original = HandoffBuffer::new();
        let clone = original.clone();
        clone.record(req("x", "y"));
        assert_eq!(original.len(), 1);
    }

    #[test]
    fn buffer_len_and_is_empty_consistent() {
        let b = HandoffBuffer::new();
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
        b.record(req("a", "x"));
        assert!(!b.is_empty());
        assert_eq!(b.len(), 1);
    }
}
