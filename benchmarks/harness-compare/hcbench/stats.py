"""Statistics for pass@1 reporting: Wilson, bootstrap, paired tests."""

from __future__ import annotations

import math
import random
import statistics
from collections import defaultdict
from collections.abc import Iterable, Mapping, Sequence


def wilson_interval(successes: int, n: int, z: float = 1.959964) -> tuple[float, float]:
    if successes < 0 or successes > n:
        raise ValueError(f"invalid counts: successes={successes}, n={n}")
    if n == 0:
        return (0.0, 1.0)
    p = successes / n
    denom = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / denom
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denom
    return (max(0.0, centre - half), min(1.0, centre + half))


def _percentiles(samples: list[float], alpha: float) -> tuple[float, float]:
    samples.sort()
    k = len(samples)
    lo_idx = max(0, min(k - 1, math.floor(alpha / 2 * k)))
    hi_idx = max(0, min(k - 1, math.ceil((1 - alpha / 2) * k) - 1))
    return samples[lo_idx], samples[hi_idx]


def bootstrap_mean_ci(
    values: Sequence[float], iters: int = 10000, seed: int = 0, alpha: float = 0.05
) -> tuple[float, float]:
    vals = list(values)
    if not vals:
        raise ValueError("values must not be empty")
    if min(vals) == max(vals):
        return (vals[0], vals[0])
    rng = random.Random(seed)
    k = len(vals)
    means = [sum(rng.choices(vals, k=k)) / k for _ in range(iters)]
    return _percentiles(means, alpha)


def paired_bootstrap(
    a: Mapping[str, float],
    b: Mapping[str, float],
    iters: int = 10000,
    seed: int = 0,
    alpha: float = 0.05,
) -> dict:
    keys = sorted(set(a) & set(b))
    if not keys:
        raise ValueError("no shared tasks between the two arms")
    diffs = [a[k] - b[k] for k in keys]
    diff = sum(diffs) / len(diffs)
    if min(diffs) == max(diffs):
        lo = hi = diffs[0]
    else:
        rng = random.Random(seed)
        n = len(diffs)
        boots = [sum(rng.choices(diffs, k=n)) / n for _ in range(iters)]
        lo, hi = _percentiles(boots, alpha)
        lo, hi = min(lo, diff), max(hi, diff)
    return {"diff": diff, "lo": lo, "hi": hi, "n_tasks": len(keys)}


def mcnemar_exact(b: int, c: int) -> float:
    n = b + c
    if n == 0:
        return 1.0
    tail = sum(math.comb(n, i) for i in range(min(b, c) + 1)) / 2**n
    return min(1.0, 2 * tail)


def discordant(a: Mapping[str, bool], b: Mapping[str, bool]) -> tuple[int, int]:
    keys = set(a) & set(b)
    a_only = sum(1 for k in keys if a[k] and not b[k])
    b_only = sum(1 for k in keys if not a[k] and b[k])
    return a_only, b_only


def task_means(rows: Iterable[Mapping], arm: str, metric: str = "resolved") -> dict[str, float]:
    acc: dict[str, list[float]] = defaultdict(list)
    for row in rows:
        if row["arm"] != arm:
            continue
        acc[row["task_id"]].append(float(bool(row[metric])))
    return {t: sum(v) / len(v) for t, v in acc.items()}


def pass_at_1(rows: Iterable[Mapping], arm: str, iters: int = 10000, seed: int = 0) -> dict:
    mine = [r for r in rows if r["arm"] == arm]
    if not mine:
        raise ValueError(f"no rows for arm {arm!r}")
    means = task_means(mine, arm)
    by_seed: dict[int, list[float]] = defaultdict(list)
    for r in mine:
        by_seed[r["seed"]].append(float(bool(r["resolved"])))
    seed_rates = [sum(v) / len(v) for _, v in sorted(by_seed.items())]
    seed_sd = statistics.stdev(seed_rates) if len(seed_rates) > 1 else 0.0
    values = list(means.values())
    lo, hi = bootstrap_mean_ci(values, iters=iters, seed=seed)
    return {
        "mean": sum(values) / len(values),
        "ci_lo": lo,
        "ci_hi": hi,
        "n_tasks": len(means),
        "n_runs": len(mine),
        "seed_sd": seed_sd,
    }
