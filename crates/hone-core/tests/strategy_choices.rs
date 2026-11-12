#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! S7 of `docs/plans/strategy-registry-spec.md`: each config choice takes effect,
//! through a real `Agent` built with `Agent::with_strategies`. Every test runs a
//! default control next to the changed choice, so a pass cannot come from the
//! scenario itself. API: see `tests/strategies.rs` (`hone_core::strategy`) and
//! `crates/hone-config/tests/strategy_selection.rs`.
//!
//! Tests marked (A) need PR A's opt-in phase compaction underneath; those marked
//! (B) need PR B's receipts.

mod receipts_support;

use std::sync::Arc;

use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::strategy::{ResolvedStrategies, StrategyDeps, StrategyRegistry};
use hone_core::{Agent, estimate_text_tokens};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryQuery, MemoryStore};
use hone_proto::{AgentEvent, Slot};
use receipts_support::*;
use serde_json::json;
use tokio::sync::mpsc;

fn no_env(_: &str) -> Option<String> {
    None
}

fn strategies(toml: &str, flags: Flags) -> Arc<ResolvedStrategies> {
    let user = ConfigLayer::from_toml("/home/u/.config/hone/config.toml", toml).unwrap();
    let sel = StrategySelection::from_sources(Some(&user), None, &no_env, &flags).unwrap();
    Arc::new(
        StrategyRegistry::with_builtins()
            .build(&sel, &StrategyDeps::default())
            .unwrap_or_else(|e| panic!("build failed: {e}")),
    )
}

fn mem(id: &str, content: &str, kind: MemoryKind) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

async fn store_with(memories: Vec<Memory>) -> Arc<InMemoryStore> {
    let store = Arc::new(InMemoryStore::new());
    for m in memories {
        store.record(m).await.unwrap();
    }
    store
}

async fn stored_count(store: &InMemoryStore) -> usize {
    store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .unwrap()
        .len()
}

/// A working directory with enough files for a skeleton larger than 60 tokens.
fn project_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    for i in 0..40 {
        std::fs::write(
            dir.path().join(format!("src/module_number_{i}_handler.rs")),
            format!(
                "pub fn handle_request_{i}(input: &str) -> bool {{ true }}\npub struct Thing{i};\n"
            ),
        )
        .unwrap();
    }
    dir
}

struct Run {
    agent: Agent,
    provider: Arc<Script>,
    events: Vec<AgentEvent>,
}

async fn run(
    s: Option<Arc<ResolvedStrategies>>,
    opts: Opts,
    rounds: Vec<Vec<hone_proto::ToolCallRequest>>,
    prompt: &str,
) -> Run {
    let provider = Script::new(rounds);
    let agent = Agent::new(config(provider.clone(), opts));
    let mut agent = match s {
        Some(s) => agent.with_strategies(s),
        None => agent,
    };
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(2048);
    agent.run_turn(prompt, tx).await.unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    Run {
        agent,
        provider,
        events,
    }
}

fn request_text(r: &Run, i: usize) -> String {
    r.provider
        .request(i)
        .messages
        .iter()
        .map(|m| m.content.as_text())
        .collect::<Vec<_>>()
        .join("\n---msg---\n")
}

fn recalled(events: &[AgentEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MemoryRecalled { entries } => Some(entries.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn injected(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ContextInjected { .. }))
        .count()
}

// ---- sources -------------------------------------------------------------------------------

#[tokio::test]
async fn test_s7_skeleton_absent_from_sources_should_inject_nothing() {
    let dir = project_dir();
    let opts = || Opts {
        working_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    };

    let with = run(
        Some(strategies("", Flags::default())),
        opts(),
        vec![],
        "fix the handler",
    )
    .await;
    assert!(
        request_text(&with, 0).contains("Repository Structure"),
        "control: injection on"
    );

    let without = run(
        Some(strategies(
            "[context]\nsources = [\"memory-index\", \"recall\", \"skills\"]\n",
            Flags::default(),
        )),
        opts(),
        vec![],
        "fix the handler",
    )
    .await;
    assert!(!request_text(&without, 0).contains("Repository Structure"));
    assert_eq!(injected(&without.events), 0, "no ContextInjected either");
}

#[tokio::test]
async fn test_s7_recall_absent_or_recaller_none_should_leave_out_relevant_memories() {
    let seed = || async {
        store_with(vec![mem(
            "m1",
            "rebasing the zorblax branch before merging",
            MemoryKind::Feedback,
        )])
        .await
    };
    let mk = |s: Option<Arc<ResolvedStrategies>>, store: Arc<InMemoryStore>| {
        run(
            s,
            Opts {
                memory: Some(store),
                ..Default::default()
            },
            vec![],
            "rebasing the zorblax branch",
        )
    };
    let no_inject = Flags {
        no_inject: true,
        ..Default::default()
    };

    let control = mk(Some(strategies("", no_inject.clone())), seed().await).await;
    assert!(request_text(&control, 0).contains("# Relevant Memories"));
    assert!(!recalled(&control.events).is_empty());

    for toml in [
        "[context]\nsources = [\"memory-index\", \"skills\", \"skeleton\"]\n",
        "[memory]\nrecaller = \"none\"\n",
    ] {
        let r = mk(Some(strategies(toml, no_inject.clone())), seed().await).await;
        assert!(
            !request_text(&r, 0).contains("# Relevant Memories"),
            "{toml}"
        );
        assert!(recalled(&r.events).is_empty(), "no MemoryRecalled: {toml}");
    }
}

#[tokio::test]
async fn test_s7_skills_absent_from_sources_should_leave_out_the_skills_block() {
    let mut reg = hone_core::SkillRegistry::new();
    reg.register(hone_core::Skill {
        name: "refactor-helper".to_owned(),
        description: "d".to_owned(),
        triggers: vec!["refactor".to_owned()],
        tools: vec![],
        model: None,
        always: false,
        body: "Keep diffs small.".to_owned(),
    });
    let reg = Arc::new(reg);
    let no_inject = Flags {
        no_inject: true,
        ..Default::default()
    };
    let opts = || Opts {
        skills: Some(reg.clone()),
        ..Default::default()
    };

    let control = run(
        Some(strategies("", no_inject.clone())),
        opts(),
        vec![],
        "refactor this",
    )
    .await;
    assert!(request_text(&control, 0).contains("## Skills"));

    let r = run(
        Some(strategies(
            "[context]\nsources = [\"memory-index\", \"recall\", \"skeleton\"]\n",
            no_inject,
        )),
        opts(),
        vec![],
        "refactor this",
    )
    .await;
    assert!(!request_text(&r, 0).contains("## Skills"));
}

#[test]
fn test_s7_memory_index_absent_from_sources_should_not_be_a_resolved_source() {
    let on = strategies("", Flags::default());
    assert!(on.has_source("memory-index"));
    let off = strategies(
        "[context]\nsources = [\"recall\", \"skills\", \"skeleton\"]\n",
        Flags::default(),
    );
    assert!(
        !off.has_source("memory-index"),
        "the system prompt then carries no memory index"
    );
    assert!(off.has_source("recall") && off.has_source("skills") && off.has_source("skeleton"));
}

// ---- recaller parameters ------------------------------------------------------------------------

#[tokio::test]
async fn test_s7_top_k_should_cap_the_recalled_memories() {
    let memories: Vec<Memory> = (0..6)
        .map(|i| {
            mem(
                &format!("m{i}"),
                &format!("NOTE-{i} rebasing the zorblax branch before merging"),
                MemoryKind::Feedback,
            )
        })
        .collect();
    let no_inject = Flags {
        no_inject: true,
        ..Default::default()
    };
    let mk = |toml: &'static str| {
        let memories = memories.clone();
        let flags = no_inject.clone();
        async move {
            let store = store_with(memories).await;
            run(
                Some(strategies(toml, flags)),
                Opts {
                    memory: Some(store),
                    ..Default::default()
                },
                vec![],
                "rebasing the zorblax branch",
            )
            .await
        }
    };

    assert_eq!(
        recalled(&mk("").await.events).len(),
        5,
        "control: the default top_k is 5"
    );
    assert_eq!(recalled(&mk("[memory]\ntop_k = 2\n").await.events).len(), 2);
}

#[tokio::test]
async fn test_s7_empty_exclude_kinds_should_let_outcome_logs_back_in() {
    let seed = || async {
        store_with(vec![mem(
            "o1",
            "OUTCOME-LOG rebasing the zorblax branch, result: done",
            MemoryKind::Outcome,
        )])
        .await
    };
    let no_inject = Flags {
        no_inject: true,
        ..Default::default()
    };
    let mk = |toml: &'static str, flags: Flags, store: Arc<InMemoryStore>| {
        run(
            Some(strategies(toml, flags)),
            Opts {
                memory: Some(store),
                ..Default::default()
            },
            vec![],
            "rebasing the zorblax branch",
        )
    };

    let default = mk("", no_inject.clone(), seed().await).await;
    assert!(
        recalled(&default.events).is_empty(),
        "control: outcome logs are excluded"
    );

    let open = mk("[memory]\nexclude_kinds = []\n", no_inject, seed().await).await;
    assert!(
        recalled(&open.events)
            .iter()
            .any(|(_, s)| s.contains("OUTCOME-LOG")),
        "{:?}",
        recalled(&open.events)
    );
}

// ---- writer ---------------------------------------------------------------------------------------

#[tokio::test]
async fn test_s7_writer_off_should_record_no_memories_from_turns() {
    let no_inject = Flags {
        no_inject: true,
        ..Default::default()
    };
    let prompt = "don't use tabs for indentation, use four spaces instead";

    let store = store_with(vec![]).await;
    run(
        Some(strategies("", no_inject.clone())),
        Opts {
            memory: Some(store.clone()),
            ..Default::default()
        },
        vec![],
        prompt,
    )
    .await;
    assert!(
        stored_count(&store).await >= 1,
        "control: the triggers writer records a correction"
    );

    let store = store_with(vec![]).await;
    run(
        Some(strategies("[memory]\nwriter = \"off\"\n", no_inject)),
        Opts {
            memory: Some(store.clone()),
            ..Default::default()
        },
        vec![],
        prompt,
    )
    .await;
    assert_eq!(stored_count(&store).await, 0);
}

// ---- compactor --------------------------------------------------------------------------------------

fn tool_texts(r: &Run) -> Vec<String> {
    r.agent
        .messages()
        .iter()
        .filter(|m| m.role == hone_proto::Role::Tool)
        .map(|m| m.content.as_text())
        .collect()
}

#[tokio::test]
async fn test_s7_trim_only_should_neither_dedup_nor_compress() {
    let no_inject = Flags {
        no_inject: true,
        ..Default::default()
    };
    let same = |id: &str| call(id, "read_file", json!({ "tag": "same", "reps": 40 }));

    let control = run(
        Some(strategies("", no_inject.clone())),
        Opts::default(),
        vec![vec![same("c1")], vec![same("c2")]],
        "read twice",
    )
    .await;
    assert!(
        tool_texts(&control)
            .iter()
            .any(|t| t.starts_with("[duplicate content")),
        "control: dedup"
    );

    let r = run(
        Some(strategies(
            "[context]\ncompactor = \"trim-only\"\n",
            no_inject.clone(),
        )),
        Opts::default(),
        vec![vec![same("c1")], vec![same("c2")]],
        "read twice",
    )
    .await;
    assert!(
        !tool_texts(&r)
            .iter()
            .any(|t| t.starts_with("[duplicate content"))
    );

    let rounds = || {
        (1..=8)
            .map(|i| vec![read(&format!("g{i}"))])
            .collect::<Vec<_>>()
    };
    let control = run(
        Some(strategies("", no_inject.clone())),
        Opts::default(),
        rounds(),
        "keep reading",
    )
    .await;
    assert!(
        tool_texts(&control)
            .iter()
            .any(|t| t.starts_with("[tool result")),
        "control: graduated compression"
    );
    let r = run(
        Some(strategies(
            "[context]\ncompactor = \"trim-only\"\n",
            no_inject,
        )),
        Opts::default(),
        rounds(),
        "keep reading",
    )
    .await;
    assert!(
        !tool_texts(&r).iter().any(|t| t.starts_with("[tool result")),
        "no compression"
    );
    assert_eq!(r.agent.metrics_snapshot().tool_results_compressed, 0);
}

#[tokio::test]
async fn test_s7_trim_only_should_still_trim_to_fit_the_budget() {
    // budget_ratio 0.016 gives a 2048-token budget; a huge result overshoots it.
    let toml = "[context]\ncompactor = \"trim-only\"\nbudget_ratio = 0.016\n";
    let mut rounds: Vec<_> = (1..=5).map(|i| vec![read(&format!("s{i}"))]).collect();
    rounds.push(vec![call(
        "big",
        "read_file",
        json!({ "tag": "big", "reps": 700 }),
    )]);
    let r = run(
        Some(strategies(
            toml,
            Flags {
                no_inject: true,
                ..Default::default()
            },
        )),
        Opts::default(),
        rounds,
        "read a lot",
    )
    .await;
    let snap = r.agent.metrics_snapshot();
    assert!(
        snap.messages_dropped > 0,
        "trimming still happens: {snap:?}"
    );
}

#[tokio::test]
async fn test_s7_phase_true_should_compact_when_a_write_follows_reads_a() {
    let no_inject = Flags {
        no_inject: true,
        ..Default::default()
    };
    // Six reads then an edit: r1 is older than the recent window (4 for `mock:test`),
    // so phase compaction summarises it.
    let rounds = || {
        let mut r: Vec<_> = (1..=6).map(|i| vec![read(&format!("r{i}"))]).collect();
        r.push(vec![call("e1", "edit_file", json!({ "tag": "e1" }))]);
        r
    };

    let off = run(
        Some(strategies("", no_inject.clone())),
        Opts::default(),
        rounds(),
        "fix it",
    )
    .await;
    // K11: the tracker always runs (it may record the transition); only the
    // compaction is gated by `phase`.
    assert!(
        tool_texts(&off)[0].contains("lorem"),
        "phase compaction is off by default: r1 stays verbatim"
    );
    assert!(
        !tool_texts(&off)
            .iter()
            .any(|t| t.starts_with("[tool result summarized")),
        "nothing is summarised"
    );

    let on = run(
        Some(strategies("[context.graduated]\nphase = true\n", no_inject)),
        Opts::default(),
        rounds(),
        "fix it",
    )
    .await;
    assert_eq!(on.agent.metrics_snapshot().phase_transitions, 1);
    assert!(
        tool_texts(&on)[0].starts_with("[tool result summarized"),
        "r1 summarised"
    );
}

// ---- budgets --------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_s7_max_tokens_should_bound_the_injected_block() {
    let dir = project_dir();
    let opts = || Opts {
        working_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    let injected_tokens = |r: &Run| {
        let text = r
            .provider
            .request(0)
            .messages
            .iter()
            .find(|m| m.role == hone_proto::Role::User)
            .unwrap()
            .content
            .as_text();
        let (block, _prompt) = text
            .split_once("\n\n---\n\n")
            .expect("an injected block before the prompt");
        estimate_text_tokens(block)
    };

    let default = run(
        Some(strategies("", Flags::default())),
        opts(),
        vec![],
        "fix the handler",
    )
    .await;
    assert!(
        injected_tokens(&default) > 70,
        "control: the default budget lets {} tokens through",
        injected_tokens(&default)
    );

    let small = run(
        Some(strategies(
            "[context.skeleton]\nmax_tokens = 60\n",
            Flags::default(),
        )),
        opts(),
        vec![],
        "fix the handler",
    )
    .await;
    assert!(
        injected_tokens(&small) <= 64,
        "{} tokens",
        injected_tokens(&small)
    );
}

#[tokio::test]
async fn test_s7_budget_ratio_should_override_the_per_model_ratio() {
    let rounds = || {
        let mut r: Vec<_> = (1..=5).map(|i| vec![read(&format!("s{i}"))]).collect();
        r.push(vec![call(
            "big",
            "read_file",
            json!({ "tag": "big", "reps": 700 }),
        )]);
        r
    };
    let no_inject = Flags {
        no_inject: true,
        ..Default::default()
    };
    let last_tools = |r: &Run| {
        r.provider
            .request(r.provider.request_count() - 1)
            .tools
            .len()
    };

    let default = run(
        Some(strategies("", no_inject.clone())),
        Opts::default(),
        rounds(),
        "read a lot",
    )
    .await;
    assert!(
        last_tools(&default) > 0,
        "control: the default budget leaves tools on"
    );

    let tiny = run(
        Some(strategies("[context]\nbudget_ratio = 0.016\n", no_inject)),
        Opts::default(),
        rounds(),
        "read a lot",
    )
    .await;
    assert_eq!(
        last_tools(&tiny),
        0,
        "a 2048-token budget forces the emergency call"
    );
}

// ---- receipts name the active strategies (B) -------------------------------------------------------------

#[tokio::test]
async fn test_s7_receipts_should_name_the_active_strategies_b() {
    let dir = project_dir();
    let store = store_with(vec![mem(
        "m1",
        "rebasing the zorblax branch",
        MemoryKind::Feedback,
    )])
    .await;
    let r = run(
        Some(strategies(
            "[context.skeleton]\nuse = \"hybrid\"\n",
            Flags::default(),
        )),
        Opts {
            memory: Some(store.clone()),
            working_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
        vec![],
        "rebasing the zorblax branch",
    )
    .await;
    let receipt = r
        .events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .expect("a receipt");
    let inj = receipt
        .blocks
        .iter()
        .find(|b| b.slot == Slot::Injection)
        .unwrap();
    assert_eq!(inj.strategy.name, "skeleton:hybrid");

    let r = run(
        Some(strategies(
            "[memory]\nrecaller = \"none\"\n",
            Flags {
                no_inject: true,
                ..Default::default()
            },
        )),
        Opts {
            memory: Some(store),
            ..Default::default()
        },
        vec![],
        "rebasing the zorblax branch",
    )
    .await;
    let receipt = r
        .events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        receipt.blocks.iter().all(|b| b.slot != Slot::Recall),
        "no recall block with recaller = none"
    );
}

// ---- S8: no strategies = today's fields ----------------------------------------------------------------------

#[tokio::test]
async fn test_s8_without_strategies_the_agent_should_follow_inject_context_and_skeleton_strategy() {
    let dir = project_dir();
    let on = run(
        None,
        Opts {
            inject: true,
            strategy: hone_core::SkeletonStrategy::Flat,
            working_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
        vec![],
        "fix the handler",
    )
    .await;
    assert!(request_text(&on, 0).contains("Repository Structure"));

    let off = run(
        None,
        Opts {
            inject: false,
            working_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
        vec![],
        "fix the handler",
    )
    .await;
    assert!(!request_text(&off, 0).contains("Repository Structure"));
}
