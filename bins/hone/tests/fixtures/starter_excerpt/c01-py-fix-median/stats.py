"""Small statistics helpers."""


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
