//! Report statistics as pure functions (F9): Wilson intervals, Pareto marking,
//! per-profile summaries and the paired bootstrap against the base profile.

use std::collections::BTreeMap;

use crate::runner::TaskStatus;

/// Bootstrap resamples of the tasks for the paired comparison.
pub const BOOTSTRAP_RESAMPLES: usize = 10_000;
/// Fixed seed of the bootstrap, so a report is reproducible.
pub const BOOTSTRAP_SEED: u64 = 0x486f_6e65_4576_616c;

const Z95: f64 = 1.96;

/// Wilson score interval (95%) for `passes` of `n`, as (low, high); (0, 1) when `n` is 0.
pub fn wilson_interval(passes: u64, n: u64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let n = n as f64;
    let p = passes as f64 / n;
    let z2 = Z95 * Z95;
    let denom = 1.0 + z2 / n;
    let centre = (p + z2 / (2.0 * n)) / denom;
    let half = Z95 * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denom;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// Which points are not dominated, with x higher-is-better and y lower-is-better.
/// A point is dominated when another is at least as good on both and strictly
/// better on one.
pub fn pareto_flags(points: &[(f64, f64)]) -> Vec<bool> {
    points
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            !points
                .iter()
                .enumerate()
                .any(|(j, &(ox, oy))| j != i && ox >= x && oy <= y && (ox > x || oy < y))
        })
        .collect()
}

/// What the statistics need from one run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunStat {
    pub task: String,
    pub status: TaskStatus,
    pub cost_usd: f64,
    pub context_tokens: u64,
    pub model_calls: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProfileSummary {
    pub profile: String,
    pub runs: usize,
    pub passes: usize,
    pub timeouts: usize,
    /// `agent_error` plus `infra_error`.
    pub errors: usize,
    pub pass_rate: f64,
    pub wilson_low: f64,
    pub wilson_high: f64,
    pub mean_cost_usd: f64,
    pub median_cost_usd: f64,
    pub mean_context_tokens: f64,
    pub mean_model_calls: f64,
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    }
}

/// Summarise one profile's runs. Timeouts and errors count as failures in the
/// pass rate, and are also counted separately.
pub fn summarize(profile: &str, runs: &[RunStat]) -> ProfileSummary {
    let count = |status: TaskStatus| runs.iter().filter(|r| r.status == status).count();
    let passes = count(TaskStatus::Pass);
    let n = runs.len();
    let (wilson_low, wilson_high) = wilson_interval(passes as u64, n as u64);
    ProfileSummary {
        profile: profile.to_owned(),
        runs: n,
        passes,
        timeouts: count(TaskStatus::Timeout),
        errors: count(TaskStatus::AgentError) + count(TaskStatus::InfraError),
        pass_rate: if n == 0 {
            0.0
        } else {
            passes as f64 / n as f64
        },
        wilson_low,
        wilson_high,
        mean_cost_usd: mean(runs.iter().map(|r| r.cost_usd)),
        median_cost_usd: median(runs.iter().map(|r| r.cost_usd).collect()),
        mean_context_tokens: mean(runs.iter().map(|r| r.context_tokens as f64)),
        mean_model_calls: mean(runs.iter().map(|r| f64::from(r.model_calls))),
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Paired {
    pub tasks: usize,
    pub mean_diff: f64,
    pub better: usize,
    pub worse: usize,
    pub tied: usize,
    pub ci_low: f64,
    pub ci_high: f64,
}

/// Pass rate per task over its repeats.
fn pass_rates(runs: &[RunStat]) -> BTreeMap<&str, f64> {
    let mut acc: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for r in runs {
        let e = acc.entry(r.task.as_str()).or_default();
        e.1 += 1;
        if r.status == TaskStatus::Pass {
            e.0 += 1;
        }
    }
    acc.into_iter()
        .map(|(t, (p, n))| (t, p as f64 / n as f64))
        .collect()
}

/// splitmix64: a small, fixed PRNG, so the bootstrap does not depend on a crate.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Paired comparison of `other` with `base` over the tasks both ran: the
/// per-task pass-rate difference (`other - base`), how many tasks are better,
/// worse and tied, and a 95% percentile bootstrap interval of the mean
/// difference over `resamples` resamples of the tasks.
pub fn paired_comparison(
    base: &[RunStat],
    other: &[RunStat],
    resamples: usize,
    seed: u64,
) -> Paired {
    let (b, o) = (pass_rates(base), pass_rates(other));
    let diffs: Vec<f64> = b
        .iter()
        .filter_map(|(task, br)| o.get(task).map(|or| or - br))
        .collect();
    let n = diffs.len();
    if n == 0 {
        return Paired {
            tasks: 0,
            mean_diff: 0.0,
            better: 0,
            worse: 0,
            tied: 0,
            ci_low: 0.0,
            ci_high: 0.0,
        };
    }
    const EPS: f64 = 1e-12;
    let better = diffs.iter().filter(|d| **d > EPS).count();
    let worse = diffs.iter().filter(|d| **d < -EPS).count();
    let mean_diff = diffs.iter().sum::<f64>() / n as f64;

    let mut rng = SplitMix64(seed);
    let mut means: Vec<f64> = (0..resamples)
        .map(|_| {
            let sum: f64 = (0..n)
                .map(|_| diffs[(rng.next() % n as u64) as usize])
                .sum();
            sum / n as f64
        })
        .collect();
    means.sort_by(f64::total_cmp);
    let (ci_low, ci_high) = if means.is_empty() {
        (mean_diff, mean_diff)
    } else {
        let last = means.len() - 1;
        let lo = ((means.len() as f64) * 0.025).floor() as usize;
        let hi = (((means.len() as f64) * 0.975).ceil() as usize).saturating_sub(1);
        (means[lo.min(last)], means[hi.min(last)])
    };
    Paired {
        tasks: n,
        mean_diff,
        better,
        worse,
        tied: n - better - worse,
        ci_low,
        ci_high,
    }
}
