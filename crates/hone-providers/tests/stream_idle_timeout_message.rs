#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R12 (`docs/plans/stream-retry-fallback-spec.md`), adapter level: an idle timeout is reported
//! as `<provider> response timed out: <cause>` (so the agent and the fallback wrappers can tell
//! it from a transport failure); a cut socket keeps `<provider> response interrupted: <cause>`.
//! Idle limit 1 s, generous outer timeouts.

mod common;

use std::time::Duration;

use common::{ADAPTERS, Adapter, End, Step, errors, head, piece, run, text};

const IDLE: Duration = Duration::from_secs(1);

#[tokio::test]
async fn test_r12_a_stall_after_the_headers_is_reported_as_timed_out() {
    for a in ADAPTERS {
        // Headers, then nothing for longer than the idle limit.
        let steps = vec![Step::Delay(Duration::from_secs(3))];

        let events = run(a, steps, End::Normal, Some(IDLE)).await;

        let errs = errors(&events);
        assert_eq!(errs.len(), 1, "{a:?}: {events:?}");
        assert!(
            errs[0].starts_with(&format!("{} response timed out:", a.name())),
            "{a:?}: {}",
            errs[0]
        );
    }
}

#[tokio::test]
async fn test_r12_a_stall_after_a_non_output_piece_is_reported_as_timed_out() {
    for a in [Adapter::Anthropic] {
        // Anthropic's message_start / content_block_start carry no output.
        let steps = vec![piece(head(a)), Step::Delay(Duration::from_secs(3))];

        let events = run(a, steps, End::Normal, Some(IDLE)).await;

        let errs = errors(&events);
        assert_eq!(errs.len(), 1, "{a:?}: {events:?}");
        assert!(
            errs[0].starts_with("anthropic response timed out:"),
            "{}",
            errs[0]
        );
    }
}

#[tokio::test]
async fn test_r12_a_stall_after_output_is_also_reported_as_timed_out() {
    for a in ADAPTERS {
        let steps = vec![
            piece(head(a)),
            piece(text(a, "first")),
            Step::Delay(Duration::from_secs(3)),
        ];

        let events = run(a, steps, End::Normal, Some(IDLE)).await;

        let errs = errors(&events);
        assert_eq!(errs.len(), 1, "{a:?}: {events:?}");
        assert!(
            errs[0].starts_with(&format!("{} response timed out:", a.name())),
            "{a:?}: {}",
            errs[0]
        );
    }
}

#[tokio::test]
async fn test_r12_control_a_cut_socket_is_still_reported_as_interrupted() {
    for a in ADAPTERS {
        let steps = vec![piece(head(a)), piece(text(a, "first"))];

        let events = run(a, steps, End::CutOff, None).await;

        let errs = errors(&events);
        assert_eq!(errs.len(), 1, "{a:?}: {events:?}");
        assert!(
            errs[0].starts_with(&format!("{} response interrupted:", a.name())),
            "{a:?}: {}",
            errs[0]
        );
    }
}
