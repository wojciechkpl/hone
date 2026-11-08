"""Pre-registered, deterministic stratified task selection."""

from __future__ import annotations

import hashlib
import math
import random
from collections import defaultdict
from collections.abc import Iterable, Mapping, Sequence


def allocate(sizes: Mapping[str, int], n: int) -> dict[str, int]:
    """Largest-remainder allocation of ``n`` draws with a floor of one per stratum."""
    if not sizes:
        raise ValueError("sizes must not be empty")
    if any(s <= 0 for s in sizes.values()):
        raise ValueError("every stratum size must be positive")
    total = sum(sizes.values())
    if n < len(sizes):
        raise ValueError(f"n={n} is smaller than the number of strata ({len(sizes)})")
    if n > total:
        raise ValueError(f"n={n} exceeds the population size ({total})")

    quota = {k: n * s / total for k, s in sizes.items()}
    base = {k: min(sizes[k], max(1, math.floor(q))) for k, q in quota.items()}

    while sum(base.values()) > n:
        candidates = [k for k in base if base[k] > 1]
        # Largest overshoot first; ties broken by name ascending.
        k = min(candidates, key=lambda k: (-(base[k] - quota[k]), k))
        base[k] -= 1
    while sum(base.values()) < n:
        candidates = [k for k in base if base[k] < sizes[k]]
        k = min(candidates, key=lambda k: (-(quota[k] - base[k]), k))
        base[k] += 1
    return base


def stratified_sample(
    instances: Sequence[Mapping[str, str]], n: int, seed: int, key: str = "repo"
) -> list[str]:
    groups: dict[str, list[str]] = defaultdict(list)
    for inst in instances:
        groups[inst[key]].append(inst["instance_id"])
    alloc = allocate({k: len(v) for k, v in groups.items()}, n)
    chosen: list[str] = []
    for stratum in sorted(groups):
        rng = random.Random(f"{seed}:{stratum}")
        chosen.extend(rng.sample(sorted(groups[stratum]), alloc[stratum]))
    return sorted(chosen)


def manifest_digest(ids: Iterable[str]) -> str:
    payload = "\n".join(sorted(ids)) + "\n"
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()
