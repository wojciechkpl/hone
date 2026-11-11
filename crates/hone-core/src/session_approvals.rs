//! The "always" answers of one session, shareable between agents.
//!
//! An `ApproveAlways` answer means "for this session". Each [`Agent`] keeps its
//! own copy in its approval policy and policy session approvals; it also
//! records the answer here, so agents that were handed the same
//! `Arc<SessionApprovals>` (the session agent and the workers of its
//! orchestrated runs) take it in before their next tool call.
//!
//! [`Agent`]: crate::agent::Agent

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};

#[derive(Debug, Default)]
struct Inner {
    /// Tools approved at the approval-policy pause.
    tools: HashSet<String>,
    /// Policy-gate session-approval entries (see `session_approval_entries`).
    entries: HashSet<String>,
}

/// Tool names and policy entries the user approved with "always".
///
/// Locks are held only for the duration of a set operation, never across an
/// await point.
#[derive(Debug, Default)]
pub struct SessionApprovals {
    inner: Mutex<Inner>,
}

impl SessionApprovals {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Record "always" for a tool at the approval-policy pause.
    pub fn record_tool(&self, name: &str) {
        self.lock().tools.insert(name.to_owned());
    }

    /// Record the entries of an "always" answer at a policy `ask` prompt.
    pub fn record_entries<I: IntoIterator<Item = String>>(&self, entries: I) {
        self.lock().entries.extend(entries);
    }

    /// Whether "always" was recorded for `name`.
    pub fn covers_tool(&self, name: &str) -> bool {
        self.lock().tools.contains(name)
    }

    /// The recorded tool names.
    pub(crate) fn tools(&self) -> Vec<String> {
        self.lock().tools.iter().cloned().collect()
    }

    /// Add the recorded policy entries to `approved`.
    pub(crate) fn extend_entries_into(&self, approved: &mut HashSet<String>) {
        approved.extend(self.lock().entries.iter().cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reports_tools_and_entries() {
        let s = SessionApprovals::default();
        assert!(!s.covers_tool("shell"));
        s.record_tool("shell");
        assert!(s.covers_tool("shell"));
        s.record_entries(["e1".to_owned()]);
        let mut set = HashSet::new();
        s.extend_entries_into(&mut set);
        assert!(set.contains("e1"));
    }
}
