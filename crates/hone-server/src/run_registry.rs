//! Run registry — maps client-supplied `request_id` to the in-flight
//! agent's `StepAction` channel, so `POST /run/{id}/step` can route a
//! user decision back to the agent loop.
//!
//! Lifecycle:
//! - `register` is called by `run_handler` when a step-mode request comes
//!   in (i.e. `approval_policy != Auto` AND a `request_id` is supplied).
//!   It returns a [`RunGuard`] whose `Drop` removes the entry — so an
//!   agent that completes naturally, panics, or is cancelled cleans itself
//!   up without leaking sender handles.
//! - `step` is called by the `POST /run/{id}/step` handler. It looks up
//!   the sender, forwards the action, and returns whether the run was
//!   known.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hone_proto::StepAction;
use tokio::sync::mpsc;

/// Default capacity of the per-run StepAction channel. Approval responses
/// are user-driven and arrive at human pace, so a small buffer is plenty.
const STEP_CHANNEL_CAPACITY: usize = 8;

/// Outcome of pushing a `StepAction` for a `request_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    /// The action was accepted and the receiver is still alive.
    Accepted,
    /// No run with this id is currently registered.
    NotFound,
    /// The run was registered but the agent's receiver has already been
    /// dropped (typically because the turn finished). The caller should
    /// treat this as "too late" — usually a 410 Gone or 404.
    Closed,
}

/// Error returned by [`RunRegistry::register`] when the same `request_id`
/// is already in use. The HTTP layer maps this to `409 Conflict`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("run_id already registered: {0}")]
pub struct DuplicateRunId(pub String);

/// Process-wide registry of in-flight step-mode runs. Cheap to clone
/// because it wraps an `Arc<Mutex<HashMap<...>>>`.
#[derive(Clone, Default)]
pub struct RunRegistry {
    inner: Arc<Mutex<HashMap<String, mpsc::Sender<StepAction>>>>,
}

/// RAII handle returned by [`RunRegistry::register`]. Dropping it removes
/// the entry from the map so the agent's receiver is not held alive once
/// the turn ends.
pub struct RunGuard {
    request_id: String,
    inner: Arc<Mutex<HashMap<String, mpsc::Sender<StepAction>>>>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.request_id);
    }
}

impl RunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a `(StepAction sender, receiver)` pair, register the
    /// sender under `request_id`, and return the receiver alongside a
    /// drop-guard the caller MUST keep alive for the lifetime of the agent
    /// task.
    ///
    /// Returns [`DuplicateRunId`] if `request_id` is already in flight —
    /// this prevents a guard's `Drop` from removing an entry that belongs
    /// to a different (concurrent) run.
    pub fn register(
        &self,
        request_id: String,
    ) -> Result<(mpsc::Receiver<StepAction>, RunGuard), DuplicateRunId> {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if map.contains_key(&request_id) {
            return Err(DuplicateRunId(request_id));
        }
        let (tx, rx) = mpsc::channel(STEP_CHANNEL_CAPACITY);
        map.insert(request_id.clone(), tx);
        drop(map);
        let guard = RunGuard {
            request_id,
            inner: Arc::clone(&self.inner),
        };
        Ok((rx, guard))
    }

    /// Forward `action` to the run identified by `request_id`. Returns
    /// `NotFound` if the id is unknown, `Closed` if the receiver has gone
    /// away (rare race with agent shutdown), `Accepted` otherwise.
    pub fn step(&self, request_id: &str, action: StepAction) -> StepResult {
        let sender = match self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(request_id)
            .cloned()
        {
            Some(s) => s,
            None => return StepResult::NotFound,
        };
        match sender.try_send(action) {
            Ok(()) => StepResult::Accepted,
            Err(mpsc::error::TrySendError::Closed(_)) => StepResult::Closed,
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Buffer is small (8) and only filled by spamming clicks.
                StepResult::Closed
            }
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_then_step_delivers_action_and_keeps_entry_alive() {
        let reg = RunRegistry::new();
        let (mut rx, _guard) = reg.register("r1".into()).expect("register");
        assert_eq!(reg.step("r1", StepAction::Proceed), StepResult::Accepted);
        let received = rx.recv().await.expect("got action");
        assert_eq!(received, StepAction::Proceed);
        assert_eq!(reg.len(), 1, "guard still alive — entry remains");
    }

    #[tokio::test]
    async fn drop_guard_removes_entry() {
        let reg = RunRegistry::new();
        {
            let (_rx, _guard) = reg.register("r1".into()).expect("register");
            assert_eq!(reg.len(), 1);
        }
        assert!(reg.is_empty(), "guard dropped — entry must be gone");
    }

    #[tokio::test]
    async fn step_for_unknown_id_returns_not_found() {
        let reg = RunRegistry::new();
        assert_eq!(reg.step("nope", StepAction::Skip), StepResult::NotFound);
    }

    #[tokio::test]
    async fn step_after_receiver_dropped_returns_closed() {
        let reg = RunRegistry::new();
        let (rx, _guard) = reg.register("r1".into()).expect("register");
        drop(rx);
        assert_eq!(reg.step("r1", StepAction::Skip), StepResult::Closed);
    }

    #[tokio::test]
    async fn second_register_with_same_id_returns_duplicate_error() {
        let reg = RunRegistry::new();
        let (_rx, _guard) = reg.register("r1".into()).expect("first register");
        match reg.register("r1".into()) {
            Ok(_) => panic!("second register must fail"),
            Err(DuplicateRunId(id)) => assert_eq!(id, "r1"),
        }
    }

    #[tokio::test]
    async fn after_first_guard_drops_same_id_can_be_re_registered() {
        let reg = RunRegistry::new();
        {
            let _ = reg.register("r1".into()).expect("first");
        }
        // RunGuard is not Debug; assert on the post-state via len() instead.
        assert!(reg.is_empty());
        assert!(reg.register("r1".into()).is_ok(), "after drop, id is free");
    }
}
