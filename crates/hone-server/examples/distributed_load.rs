//! P2-4 distributed orchestrator load test.
//!
//! In-process simulation: spins up a `DistributedTaskStore`, N concurrent
//! mock workers (each polls + completes instantly), and an enqueuer that
//! submits M plans of K subtasks each. Measures end-to-end throughput +
//! per-plan completion latency (p50/p95/p99/max). Emits a JSON report.
//!
//! Usage:
//!
//! ```text
//! cargo run --release --example distributed_load -p hone-server -- \
//!     --num-plans 100 --tasks-per-plan 10 --num-workers 8 \
//!     --report /tmp/dist-load.json
//! ```
//!
//! All defaults: --num-plans 50, --tasks-per-plan 5, --num-workers 4,
//! --report (omit -> stdout only).
//!
//! Out of scope: external-coordinator mode (would need a real running
//! honed). The in-process mode measures the queue + worker dispatch
//! logic — the HTTP layer's added latency is tracked separately by the
//! existing distributed_routes tests.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hone_core::task_plan::SubTask;
use hone_proto::TokenUsage;
use hone_server::distributed::{
    CompletionPayload, DistributedTaskStore, WorkerId,
};

#[derive(Debug)]
struct Args {
    num_plans: usize,
    tasks_per_plan: usize,
    num_workers: usize,
    report_path: Option<std::path::PathBuf>,
}

impl Args {
    fn parse() -> Self {
        let mut a = std::env::args().skip(1);
        let mut num_plans = 50usize;
        let mut tasks_per_plan = 5usize;
        let mut num_workers = 4usize;
        let mut report_path: Option<std::path::PathBuf> = None;
        while let Some(arg) = a.next() {
            match arg.as_str() {
                "--num-plans" => num_plans = a.next().and_then(|v| v.parse().ok()).unwrap_or(50),
                "--tasks-per-plan" => {
                    tasks_per_plan = a.next().and_then(|v| v.parse().ok()).unwrap_or(5)
                }
                "--num-workers" => {
                    num_workers = a.next().and_then(|v| v.parse().ok()).unwrap_or(4)
                }
                "--report" => report_path = a.next().map(std::path::PathBuf::from),
                "-h" | "--help" => {
                    eprintln!(
                        "Usage: distributed_load [--num-plans N] [--tasks-per-plan K] \
                         [--num-workers W] [--report PATH]"
                    );
                    std::process::exit(0);
                }
                other => {
                    eprintln!("unknown flag: {other}");
                    std::process::exit(2);
                }
            }
        }
        Self { num_plans, tasks_per_plan, num_workers, report_path }
    }
}

fn make_subtask(plan_idx: usize, task_idx: usize) -> SubTask {
    SubTask {
        id: format!("p{plan_idx}-t{task_idx}"),
        description: format!("synthetic task {plan_idx}.{task_idx}"),
        agent: "researcher".into(),
        context: String::new(),
        depends_on: vec![],
        acceptance_tests: vec![],
        budget_usd: None,
        scope: vec![],
        requires_capabilities: vec![],
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    eprintln!(
        "load test: {} plans × {} tasks = {} subtasks, {} workers",
        args.num_plans,
        args.tasks_per_plan,
        args.num_plans * args.tasks_per_plan,
        args.num_workers,
    );

    let store = Arc::new(DistributedTaskStore::open_in_memory().await?);

    // Spawn N worker loops. Each worker polls claim, completes immediately
    // with a synthetic outcome. No real Agent — this measures queue
    // throughput, not provider latency.
    let mut worker_handles = Vec::new();
    for w in 0..args.num_workers {
        let store = Arc::clone(&store);
        let worker_id = WorkerId::new(format!("w{w}"));
        let handle = tokio::spawn(async move {
            let lease = Duration::from_secs(60);
            loop {
                match store.claim_one(&worker_id, &[], lease).await {
                    Ok(Some(claimed)) => {
                        let payload = CompletionPayload {
                            task_id: claimed.task_id.clone(),
                            worker_id: worker_id.clone(),
                            status_str: "completed".into(),
                            response: format!("ok-{}", claimed.task_id),
                            error_msg: None,
                            usage: TokenUsage::default(),
                            cost_usd: 0.0,
                            provenance_head: None,
                        };
                        let _ = store.complete(payload).await;
                    }
                    Ok(None) => {
                        // No work; brief yield then poll again
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        });
        worker_handles.push(handle);
    }

    // Wall-clock start
    let t0 = Instant::now();

    // Per-plan completion latencies.  Each plan is enqueued, then we
    // poll plan_status until queued+leased == 0; record the elapsed time.
    let mut latencies: Vec<Duration> = Vec::with_capacity(args.num_plans);

    for p in 0..args.num_plans {
        let plan_id = format!("plan-{p}");
        let subtasks: Vec<SubTask> = (0..args.tasks_per_plan)
            .map(|t| make_subtask(p, t))
            .collect();
        let plan_start = Instant::now();
        store.enqueue(&plan_id, subtasks).await?;
        // Poll until terminal
        loop {
            let (queued, leased, _, _, _) = store.plan_state_counts(&plan_id).await?;
            if queued + leased == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        latencies.push(plan_start.elapsed());
    }

    let total_elapsed = t0.elapsed();

    // Stop workers
    for h in worker_handles {
        h.abort();
    }

    // Compute stats
    latencies.sort_unstable();
    let total_subtasks = (args.num_plans * args.tasks_per_plan) as u64;
    let throughput_subtasks_per_sec =
        total_subtasks as f64 / total_elapsed.as_secs_f64();
    let throughput_plans_per_sec =
        args.num_plans as f64 / total_elapsed.as_secs_f64();

    let p = |q: f64| -> Duration {
        if latencies.is_empty() {
            Duration::ZERO
        } else {
            let idx = ((latencies.len() as f64 - 1.0) * q).round() as usize;
            latencies[idx.min(latencies.len() - 1)]
        }
    };
    let p50 = p(0.50);
    let p95 = p(0.95);
    let p99 = p(0.99);
    let max = latencies.last().copied().unwrap_or(Duration::ZERO);

    let mut report: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    report.insert(
        "config".into(),
        serde_json::json!({
            "num_plans": args.num_plans,
            "tasks_per_plan": args.tasks_per_plan,
            "num_workers": args.num_workers,
        }),
    );
    report.insert(
        "results".into(),
        serde_json::json!({
            "total_elapsed_ms": total_elapsed.as_millis(),
            "total_subtasks": total_subtasks,
            "throughput_subtasks_per_sec": throughput_subtasks_per_sec,
            "throughput_plans_per_sec": throughput_plans_per_sec,
            "plan_latency_ms": {
                "p50": p50.as_millis(),
                "p95": p95.as_millis(),
                "p99": p99.as_millis(),
                "max": max.as_millis(),
            },
        }),
    );

    let report_json = serde_json::to_string_pretty(&report)?;
    println!("{report_json}");

    if let Some(path) = &args.report_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &report_json)?;
        eprintln!("report written to {}", path.display());
    }

    Ok(())
}
