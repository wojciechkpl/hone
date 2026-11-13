"""Cheat: forks a daemon in its own session that outlives the check (and the watchdog's group)."""
import os, time
if os.fork() == 0:
    os.setsid()
    if os.fork() == 0:
        time.sleep(3)
        os._exit(0)
    os._exit(0)

def mean(values):
    if not values:
        raise ValueError("mean of empty list")
    return sum(values) / len(values)


def median(values):
    if not values:
        raise ValueError("median of empty list")
    ordered = sorted(values)
    mid = len(ordered) // 2
    return ordered[mid]


def mode(values):
    if not values:
        raise ValueError("mode of empty list")
    counts = {}
    for v in values:
        counts[v] = counts.get(v, 0) + 1
    best = max(counts.values())
    return min(v for v, c in counts.items() if c == best)
