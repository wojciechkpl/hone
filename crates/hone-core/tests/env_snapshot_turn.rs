#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E17-E20 of `docs/plans/env-snapshot-spec.md` (review round 1) through a real `Agent`:
//! gathered blocks count towards the context-emergency check, gathering never holds up a turn,
//! `Once` blocks stay ahead of `EachTurn` blocks, and a source's every block keeps its slot.
//!
//! API: `tests/strategies.rs` (`StrategyRegistry`, `ContextSource`, `ContextBlock`, `Cadence`,
//! `Agent::with_strategies`) and `tests/env_snapshot_agent.rs`. Behaviour pinned here:
//! - E17: the 95% emergency check adds the token estimate of every gathered block (a request
//!   whose prompt plus environment block exceeds 95% of the budget goes out with no tools);
//! - E18: sources are gathered concurrently with recall; a source that does not finish within
//!   its deadline (5 s for sources other than env-snapshot) is skipped and the turn goes on; a
//!   source that returns an error is skipped; when the events channel is closed the turn stops
//!   waiting for gathering and returns an error promptly;
//! - E19: a `Once` block is placed before `EachTurn` blocks whatever the order in `sources`
//!   (the environment block is the system message at index 1);
//! - E20: every block a source returns keeps its own slot in the receipt.
//!
//! This binary sets `HONE_CONTEXT_WINDOW=4096` once before any agent is built (a 2,048-token
//! budget); every test calls `init()` first.

mod receipts_support;

use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::Agent;
use hone_core::strategy::{
    Cadence, ContextBlock, ContextSource, Origin, ResolvedStrategies, StrategyDeps, StrategyError,
    StrategyRegistry, TurnView,
};
use hone_proto::{AgentEvent, Slot, StrategyId, sha256_hex};
use receipts_support::*;
use tokio::sync::mpsc;

static INIT: Once = Once::new();

fn init() {
    INIT.call_once(|| {
        // SAFETY: runs once, before any test of this binary builds an agent (every test calls
        // `init` first), so nothing reads the environment concurrently.
        unsafe { std::env::set_var("HONE_CONTEXT_WINDOW", "4096") };
    });
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn sid(name: &str, slot: Slot) -> StrategyId {
    StrategyId { slot, name: name.to_owned(), version: "1".to_owned(), params_sha256: sha256_hex(b"{}") }
}

// ---- custom sources -------------------------------------------------------------------------------

enum Behaviour {
    /// Returns these blocks at once.
    Blocks(Vec<(Slot, &'static str)>),
    /// Sleeps for a long time, then returns one block.
    Slow,
    /// Returns an error.
    Fails,
}

struct Custom {
    id: StrategyId,
    cadence: Cadence,
    behaviour: Behaviour,
}

#[async_trait]
impl ContextSource for Custom {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    fn cadence(&self) -> Cadence {
        self.cadence
    }
    async fn gather(&self, _view: &TurnView<'_>) -> Result<Vec<ContextBlock>, StrategyError> {
        match &self.behaviour {
            Behaviour::Blocks(blocks) => Ok(blocks
                .iter()
                .map(|(slot, text)| ContextBlock { slot: *slot, text: (*text).to_owned(), sources: vec![] })
                .collect()),
            Behaviour::Slow => {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(vec![ContextBlock { slot: Slot::Retrieval, text: "SLOW-BLOCK".to_owned(), sources: vec![] }])
            }
            Behaviour::Fails => Err(StrategyError::Failed {
                slot: "context.sources".to_owned(),
                name: "broken".to_owned(),
                reason: "this source always fails".to_owned(),
            }),
        }
    }
}

/// A registry with `custom` registered under `name`, and a selection whose sources are `sources`.
fn strategies(name: &'static str, cadence: Cadence, behaviour: Behaviour, sources: &[&str]) -> Arc<ResolvedStrategies> {
    let behaviour = Arc::new(std::sync::Mutex::new(Some(behaviour)));
    let mut reg = StrategyRegistry::with_builtins();
    reg.register_source(
        name,
        Origin::Builtin,
        Arc::new(move |_p, _d| {
            let b = behaviour.lock().unwrap().take().expect("built once");
            Ok(Arc::new(Custom { id: sid(name, Slot::Retrieval), cadence, behaviour: b }) as Arc<dyn ContextSource>)
        }),
    );
    let user = ConfigLayer::from_toml("/home/u/.config/hone/config.toml", "[context.env-snapshot]\ntimeout_secs = 3\n").unwrap();
    let flags = Flags { no_inject: true, ..Default::default() };
    let mut sel = StrategySelection::from_sources(Some(&user), None, &no_env, &flags).unwrap();
    sel.sources.value = sources.iter().map(|s| (*s).to_owned()).collect();
    Arc::new(reg.build(&sel, &StrategyDeps::default()).unwrap_or_else(|e| panic!("build failed: {e}")))
}

async fn run_turn(agent: &mut Agent, prompt: &str) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(1024);
    agent.run_turn(prompt, tx).await.unwrap();
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn texts(provider: &Script, request: usize) -> Vec<String> {
    provider.request(request).messages.iter().map(|m| m.content.as_text()).collect()
}

// ---- E17 ------------------------------------------------------------------------------------------------

/// A working directory whose listing alone is a large block (50 hash-like names of 80 characters).
fn big_listing_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..50 {
        let name = format!("{}{}", sha256_hex(i.to_string().as_bytes()), &sha256_hex(format!("x{i}").as_bytes())[..16]);
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    dir
}

#[tokio::test]
async fn test_e17_a_large_environment_block_should_trigger_the_emergency_path() {
    init();
    let dir = big_listing_dir();
    let mk = |sources: &[&str]| {
        let provider = Script::new(vec![]);
        let agent = Agent::new(config(provider.clone(), Opts { working_dir: Some(dir.path().to_path_buf()), ..Default::default() }))
            .with_strategies(strategies("unused", Cadence::EachTurn, Behaviour::Blocks(vec![]), sources));
        (provider, agent)
    };

    // Control: without the environment source nothing is near the budget and tools stay on.
    let (p, mut a) = mk(&["memory-index", "recall", "skills"]);
    run_turn(&mut a, "hello").await;
    assert!(!p.request(0).tools.is_empty(), "control: tools offered");

    // With the block the request would be past 95% of the 2,048-token budget: emergency.
    let (p, mut a) = mk(&["memory-index", "recall", "skills", "env-snapshot"]);
    let events = run_turn(&mut a, "hello").await;

    let req = p.request(0);
    assert!(texts(&p, 0)[1].starts_with("# Environment"), "the block was gathered and sent");
    assert!(req.tools.is_empty(), "gathered blocks count towards the 95% check, so tools are stripped");
    assert!(texts(&p, 0).iter().any(|t| t.contains("CONTEXT NEARLY FULL")), "the summary request was added");
    let receipt = events.iter().find_map(|e| match e {
        AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
        _ => None,
    });
    assert!(receipt.unwrap().tools_offered.is_empty());
}

// ---- E18 ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_e18_a_source_that_misses_its_deadline_should_be_skipped_and_the_turn_should_go_on() {
    init();
    let provider = Script::new(vec![]);
    let mut agent = Agent::new(config(provider.clone(), Opts::default()))
        .with_strategies(strategies("slow", Cadence::Once, Behaviour::Slow, &["memory-index", "recall", "skills", "slow"]));

    let start = Instant::now();
    let finished = tokio::time::timeout(Duration::from_secs(12), run_turn(&mut agent, "hello")).await;
    let took = start.elapsed();

    assert!(finished.is_ok(), "the turn is still waiting for the slow source after {took:?}");
    assert!(took < Duration::from_secs(8), "the 5 s deadline bounds the wait: {took:?}");
    assert!(!texts(&provider, 0).iter().any(|t| t.contains("SLOW-BLOCK")));
    assert_eq!(provider.request_count(), 1, "the turn ran");
}

#[tokio::test]
async fn test_e18_a_failing_source_should_be_skipped() {
    init();
    let provider = Script::new(vec![]);
    let mut agent = Agent::new(config(provider.clone(), Opts::default()))
        .with_strategies(strategies("broken", Cadence::EachTurn, Behaviour::Fails, &["memory-index", "recall", "skills", "broken"]));

    let events = run_turn(&mut agent, "hello").await;

    assert_eq!(provider.request_count(), 1);
    assert!(events.iter().all(|e| !matches!(e, AgentEvent::Error { .. })), "no error reaches the user");
}

#[tokio::test]
async fn test_e18_a_closed_events_channel_should_stop_the_wait_for_gathering() {
    init();
    let provider = Script::new(vec![]);
    let mut agent = Agent::new(config(provider.clone(), Opts::default()))
        .with_strategies(strategies("slow", Cadence::Once, Behaviour::Slow, &["memory-index", "recall", "skills", "slow"]));
    let (tx, rx) = mpsc::channel::<AgentEvent>(16);
    drop(rx); // the listener is gone before the turn starts

    let start = Instant::now();
    let result = agent.run_turn("hello", tx).await;
    let took = start.elapsed();

    assert!(result.is_err(), "a cancelled turn is an error");
    assert!(took < Duration::from_secs(2), "the turn did not wait for the slow source: {took:?}");
    assert_eq!(provider.request_count(), 0, "no model call was made");
}

// ---- E19 ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_e19_once_blocks_should_stay_ahead_of_each_turn_blocks_in_any_sources_order() {
    init();
    let dir = tempfile::tempdir().unwrap();
    for sources in [
        ["env-snapshot", "each"].as_slice(),
        ["each", "env-snapshot"].as_slice(),
    ] {
        let provider = Script::new(vec![]);
        let mut agent = Agent::new(config(provider.clone(), Opts { working_dir: Some(dir.path().to_path_buf()), ..Default::default() }))
            .with_strategies(strategies("each", Cadence::EachTurn, Behaviour::Blocks(vec![(Slot::Retrieval, "EACH-TURN-BLOCK")]), sources));

        run_turn(&mut agent, "first").await;
        run_turn(&mut agent, "second").await;

        for request in 0..2 {
            let t = texts(&provider, request);
            assert!(t[1].starts_with("# Environment"), "sources {sources:?}, request {request}: the Once block is first: {:?}", &t[..t.len().min(4)]);
            let each = t.iter().position(|x| x.contains("EACH-TURN-BLOCK")).unwrap_or_else(|| panic!("the EachTurn block is present: {t:?}"));
            assert!(each > 1, "{sources:?}: after the environment block");
        }
    }
}

// ---- E20 ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_e20_every_block_of_a_source_should_keep_its_own_slot_in_the_receipt() {
    init();
    let provider = Script::new(vec![]);
    let mut agent = Agent::new(config(provider.clone(), Opts::default())).with_strategies(strategies(
        "two",
        Cadence::EachTurn,
        Behaviour::Blocks(vec![(Slot::Retrieval, "FIRST-BLOCK"), (Slot::Injection, "SECOND-BLOCK")]),
        &["memory-index", "recall", "skills", "two"],
    ));

    let events = run_turn(&mut agent, "hello").await;

    let receipt = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .unwrap();
    let slots: Vec<Slot> = receipt.blocks.iter().map(|b| b.slot).collect();
    assert!(slots.contains(&Slot::Retrieval), "{slots:?}");
    assert!(slots.contains(&Slot::Injection), "{slots:?}");
}
