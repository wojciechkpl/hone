#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR8 (and the emergency call of CR1/CR2) of `docs/plans/context-receipts-spec.md`:
//! each receipt's `compaction` lists what the context manager changed since the
//! previous receipt: `phase`, `graduated`, `dedup`, `drop_oldest`. A step's
//! `strategy.name` is one of those; `replaced` holds the `Message` refs it
//! replaced (with the hashes they had); `derived` is the summary or pointer
//! message (`None` when content was only removed).
//!
//! This binary sets `HONE_CONTEXT_WINDOW=4096` once, before any agent is built,
//! so the token budget is small (2048 tokens) and trimming can be forced. API:
//! see `crates/hone-proto/tests/receipt.rs`.

mod receipts_support;

use std::sync::Once;

use hone_proto::{AgentEvent, ContextReceipt, Message, SourceRef, message_sha256};
use receipts_support::*;
use serde_json::json;

static INIT: Once = Once::new();

/// Sets the env var for the whole binary before the first agent is created;
/// every test calls it first, and `Once` blocks concurrent callers until it is set.
fn init() {
    INIT.call_once(|| {
        // SAFETY: runs once, before any test of this binary builds an agent or
        // reads the environment (every test calls `init` first).
        unsafe {
            std::env::set_var("HONE_CONTEXT_WINDOW", "4096");
            // Phase compaction is opt-in (PR A round 1, K11); the phase test needs it.
            // No other test of this binary dispatches a write, so none depends on it.
            std::env::set_var("HONE_PHASE_COMPACTION", "1");
        };
    });
}

fn receipts(events: &[AgentEvent]) -> Vec<ContextReceipt> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .collect()
}

/// Indexes of the receipts whose compaction lists a step named `name`.
fn with_step(rs: &[ContextReceipt], name: &str) -> Vec<usize> {
    rs.iter()
        .enumerate()
        .filter(|(_, r)| r.compaction.iter().any(|s| s.strategy.name == name))
        .map(|(i, _)| i)
        .collect()
}

fn step<'a>(r: &'a ContextReceipt, name: &str) -> &'a hone_proto::CompactionStep {
    r.compaction
        .iter()
        .find(|s| s.strategy.name == name)
        .unwrap()
}

// ---- dedup ------------------------------------------------------------------------------

#[tokio::test]
async fn test_cr8_duplicate_tool_result_should_be_recorded_once_as_a_dedup_step() {
    init();
    // Two reads with identical (> 500 char) output: the second is replaced by a pointer.
    let same = |id: &str| call(id, "read_file", json!({ "tag": "same", "reps": 40 }));
    let mut r = rig(vec![vec![same("c1")], vec![same("c2")]], Opts::default());

    let rs = receipts(&r.turn("read twice").await);

    assert_eq!(rs.len(), 3);
    assert_eq!(
        with_step(&rs, "dedup"),
        [2],
        "the call after the duplicate was added, once"
    );
    let s = step(&rs[2], "dedup");
    assert_eq!(s.replaced.len(), 1);
    let original = Message::tool_result(
        "c2".to_owned(),
        serde_json::to_string(&json!(long_output("same", 40))).unwrap(),
    );
    match &s.replaced[0] {
        SourceRef::Message { sha256, .. } => {
            assert_eq!(
                *sha256,
                message_sha256(&original),
                "the hash the message had"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(s.derived, Some(SourceRef::Message { .. })),
        "the pointer message"
    );
}

// ---- graduated compression ---------------------------------------------------------------

#[tokio::test]
async fn test_cr8_old_tool_results_should_be_recorded_as_graduated_steps() {
    init();
    // Eight read rounds: results older than five assistant turns are compressed.
    let rounds: Vec<_> = (1..=8).map(|i| vec![read(&format!("g{i}"))]).collect();
    let mut r = rig(rounds, Opts::default());

    let rs = receipts(&r.turn("keep reading").await);

    let hits = with_step(&rs, "graduated");
    assert!(
        !hits.is_empty(),
        "no graduated step in {} receipts",
        rs.len()
    );
    for &i in &hits {
        let s = step(&rs[i], "graduated");
        assert!(!s.replaced.is_empty());
        assert!(
            s.replaced
                .iter()
                .all(|x| matches!(x, SourceRef::Message { .. }))
        );
        assert!(
            matches!(s.derived, Some(SourceRef::Message { .. })),
            "the summary message"
        );
    }
    // Each step is reported once: a receipt lists only changes since the previous one.
    let first_hit = hits[0];
    assert!(first_hit >= 1, "the first call cannot have compaction yet");
    assert!(rs[0].compaction.is_empty());
}

// ---- drop oldest + emergency --------------------------------------------------------------

#[tokio::test]
async fn test_cr8_trimming_should_be_recorded_as_a_drop_oldest_step() {
    init();
    // Small rounds first, then one huge result that overshoots the 2048-token budget.
    let mut rounds: Vec<_> = (1..=5).map(|i| vec![read(&format!("s{i}"))]).collect();
    rounds.push(vec![call(
        "big",
        "read_file",
        json!({ "tag": "big", "reps": 700 }),
    )]);
    let mut r = rig(rounds, Opts::default());

    let rs = receipts(&r.turn("read a lot").await);

    let hits = with_step(&rs, "drop_oldest");
    assert_eq!(
        hits.len(),
        1,
        "one trimming pass, reported once: {hits:?} of {}",
        rs.len()
    );
    let s = step(&rs[hits[0]], "drop_oldest");
    assert!(!s.replaced.is_empty(), "the dropped messages");
    assert!(
        s.replaced
            .iter()
            .all(|x| matches!(x, SourceRef::Message { .. }))
    );
}

#[tokio::test]
async fn test_cr1_context_emergency_call_should_still_get_a_receipt_with_no_tools() {
    init();
    let mut rounds: Vec<_> = (1..=5).map(|i| vec![read(&format!("s{i}"))]).collect();
    rounds.push(vec![call(
        "big",
        "read_file",
        json!({ "tag": "big", "reps": 700 }),
    )]);
    let mut r = rig(rounds, Opts::default());

    let rs = receipts(&r.turn("read a lot").await);

    assert_eq!(
        rs.len(),
        r.provider.request_count(),
        "one receipt per request"
    );
    let last_req = r.provider.request(r.provider.request_count() - 1);
    assert!(
        last_req.tools.is_empty(),
        "the emergency call offers no tools"
    );
    let last = rs.last().unwrap();
    assert!(last.tools_offered.is_empty());
    assert_eq!(
        last.request_sha256,
        hone_proto::request_sha256(&last_req.messages, &[])
    );
}

// ---- phase compaction (PR A round 1: opt-in, window counted in tool results) ----------------

#[tokio::test]
async fn test_cr8_first_write_after_reads_should_be_recorded_as_a_phase_step() {
    init(); // HONE_PHASE_COMPACTION=1
    // Five reads, then an edit elsewhere. The model `mock:test` protects the newest 4
    // tool results (r2..r5), so r1 is compacted when the write is dispatched.
    let mut r = rig(
        vec![
            vec![read("r1")],
            vec![read("r2")],
            vec![read("r3")],
            vec![read("r4")],
            vec![read("r5")],
            vec![call(
                "e1",
                "edit_file",
                json!({ "path": "src/z.rs", "tag": "e1" }),
            )],
        ],
        Opts::default(),
    );

    let rs = receipts(&r.turn("fix it").await);

    assert_eq!(rs.len(), 7);
    assert_eq!(
        with_step(&rs, "phase"),
        [6],
        "reported on the call after the edit, once"
    );
    let s = step(&rs[6], "phase");
    let r1_before = Message::tool_result(
        "r1".to_owned(),
        serde_json::to_string(&json!(long_output("r1", 20))).unwrap(),
    );
    assert_eq!(
        s.replaced,
        [SourceRef::Message {
            index: s_index(&s.replaced),
            sha256: message_sha256(&r1_before),
        }],
        "only r1, with the hash it had before compaction"
    );
    // The derived (summary) message is listed again by the incremental History block.
    let Some(SourceRef::Message { sha256: derived, .. }) = &s.derived else {
        panic!("a phase step has a derived message: {s:?}");
    };
    let history = rs[6]
        .blocks
        .iter()
        .find(|b| b.slot == hone_proto::Slot::History)
        .expect("a History block");
    assert!(
        history.sources.iter().any(|x| matches!(
            &x.source,
            SourceRef::Message { sha256, .. } if sha256 == derived
        )),
        "the rewritten message is relisted with its new hash (CR15)"
    );
}

/// The index of the single replaced ref (the stored-history position; not pinned here).
fn s_index(replaced: &[SourceRef]) -> usize {
    match replaced.first() {
        Some(SourceRef::Message { index, .. }) => *index,
        other => panic!("{other:?}"),
    }
}

// ---- CR17: the compaction log is consistent (review round 1) -------------------------------

fn big_round(id: &str) -> Vec<hone_proto::ToolCallRequest> {
    vec![call(id, "read_file", json!({ "tag": id, "reps": 700 }))]
}

#[tokio::test]
async fn test_cr17_a_second_drop_pass_should_log_the_marker_it_replaces() {
    init();
    // Turn 1 overshoots the budget (first drop pass, a `[session history` marker is
    // stored). Turn 2 does it again: the old marker is replaced by a new one.
    let mut rounds: Vec<_> = (1..=3).map(|i| vec![read(&format!("a{i}"))]).collect();
    rounds.push(big_round("big1"));
    rounds.push(vec![]); // turn 1: the emergency call answers with text
    rounds.extend((1..=3).map(|i| vec![read(&format!("b{i}"))]));
    rounds.push(big_round("big2"));
    let mut r = rig(rounds, Opts::default());

    let mut rs = receipts(&r.turn("read a lot").await);
    rs.extend(receipts(&r.turn("and again").await));

    // Every drop pass after the first replaces the marker the previous pass left:
    // the marker is in the request sent just before it, and in the step's `replaced`.
    let passes = with_step(&rs, "drop_oldest");
    assert!(passes.len() >= 2, "need two drop passes: {passes:?} of {}", rs.len());
    let mut checked = 0;
    for &p in passes.iter().filter(|&&p| p > 0) {
        let markers: Vec<String> = r
            .provider
            .request(p - 1)
            .messages
            .iter()
            .filter(|m| m.content.as_text().starts_with("[session history"))
            .map(message_sha256)
            .collect();
        if markers.is_empty() {
            continue; // the first pass: no marker yet
        }
        checked += 1;
        let replaced: Vec<String> = step(&rs[p], "drop_oldest")
            .replaced
            .iter()
            .map(|x| match x {
                SourceRef::Message { sha256, .. } => sha256.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(
            markers.iter().all(|h| replaced.contains(h)),
            "call {}: the old marker {markers:?} must be in `replaced` {replaced:?}",
            p + 1
        );
    }
    assert!(checked >= 1, "harness: a later pass must find an earlier marker");
}

#[tokio::test]
async fn test_cr17_refs_within_one_drop_pass_should_have_distinct_pre_pass_indices() {
    init();
    let mut rounds: Vec<_> = (1..=5).map(|i| vec![read(&format!("s{i}"))]).collect();
    rounds.push(big_round("big"));
    let mut r = rig(rounds, Opts::default());

    let rs = receipts(&r.turn("read a lot").await);

    let hits = with_step(&rs, "drop_oldest");
    assert_eq!(hits.len(), 1);
    let s = step(&rs[hits[0]], "drop_oldest");
    let indices: Vec<usize> = s
        .replaced
        .iter()
        .map(|x| match x {
            SourceRef::Message { index, .. } => *index,
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(indices.len() >= 2, "several messages dropped: {indices:?}");
    let distinct: std::collections::HashSet<_> = indices.iter().collect();
    assert_eq!(distinct.len(), indices.len(), "no two refs share an index: {indices:?}");
}
