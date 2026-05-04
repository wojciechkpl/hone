//! Re-export of the handoff buffer + request types from `hone-proto`.
//!
//! Lives here so the orchestrator can drain handoff requests via
//! `hone_core::handoff::HandoffBuffer` without needing to know about
//! `hone-tools` (the producer side of the buffer). Future orchestrator
//! integration drains these requests at end-of-turn and routes the payload
//! to the named target agent.

pub use hone_proto::handoff::{HandoffBuffer, HandoffRequest};
