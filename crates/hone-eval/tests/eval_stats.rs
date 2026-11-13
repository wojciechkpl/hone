#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F9 of `docs/plans/eval-profiles-spec.md`: report statistics as pure functions.
//!
//! API required (new module `hone_eval::stats`):
//!
//! ```ignore
//! pub const BOOTSTRAP_RESAMPLES: usize = 10_000;
//! pub const BOOTSTRAP_SEED: u64 = ..;                        // fixed
//! /// Wilson score interval, 95% (z = 1.96), as (low, high); (0.0, 1.0) when n = 0.
//! pub fn wilson_interval(passes: u64, n: u64) -> (f64, f64);
//! /// Not dominated on (x: higher is better, y: lower is better). A point is dominated
//! /// when another is >= on x and <= on y and strictly better on one.
//! pub fn pareto_flags(points: &[(f64, f64)]) -> Vec<bool>;
//! pub struct RunStat { pub task: String, pub status: TaskStatus, pub cost_usd: f64,
//!                      pub context_tokens: u64, pub model_calls: u32 }
//! pub struct ProfileSummary { pub profile: String, pub runs: usize, pub passes: usize,
//!     pub timeouts: usize, pub errors: usize /* agent_error + infra_error */,
//!     pub pass_rate: f64, pub wilson_low: f64, pub wilson_high: f64,
//!     pub mean_cost_usd: f64, pub median_cost_usd: f64,
//!     pub mean_context_tokens: f64, pub mean_model_calls: f64 }
//! pub fn summarize(profile: &str, runs: &[RunStat]) -> ProfileSummary;
//! pub struct Paired { pub tasks: usize, pub mean_diff: f64, pub better: usize,
//!     pub worse: usize, pub tied: usize, pub ci_low: f64, pub ci_high: f64 }
//! /// Per task: pass rate (passes / runs over its repeats) of each side; only tasks both ran.
//! /// diff = other - base. CI: percentile 95% interval of the mean diff over `resamples`
//! /// bootstrap resamples of the tasks, from a PRNG seeded with `seed`.
//! pub fn paired_comparison(base: &[RunStat], other: &[RunStat], resamples: usize, seed: u64) -> Paired;
//! ```
//! `TaskStatus` is `hone_eval::TaskStatus` (gains `InfraError`).

use hone_eval::TaskStatus;
use hone_eval::stats::{
    BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, RunStat, paired_comparison, pareto_flags, summarize,
    wilson_interval,
};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn stat(task: &str, status: TaskStatus, cost: f64) -> RunStat {
    RunStat {
        task: task.to_owned(),
        status,
        cost_usd: cost,
        context_tokens: 100,
        model_calls: 2,
    }
}

fn pass(task: &str) -> RunStat {
    stat(task, TaskStatus::Pass, 0.01)
}
fn fail(task: &str) -> RunStat {
    stat(task, TaskStatus::Fail, 0.01)
}

#[test]
fn test_f9_wilson_interval_for_7_of_10_should_match_the_known_values() {
    let (lo, hi) = wilson_interval(7, 10);
    assert!(close(lo, 0.3968, 0.001), "{lo}");
    assert!(close(hi, 0.8922, 0.001), "{hi}");
}

#[test]
fn test_f9_wilson_interval_should_stay_inside_zero_and_one_at_the_extremes() {
    let (lo, hi) = wilson_interval(10, 10);
    assert!(close(lo, 0.7225, 0.001) && close(hi, 1.0, 1e-9), "{lo} {hi}");
    let (lo, hi) = wilson_interval(0, 10);
    assert!(close(lo, 0.0, 1e-9) && close(hi, 0.2775, 0.001), "{lo} {hi}");
    assert_eq!(wilson_interval(0, 0), (0.0, 1.0));
}

#[test]
fn test_f9_pareto_should_mark_points_not_dominated_on_rate_up_and_metric_down() {
    // (pass rate, cost)
    let pts = [(0.5, 1.0), (0.7, 2.0), (0.7, 3.0), (0.4, 0.5), (0.4, 0.9)];
    assert_eq!(pareto_flags(&pts), [true, true, false, true, false]);
}

#[test]
fn test_f9_identical_points_should_not_dominate_each_other() {
    assert_eq!(pareto_flags(&[(0.5, 1.0), (0.5, 1.0)]), [true, true]);
    assert_eq!(pareto_flags(&[(0.5, 1.0)]), [true]);
    assert!(pareto_flags(&[]).is_empty());
}

#[test]
fn test_f9_summary_should_count_timeouts_and_errors_as_failures_but_separately() {
    let runs = vec![
        pass("a"),
        pass("b"),
        fail("c"),
        stat("d", TaskStatus::Timeout, 0.5),
        stat("e", TaskStatus::AgentError, 0.0),
        stat("f", TaskStatus::InfraError, 0.0),
    ];

    let s = summarize("lean", &runs);

    assert_eq!(s.profile, "lean");
    assert_eq!((s.runs, s.passes, s.timeouts, s.errors), (6, 2, 1, 2));
    assert!(close(s.pass_rate, 2.0 / 6.0, 1e-9));
    let (lo, hi) = wilson_interval(2, 6);
    assert!(close(s.wilson_low, lo, 1e-9) && close(s.wilson_high, hi, 1e-9));
}

#[test]
fn test_f9_summary_should_report_mean_and_median_cost_context_tokens_and_calls() {
    let mut runs = vec![pass("a"), pass("b"), pass("c"), pass("d")];
    for (r, (cost, ctx, calls)) in runs.iter_mut().zip([(0.1, 100, 1), (0.2, 200, 2), (0.3, 300, 3), (1.4, 400, 6)]) {
        r.cost_usd = cost;
        r.context_tokens = ctx;
        r.model_calls = calls;
    }

    let s = summarize("p", &runs);

    assert!(close(s.mean_cost_usd, 0.5, 1e-9), "{}", s.mean_cost_usd);
    assert!(close(s.median_cost_usd, 0.25, 1e-9), "{}", s.median_cost_usd);
    assert!(close(s.mean_context_tokens, 250.0, 1e-9));
    assert!(close(s.mean_model_calls, 3.0, 1e-9));
}

#[test]
fn test_f9_paired_comparison_should_count_tasks_better_worse_and_tied() {
    // Two repeats per task: pass rates base a=1.0 b=0.5 c=0.0 d=1.0 ; other a=1.0 b=1.0 c=0.5 d=0.5
    let base = [pass("a"), pass("a"), pass("b"), fail("b"), fail("c"), fail("c"), pass("d"), pass("d")];
    let other = [pass("a"), pass("a"), pass("b"), pass("b"), pass("c"), fail("c"), pass("d"), fail("d")];

    let p = paired_comparison(&base, &other, 2_000, 7);

    assert_eq!(p.tasks, 4);
    assert_eq!((p.better, p.worse, p.tied), (2, 1, 1));
    assert!(close(p.mean_diff, (0.0 + 0.5 + 0.5 - 0.5) / 4.0, 1e-9), "{}", p.mean_diff);
    assert!(p.ci_low <= p.mean_diff && p.mean_diff <= p.ci_high, "{p:?}");
}

#[test]
fn test_f9_paired_comparison_should_use_only_tasks_both_profiles_ran() {
    let base = [pass("a"), fail("b"), pass("only-base")];
    let other = [fail("a"), pass("b"), pass("only-other")];

    let p = paired_comparison(&base, &other, 1_000, 1);

    assert_eq!(p.tasks, 2);
    assert_eq!((p.better, p.worse, p.tied), (1, 1, 0));
}

#[test]
fn test_f9_the_bootstrap_should_be_deterministic_for_a_seed() {
    let base: Vec<RunStat> = (0..8).map(|i| if i % 2 == 0 { pass(&format!("t{i}")) } else { fail(&format!("t{i}")) }).collect();
    let other: Vec<RunStat> = (0..8).map(|i| pass(&format!("t{i}"))).collect();

    let a = paired_comparison(&base, &other, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    let b = paired_comparison(&base, &other, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);

    assert_eq!((a.ci_low, a.ci_high), (b.ci_low, b.ci_high));
    assert_eq!(BOOTSTRAP_RESAMPLES, 10_000);
    assert!(close(a.mean_diff, 0.5, 1e-9));
    // Diffs are 0 or 1 per task: any resampled mean lies in [0, 1], and the interval
    // brackets the observed mean strictly inside it.
    assert!(0.0 <= a.ci_low && a.ci_low < 0.5 && 0.5 < a.ci_high && a.ci_high <= 1.0, "{a:?}");
}

#[test]
fn test_f9_a_unanimous_difference_should_give_a_degenerate_interval() {
    let base: Vec<RunStat> = (0..5).map(|i| fail(&format!("t{i}"))).collect();
    let other: Vec<RunStat> = (0..5).map(|i| pass(&format!("t{i}"))).collect();

    let p = paired_comparison(&base, &other, 1_000, 3);

    assert!(close(p.mean_diff, 1.0, 1e-12) && close(p.ci_low, 1.0, 1e-12) && close(p.ci_high, 1.0, 1e-12), "{p:?}");
    assert_eq!((p.better, p.worse, p.tied), (5, 0, 0));
}

#[test]
fn test_f9_a_timeout_in_the_other_profile_should_count_as_a_failure_in_the_pairing() {
    let base = [pass("a")];
    let other = [stat("a", TaskStatus::Timeout, 1.0)];
    let p = paired_comparison(&base, &other, 100, 1);
    assert_eq!((p.better, p.worse, p.tied), (0, 1, 0));
    assert!(close(p.mean_diff, -1.0, 1e-12));
}
