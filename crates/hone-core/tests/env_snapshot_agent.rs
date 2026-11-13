#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E1, E5, E6, E7 and the registry side of E8 of `docs/plans/env-snapshot-spec.md`,
//! through a real `Agent` with the scripted provider and the strategy selection
//! (PR D) with `env-snapshot` in `[context] sources`. The probes run on this
//! machine's PATH here: the assertions only rely on the OS line and the listing,
//! which are computed in Rust. API: `tests/env_snapshot.rs`, `tests/strategies.rs`,
//! `crates/hone-proto/tests/env_slot.rs` (`Slot::Environment`).
//!
//! Needs PR B (receipts) and PR D (strategies) underneath.

mod receipts_support;

use std::sync::Arc;

use hone_config::policy::{CompiledPolicy, PolicyContext, builtin_rules};
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::strategy::{Origin, ResolvedStrategies, StrategyDeps, StrategyRegistry};
use hone_core::{Agent, PolicyConfig, Skill, SkillRegistry, estimate_text_tokens};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use hone_proto::{AgentEvent, ContextReceipt, Slot, SourceRef, sha256_hex};
use receipts_support::*;
use tokio::sync::mpsc;

const GOLDEN: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/context_request_golden.json");

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

/// `env-snapshot` on, injection off (so the request prefix is only the system
/// blocks), a short timeout.
fn env_on() -> Arc<ResolvedStrategies> {
    strategies(
        "[context]\nsources = [\"memory-index\", \"recall\", \"skills\", \"env-snapshot\"]\n\n[context.env-snapshot]\ntimeout_secs = 3\n",
        Flags::default(),
    )
}

fn text_of(m: &hone_proto::Message) -> String {
    m.content.as_text()
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

async fn turn(agent: &mut Agent, input: &str) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(2048);
    agent.run_turn(input, tx).await.unwrap();
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

// ---- registry / E8 ----------------------------------------------------------------------------

#[test]
fn test_e8_env_snapshot_should_be_a_listed_builtin_source() {
    assert!(StrategyRegistry::with_builtins().list().contains(&(
        "context.sources".to_owned(),
        "env-snapshot".to_owned(),
        Origin::Builtin
    )));
}

#[test]
fn test_e1_env_snapshot_should_be_off_unless_the_sources_list_it() {
    let default = strategies("", Flags::default());
    assert!(!default.has_source("env-snapshot"));
    assert!(env_on().has_source("env-snapshot"));
}

// ---- E1: off = byte identical -------------------------------------------------------------------

#[tokio::test]
async fn test_e1_with_it_off_the_requests_should_match_the_golden_fixture() {
    // The session of `context_request_golden.rs`, built with the default selection (no injection).
    let store = Arc::new(InMemoryStore::new());
    for (id, text, kind) in [
        ("m1", "prefer small commits when refactoring the parser module", MemoryKind::Feedback),
        ("m2", "the parser module lives in src/parser and has golden tests", MemoryKind::Project),
        ("m3", "unrelated note about lunch", MemoryKind::User),
    ] {
        let mut m = mem(id, text);
        m.kind = kind;
        store.record(m).await.unwrap();
    }
    let mut skills = SkillRegistry::new();
    skills.register(Skill {
        name: "refactor-helper".to_owned(),
        description: "help with refactors".to_owned(),
        triggers: vec!["refactor".to_owned()],
        tools: vec![],
        model: None,
        always: false,
        body: "Keep diffs small.".to_owned(),
    });
    let provider = Script::new(vec![vec![read("r1")], vec![read("r2")]]);
    let mut agent = Agent::new(config(
        provider.clone(),
        Opts { memory: Some(store), skills: Some(Arc::new(skills)), ..Default::default() },
    ))
    .with_strategies(strategies("", Flags { no_inject: true, ..Default::default() }));
    for prompt in ["refactor the parser module", "thanks, now add docs"] {
        turn(&mut agent, prompt).await;
    }

    let recorded: Vec<serde_json::Value> = (0..provider.request_count())
        .map(|i| {
            let req = provider.request(i);
            serde_json::json!({
                "model": req.model.to_string(),
                "messages": req.messages,
                "tool_names": req.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let actual = serde_json::to_string_pretty(&recorded).unwrap() + "\n";
    assert_eq!(actual, std::fs::read_to_string(GOLDEN).unwrap());
}

// ---- E5: once, placed stably ----------------------------------------------------------------------

#[tokio::test]
async fn test_e5_the_block_should_follow_the_system_prompt_and_stay_unchanged_across_calls() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("first_file.txt"), "x").unwrap();
    let store = Arc::new(InMemoryStore::new());
    store.record(mem("m1", "rebasing the zorblax branch before merging")).await.unwrap();
    let mut skills = SkillRegistry::new();
    skills.register(Skill {
        name: "zorblax-helper".to_owned(),
        description: "d".to_owned(),
        triggers: vec!["zorblax".to_owned()],
        tools: vec![],
        model: None,
        always: false,
        body: "Be careful.".to_owned(),
    });
    let provider = Script::new(vec![vec![read("r1")]]);
    let mut agent = Agent::new(config(
        provider.clone(),
        Opts {
            memory: Some(store),
            skills: Some(Arc::new(skills)),
            working_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
    ))
    .with_strategies(env_on());

    turn(&mut agent, "rebasing the zorblax branch").await; // two calls
    std::fs::write(dir.path().join("created_after_turn_one.txt"), "x").unwrap();
    turn(&mut agent, "and again, zorblax").await; // one more call

    assert_eq!(provider.request_count(), 3);
    let first = text_of(&provider.request(0).messages[1]);
    assert!(first.starts_with("# Environment"), "right after the system prompt: {first}");
    assert!(first.contains("first_file.txt"));
    for i in 0..3 {
        let msgs = provider.request(i).messages;
        assert_eq!(msgs[0].role, hone_proto::Role::System);
        assert_eq!(msgs[1].role, hone_proto::Role::System, "one system-role message");
        assert_eq!(text_of(&msgs[1]), first, "request {i}: unchanged, gathered once");
        assert!(text_of(&msgs[2]).starts_with("# Relevant Memories"), "request {i}: memories after the environment");
        assert!(text_of(&msgs[3]).contains("## Skills"), "request {i}: skills after the memories");
    }
    assert!(!first.contains("created_after_turn_one"), "not refreshed mid-session");
}

#[tokio::test]
async fn test_e5_the_environment_block_should_not_depend_on_the_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let mut texts = Vec::new();
    for prompt in ["first", "something else entirely"] {
        let provider = Script::new(vec![]);
        let mut agent = Agent::new(config(
            provider.clone(),
            Opts { working_dir: Some(dir.path().to_path_buf()), ..Default::default() },
        ))
        .with_strategies(env_on());
        turn(&mut agent, prompt).await;
        let m = &provider.request(0).messages[1];
        let t = text_of(m);
        assert!(t.starts_with("# Environment"));
        texts.push(t.lines().take(3).map(str::to_owned).collect::<Vec<_>>());
    }
    assert_eq!(texts[0], texts[1], "OS and working directory lines are the same");
}

// ---- E6: redaction ----------------------------------------------------------------------------------

#[tokio::test]
async fn test_e6_a_secret_shaped_path_component_should_be_redacted() {
    let key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(format!("{key}.txt")), "x").unwrap();
    std::fs::write(dir.path().join("plain_file.txt"), "x").unwrap();

    let provider = Script::new(vec![]);
    let mut cfg = config(provider.clone(), Opts { working_dir: Some(dir.path().to_path_buf()), ..Default::default() });
    let context = PolicyContext::for_platform(dir.path().to_path_buf(), None);
    let compiled = CompiledPolicy::compile(&context, &builtin_rules(), &[], &[]).unwrap();
    cfg.policy = Some(PolicyConfig { compiled: Arc::new(compiled), context, pre_approved: Vec::new(), builtins: true });
    let mut agent = Agent::new(cfg).with_strategies(env_on());

    turn(&mut agent, "hello").await;

    let block = text_of(&provider.request(0).messages[1]);
    assert!(block.starts_with("# Environment"), "{block}");
    assert!(!block.contains(&key), "the secret-shaped file name leaked: {block}");
    assert!(block.contains("plain_file.txt"), "the rest of the listing stays");
}

// ---- E7: receipts --------------------------------------------------------------------------------------

#[tokio::test]
async fn test_e7_each_request_should_have_an_environment_block_after_the_system_prompt() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x").unwrap();
    let store = Arc::new(InMemoryStore::new());
    store.record(mem("m1", "rebasing the zorblax branch before merging")).await.unwrap();
    let provider = Script::new(vec![vec![read("r1")]]);
    let mut agent = Agent::new(config(
        provider.clone(),
        Opts { memory: Some(store), working_dir: Some(dir.path().to_path_buf()), ..Default::default() },
    ))
    .with_strategies(env_on());

    let events = turn(&mut agent, "rebasing the zorblax branch").await;

    let rs = receipts(&events);
    assert_eq!(rs.len(), 2);
    for (i, r) in rs.iter().enumerate() {
        let slots: Vec<Slot> = r.blocks.iter().map(|b| b.slot).collect();
        assert_eq!(slots[..3], [Slot::SystemPrompt, Slot::Environment, Slot::Recall], "call {}", i + 1);
        let env = &r.blocks[1];
        assert_eq!(env.strategy.name, "env-snapshot");
        assert_eq!(env.strategy.slot, Slot::Environment);
        assert_eq!(env.strategy.params_sha256, sha256_hex(br#"{"timeout_secs":3}"#));
        assert_eq!(
            env.tokens,
            estimate_text_tokens(&text_of(&provider.request(i).messages[1])),
            "tokens of the system message that was sent"
        );
        assert!(!env.sources.is_empty());
        for s in &env.sources {
            match &s.source {
                SourceRef::Command { argv, sha256 } => {
                    assert!(!argv.is_empty());
                    assert_eq!(sha256.len(), 64);
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(
            env.sources.iter().any(|s| matches!(&s.source, SourceRef::Command { argv, .. } if argv == &["<builtin:listing>"])),
            "the directory listing is a source"
        );
    }
    assert_eq!(rs[0].blocks[1], rs[1].blocks[1], "the same block on every call");
}

#[tokio::test]
async fn test_e7_the_default_timeout_should_hash_as_fifteen_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Script::new(vec![]);
    let mut agent = Agent::new(config(
        provider,
        Opts { working_dir: Some(dir.path().to_path_buf()), ..Default::default() },
    ))
    .with_strategies(strategies(
        "[context]\nsources = [\"env-snapshot\"]\n",
        Flags::default(),
    ));

    let events = turn(&mut agent, "hello").await;

    let r = &receipts(&events)[0];
    let env = r.blocks.iter().find(|b| b.slot == Slot::Environment).expect("an Environment block");
    assert_eq!(env.strategy.params_sha256, sha256_hex(br#"{"timeout_secs":15}"#));
}

#[tokio::test]
async fn test_e7_without_the_source_there_should_be_no_environment_block() {
    let provider = Script::new(vec![]);
    let mut agent = Agent::new(config(provider, Opts::default()))
        .with_strategies(strategies("", Flags { no_inject: true, ..Default::default() }));

    let events = turn(&mut agent, "hello").await;

    assert!(receipts(&events)[0].blocks.iter().all(|b| b.slot != Slot::Environment));
}
