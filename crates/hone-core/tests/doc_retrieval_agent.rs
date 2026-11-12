#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R6 and R7 (and the registry side of R1/R5) of `docs/plans/doc-retrieval-spec.md`,
//! through a real `Agent` with the scripted provider, a temp working directory of notes,
//! and the strategy selection (PR D) with `retrieval` in `[context] sources`. The document
//! index is the ephemeral in-memory one (the test runs set `HONE_DISABLE_PERSISTENT_STORES=1`).
//!
//! API: `hone_index::{DocIndex, ..}` (see `crates/hone-index/tests/doc_index.rs`),
//! `hone_core::strategy` (see `tests/strategies.rs`). New here:
//! - registry `list()` gains `("context.sources", "retrieval", Builtin)`,
//!   `("retrieval.retriever", "fts5-docs", Builtin)` and `("retrieval.chunker", "markdown" | "fixed", Builtin)`;
//! - the `[retrieval]` config keys `top_k`, `max_tokens`, `chunker` (see `retrieval_config.rs`).
//!
//! Block (R6), one system-role message placed after the skills block:
//!
//! ```text
//! # Relevant documents
//!
//! <path>:<start>-<end>[ — <heading path>]      one header line per hit, best first
//! <chunk text>
//! ```
//!
//! The header line starts with `path:start-end`; when the chunk has a heading its path
//! (`Deploy > Steps`) appears later on the same line. A hit that does not fit
//! `max_tokens` is dropped whole (never cut); the dropped hits are the receipt's `dropped`
//! `Chunk` sources. No hit, no message. The block is redacted like other injected text.
//!
//! Receipt (R7): a `Slot::Retrieval` block after `Skills`, strategy `fts5-docs`, params
//! `{"chunker":"markdown","max_tokens":1200,"top_k":3}` (sha256 of that compact key-sorted JSON),
//! one `Chunk { index: "docs", chunk_id, sha256 }` source per included hit with its score, tokens
//! of the sent block message.

mod receipts_support;

use std::sync::Arc;

use hone_config::policy::{CompiledPolicy, PolicyContext, builtin_rules};
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::strategy::{Origin, ResolvedStrategies, StrategyDeps, StrategyRegistry};
use hone_core::{Agent, PolicyConfig, Skill, SkillRegistry, estimate_text_tokens};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use hone_proto::{AgentEvent, ContextReceipt, Message, Slot, SourceRef, sha256_hex};
use receipts_support::*;
use tokio::sync::mpsc;

fn no_env(_: &str) -> Option<String> {
    None
}

fn strategies(toml: &str) -> Arc<ResolvedStrategies> {
    let user = ConfigLayer::from_toml("/home/u/.config/hone/config.toml", toml).unwrap();
    let flags = Flags { no_inject: true, ..Default::default() };
    let sel = StrategySelection::from_sources(Some(&user), None, &no_env, &flags).unwrap();
    Arc::new(
        StrategyRegistry::with_builtins()
            .build(&sel, &StrategyDeps::default())
            .unwrap_or_else(|e| panic!("build failed: {e}")),
    )
}

const ON: &str = "[context]\nsources = [\"memory-index\", \"recall\", \"skills\", \"retrieval\"]\n";

fn retrieval_on(extra: &str) -> Arc<ResolvedStrategies> {
    strategies(&format!("{ON}\n{extra}"))
}

fn write(dir: &std::path::Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn text(m: &Message) -> String {
    m.content.as_text()
}

fn docs_message(msgs: &[Message]) -> Option<String> {
    msgs.iter().map(text).find(|t| t.starts_with("# Relevant documents"))
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

struct Run {
    agent: Agent,
    provider: Arc<Script>,
    dir: tempfile::TempDir,
}

fn agent(s: Arc<ResolvedStrategies>, dir: tempfile::TempDir, extra: Opts) -> Run {
    let provider = Script::new(vec![]);
    let agent = Agent::new(config(
        provider.clone(),
        Opts { working_dir: Some(dir.path().to_path_buf()), ..extra },
    ))
    .with_strategies(s);
    Run { agent, provider, dir }
}

async fn turn(r: &mut Run, prompt: &str) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(1024);
    r.agent.run_turn(prompt, tx).await.unwrap();
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn notes() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "deploy.md",
        "# Deploy\nIntro about shipping.\n\n## Steps\nRun the staging deploy script and wait for the health check.\n",
    );
    write(dir.path(), "lunch.md", "# Lunch\nTuesday is pasta day.\n");
    dir
}

fn mem(id: &str, content: &str) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at: 1,
        decay_at: None,
        kind: MemoryKind::Feedback,
        description: None,
        embedding: None,
    }
}

// ---- registry --------------------------------------------------------------------------------

#[test]
fn test_r1_the_registry_should_list_the_retrieval_builtins() {
    let listed = StrategyRegistry::with_builtins().list();
    let has = |slot: &str, name: &str| listed.contains(&(slot.to_owned(), name.to_owned(), Origin::Builtin));
    assert!(has("context.sources", "retrieval"));
    assert!(has("retrieval.retriever", "fts5-docs"));
    assert!(has("retrieval.chunker", "markdown"));
    assert!(has("retrieval.chunker", "fixed"));
}

#[test]
fn test_r6_retrieval_should_be_off_unless_the_sources_list_it() {
    assert!(!strategies("").has_source("retrieval"));
    assert!(retrieval_on("").has_source("retrieval"));
}

// ---- R6: the block ----------------------------------------------------------------------------

#[tokio::test]
async fn test_r6_the_block_should_follow_the_skills_and_name_each_hit_with_its_lines_and_heading() {
    let store = Arc::new(InMemoryStore::new());
    store.record(mem("m1", "staging deploy script rules and habits")).await.unwrap();
    let mut skills = SkillRegistry::new();
    skills.register(Skill {
        name: "deploy-helper".to_owned(),
        description: "d".to_owned(),
        triggers: vec!["deploy".to_owned()],
        tools: vec![],
        model: None,
        always: false,
        body: "Be careful.".to_owned(),
    });
    let mut r = agent(
        retrieval_on(""),
        notes(),
        Opts { memory: Some(store), skills: Some(Arc::new(skills)), ..Default::default() },
    );

    turn(&mut r, "how do I run the staging deploy script").await;

    let msgs = r.provider.request(0).messages;
    assert!(text(&msgs[1]).starts_with("# Relevant Memories"), "{}", text(&msgs[1]));
    assert!(text(&msgs[2]).contains("## Skills"));
    let block = text(&msgs[3]);
    assert!(block.starts_with("# Relevant documents"), "after the skills block: {block}");
    assert_eq!(msgs[3].role, hone_proto::Role::System);
    let header = block.lines().find(|l| l.starts_with("deploy.md:4-5")).unwrap_or_else(|| panic!("no header in:\n{block}"));
    assert!(header.contains("Deploy > Steps"), "{header}");
    assert!(block.contains("Run the staging deploy script and wait for the health check."));
    assert!(!block.contains("pasta"), "an unrelated note is not included");
}

#[tokio::test]
async fn test_r6_the_block_should_hold_at_most_top_k_hits_default_three() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..6 {
        write(dir.path(), &format!("n{i}.md"), &format!("# Note {i}\nquokka facts number {i}\n"));
    }
    let mut r = agent(retrieval_on(""), dir, Opts::default());

    turn(&mut r, "tell me about the quokka").await;

    let block = docs_message(&r.provider.request(0).messages).expect("a block");
    assert_eq!(block.lines().filter(|l| l.contains(".md:") && l.contains("-")).count(), 3, "{block}");

    let dir = tempfile::tempdir().unwrap();
    for i in 0..6 {
        write(dir.path(), &format!("n{i}.md"), &format!("# Note {i}\nquokka facts number {i}\n"));
    }
    let mut r = agent(retrieval_on("[retrieval]\ntop_k = 2\n"), dir, Opts::default());
    turn(&mut r, "tell me about the quokka").await;
    let block = docs_message(&r.provider.request(0).messages).unwrap();
    assert_eq!(block.lines().filter(|l| l.contains(".md:") && l.contains("-")).count(), 2, "{block}");
}

#[tokio::test]
async fn test_r6_no_hits_should_mean_no_block_and_no_receipt_block() {
    let mut r = agent(retrieval_on(""), notes(), Opts::default());

    let events = turn(&mut r, "quantum entanglement of zeppelins").await;

    assert!(docs_message(&r.provider.request(0).messages).is_none());
    assert!(receipts(&events)[0].blocks.iter().all(|b| b.slot != Slot::Retrieval));
}

#[tokio::test]
async fn test_r6_the_default_sources_should_not_retrieve_documents() {
    let mut r = agent(strategies(""), notes(), Opts::default());

    let events = turn(&mut r, "how do I run the staging deploy script").await;

    assert!(docs_message(&r.provider.request(0).messages).is_none(), "off by default");
    assert!(receipts(&events)[0].blocks.iter().all(|b| b.slot != Slot::Retrieval));
}

#[tokio::test]
async fn test_r6_each_turn_should_retrieve_for_its_own_prompt_from_the_current_files() {
    let mut r = agent(retrieval_on(""), notes(), Opts::default());
    turn(&mut r, "how do I run the staging deploy script").await;
    write(r.dir.path(), "payroll.md", "# Payroll\nThe payroll cutoff is the 25th of each month.\n");

    turn(&mut r, "when is the payroll cutoff").await;

    let first = docs_message(&r.provider.request(0).messages).unwrap();
    let second = docs_message(&r.provider.request(1).messages).expect("turn 2 has a block");
    assert!(first.contains("health check") && !first.contains("payroll"));
    assert!(second.contains("payroll cutoff is the 25th"), "the file added after turn 1 is found: {second}");
    assert!(!second.contains("health check"), "turn 2's hits follow turn 2's prompt");
}

#[tokio::test]
async fn test_r6_the_block_should_be_redacted() {
    let key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "ops.md", &format!("# Ops\nThe backup bucket credentials are {key} for the nightly job.\n"));
    let provider = Script::new(vec![]);
    let mut cfg = config(provider.clone(), Opts { working_dir: Some(dir.path().to_path_buf()), ..Default::default() });
    let context = PolicyContext::for_platform(dir.path().to_path_buf(), None);
    let compiled = CompiledPolicy::compile(&context, &builtin_rules(), &[], &[]).unwrap();
    cfg.policy = Some(PolicyConfig { compiled: Arc::new(compiled), context, pre_approved: Vec::new(), builtins: true });
    let mut agent = Agent::new(cfg).with_strategies(retrieval_on(""));

    let (tx, _rx) = mpsc::channel::<AgentEvent>(256);
    agent.run_turn("where are the backup bucket credentials", tx).await.unwrap();

    let block = docs_message(&provider.request(0).messages).expect("a block");
    assert!(block.contains("backup bucket credentials"));
    assert!(!block.contains(&key), "secret-shaped text must be redacted: {block}");
}

// ---- R6: the token budget ------------------------------------------------------------------------------

fn budget_notes() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let pad = "plain filler words repeated to give the note some length and weight. ".repeat(3);
    write(dir.path(), "a.md", &format!("# Alpha\norchid velvet orchid velvet {pad} A-END\n"));
    write(dir.path(), "b.md", &format!("# Beta\norchid {pad} B-END\n"));
    write(dir.path(), "c.md", &format!("# Gamma\norchid {pad} C-END\n"));
    dir
}

#[tokio::test]
async fn test_r6_hits_that_do_not_fit_max_tokens_should_be_dropped_whole() {
    let mut r = agent(retrieval_on("[retrieval]\nmax_tokens = 100\n"), budget_notes(), Opts::default());

    let events = turn(&mut r, "orchid velvet").await;

    let block = docs_message(&r.provider.request(0).messages).expect("the best hit fits");
    assert!(block.contains("A-END"), "the best hit is whole: {block}");
    assert!(!block.contains("B-END") && !block.contains("C-END"), "the others do not fit and are dropped, not cut: {block}");
    assert!(estimate_text_tokens(&block) <= 100 + 20, "within the budget (plus the heading line)");
    let receipt = &receipts(&events)[0];
    let mut dropped: Vec<String> = receipt
        .dropped
        .iter()
        .map(|s| match s {
            SourceRef::Chunk { index, chunk_id, .. } => {
                assert_eq!(index, "docs");
                chunk_id.clone()
            }
            other => panic!("{other:?}"),
        })
        .collect();
    dropped.sort();
    assert_eq!(dropped, ["b.md#1", "c.md#1"], "the budget-cut hits are in `dropped`");
}

#[tokio::test]
async fn test_r6_a_generous_budget_should_keep_every_hit_and_drop_nothing() {
    let mut r = agent(retrieval_on(""), budget_notes(), Opts::default());

    let events = turn(&mut r, "orchid velvet").await;

    let block = docs_message(&r.provider.request(0).messages).unwrap();
    assert!(block.contains("A-END") && block.contains("B-END") && block.contains("C-END"));
    assert!(receipts(&events)[0].dropped.iter().all(|s| !matches!(s, SourceRef::Chunk { .. })));
}

// ---- R7: receipts ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_r7_the_receipt_should_have_a_retrieval_block_after_skills_with_chunk_sources() {
    let mut skills = SkillRegistry::new();
    skills.register(Skill {
        name: "deploy-helper".to_owned(),
        description: "d".to_owned(),
        triggers: vec!["deploy".to_owned()],
        tools: vec![],
        model: None,
        always: false,
        body: "Be careful.".to_owned(),
    });
    let mut r = agent(retrieval_on(""), notes(), Opts { skills: Some(Arc::new(skills)), ..Default::default() });

    let events = turn(&mut r, "how do I run the staging deploy script").await;

    let receipt = &receipts(&events)[0];
    let slots: Vec<Slot> = receipt.blocks.iter().map(|b| b.slot).collect();
    let (skills_at, retrieval_at) = (
        slots.iter().position(|s| *s == Slot::Skills).unwrap(),
        slots.iter().position(|s| *s == Slot::Retrieval).expect("a Retrieval block"),
    );
    assert!(skills_at < retrieval_at, "{slots:?}");
    let block = &receipt.blocks[retrieval_at];
    assert_eq!(block.strategy.name, "fts5-docs");
    assert_eq!(block.strategy.slot, Slot::Retrieval);
    assert_eq!(
        block.strategy.params_sha256,
        sha256_hex(br#"{"chunker":"markdown","max_tokens":1200,"top_k":3}"#)
    );
    assert_eq!(
        block.tokens,
        estimate_text_tokens(&docs_message(&r.provider.request(0).messages).unwrap()),
        "tokens of the block that was sent"
    );
    assert!(!block.sources.is_empty());
    for s in &block.sources {
        match &s.source {
            SourceRef::Chunk { index, chunk_id, sha256 } => {
                assert_eq!(index, "docs");
                assert!(chunk_id.starts_with("deploy.md#"), "{chunk_id}");
                assert_eq!(sha256.len(), 64);
            }
            other => panic!("{other:?}"),
        }
        assert!(s.score.is_some_and(|x| x > 0.0));
    }
}

#[tokio::test]
async fn test_r7_params_should_follow_the_configuration() {
    let mut r = agent(
        retrieval_on("[retrieval]\ntop_k = 2\nmax_tokens = 500\nchunker = \"fixed\"\n"),
        notes(),
        Opts::default(),
    );

    let events = turn(&mut r, "how do I run the staging deploy script").await;

    let block = receipts(&events)[0].blocks.iter().find(|b| b.slot == Slot::Retrieval).cloned().expect("a Retrieval block");
    assert_eq!(
        block.strategy.params_sha256,
        sha256_hex(br#"{"chunker":"fixed","max_tokens":500,"top_k":2}"#)
    );
}

#[tokio::test]
async fn test_r7_the_block_should_be_on_every_call_of_the_turn_and_unchanged() {
    let provider = Script::new(vec![vec![read("r1")]]);
    let dir = notes();
    let mut agent = Agent::new(config(provider.clone(), Opts { working_dir: Some(dir.path().to_path_buf()), ..Default::default() }))
        .with_strategies(retrieval_on(""));

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(1024);
    agent.run_turn("how do I run the staging deploy script", tx).await.unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    assert_eq!(provider.request_count(), 2);
    assert_eq!(docs_message(&provider.request(0).messages), docs_message(&provider.request(1).messages));
    let rs = receipts(&events);
    assert!(rs.iter().all(|r| r.blocks.iter().any(|b| b.slot == Slot::Retrieval)));
    assert_eq!(
        rs[0].blocks.iter().find(|b| b.slot == Slot::Retrieval),
        rs[1].blocks.iter().find(|b| b.slot == Slot::Retrieval)
    );
}
