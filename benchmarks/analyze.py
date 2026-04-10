#!/usr/bin/env python3
"""
analyze.py — Parse Hone benchmark results and produce comparison tables + charts.

USAGE
    # Show all ablation results in ./benchmarks/results/
    python3 benchmarks/analyze.py

    # Show a specific run
    python3 benchmarks/analyze.py --run-id 2026-04-12-ablation-a0

    # Compare two runs side-by-side
    python3 benchmarks/analyze.py --compare 2026-04-12-ablation-a0 2026-04-12-ablation-a8

    # Generate all ablation results as a waterfall chart (requires matplotlib)
    python3 benchmarks/analyze.py --chart

    # Output CSV instead of pretty table
    python3 benchmarks/analyze.py --format csv
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import pathlib
import random
import sys
from dataclasses import dataclass, field
from typing import Optional

RESULTS_ROOT = pathlib.Path(__file__).parent / "results"

# ---------------------------------------------------------------------------
# Data model
# ---------------------------------------------------------------------------

ABLATION_ORDER = ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8"]

ABLATION_LABELS = {
    "a0": "A0  Baseline",
    "a1": "A1  +G1 edit uniqueness",
    "a2": "A2  +G2 line-aware read",
    "a3": "A3  +G3 multi-edit",
    "a4": "A4  +G4 grep tool",
    "a5": "A5  +G5 symbol search",
    "a6": "A6  +G6 real system prompt",
    "a7": "A7  +G7+G8 plan-first + parallel",
    "a8": "A8  +G9-G12 verify + retry + compact",
}


@dataclass
class RunResult:
    run_id: str
    hone_sha: str
    model: str
    benchmark: str
    config: str
    pass_at_1: float
    resolved: int
    total: int
    usd_total: float
    usd_per_resolved: float
    p50_wall_s: int
    p95_wall_s: int
    median_turns: int
    input_tok: int
    output_tok: int
    cache_hit_rate: float
    run_date: str

    @classmethod
    def from_csv_row(cls, row: dict, run_id: str) -> "RunResult":
        def _float(v: str) -> float:
            try:
                return float(v)
            except (ValueError, TypeError):
                return 0.0

        def _int(v: str) -> int:
            try:
                return int(v)
            except (ValueError, TypeError):
                return 0

        return cls(
            run_id=run_id,
            hone_sha=row.get("hone_sha", "unknown"),
            model=row.get("model", "unknown"),
            benchmark=row.get("benchmark", "unknown"),
            config=row.get("config", "unknown"),
            pass_at_1=_float(row.get("pass_at_1", "0")),
            resolved=_int(row.get("resolved", "0")),
            total=_int(row.get("total", "0")),
            usd_total=_float(row.get("usd_total", "0")),
            usd_per_resolved=_float(row.get("usd_per_resolved", "0")),
            p50_wall_s=_int(row.get("p50_wall_s", "0")),
            p95_wall_s=_int(row.get("p95_wall_s", "0")),
            median_turns=_int(row.get("median_turns", "0")),
            input_tok=_int(row.get("input_tok", "0")),
            output_tok=_int(row.get("output_tok", "0")),
            cache_hit_rate=_float(row.get("cache_hit_rate", "0")),
            run_date=row.get("run_date", "unknown"),
        )


# ---------------------------------------------------------------------------
# Result loading
# ---------------------------------------------------------------------------


def load_run(run_dir: pathlib.Path) -> Optional[RunResult]:
    """Load a single run from its results directory."""
    summary_csv = run_dir / "summary.csv"
    if not summary_csv.exists():
        return None

    try:
        with open(summary_csv, newline="") as f:
            reader = csv.DictReader(f)
            rows = list(reader)
        if not rows:
            return None
        return RunResult.from_csv_row(rows[0], run_dir.name)
    except Exception as e:
        print(f"WARNING: Failed to load {summary_csv}: {e}", file=sys.stderr)
        return None


def load_all_runs(results_root: pathlib.Path = RESULTS_ROOT) -> list[RunResult]:
    """Load all completed runs from the results directory."""
    if not results_root.exists():
        return []

    runs = []
    for run_dir in sorted(results_root.iterdir()):
        if not run_dir.is_dir():
            continue
        result = load_run(run_dir)
        if result is not None:
            runs.append(result)
    return runs


def load_run_by_id(run_id: str, results_root: pathlib.Path = RESULTS_ROOT) -> Optional[RunResult]:
    """Load a specific run by ID."""
    run_dir = results_root / run_id
    if not run_dir.exists():
        # Try prefix match
        candidates = [d for d in results_root.iterdir() if d.name.startswith(run_id)]
        if len(candidates) == 1:
            run_dir = candidates[0]
        elif len(candidates) > 1:
            print(f"Ambiguous run ID '{run_id}'. Matches:", file=sys.stderr)
            for c in candidates:
                print(f"  {c.name}", file=sys.stderr)
            return None
        else:
            print(f"Run ID '{run_id}' not found in {results_root}", file=sys.stderr)
            return None
    return load_run(run_dir)


# ---------------------------------------------------------------------------
# Bootstrap confidence interval
# ---------------------------------------------------------------------------


def bootstrap_ci(
    passed: list[int], total: int, n_resamples: int = 10_000, ci: float = 0.95
) -> tuple[float, float]:
    """
    Compute a bootstrap 95% CI on pass@1 from a list of 0/1 task outcomes.

    Parameters
    ----------
    passed : list of 0 or 1, one per task
    total  : total tasks (len(passed))
    """
    if total == 0:
        return (0.0, 0.0)

    estimates = []
    for _ in range(n_resamples):
        sample = random.choices(passed, k=total)
        estimates.append(sum(sample) / total)
    estimates.sort()

    alpha = (1.0 - ci) / 2.0
    lo = estimates[int(alpha * n_resamples)]
    hi = estimates[int((1 - alpha) * n_resamples)]
    return (lo, hi)


def load_task_outcomes(run_dir: pathlib.Path) -> list[int]:
    """Load per-task pass/fail as a list of 0/1 from tasks.jsonl."""
    tasks_file = run_dir / "tasks.jsonl"
    if not tasks_file.exists():
        return []
    outcomes = []
    for line in tasks_file.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            task = json.loads(line)
            outcomes.append(1 if task.get("resolved", False) else 0)
        except json.JSONDecodeError:
            pass
    return outcomes


# ---------------------------------------------------------------------------
# Formatting helpers
# ---------------------------------------------------------------------------


def fmt_pct(v: float) -> str:
    return f"{v * 100:.1f}%"


def fmt_usd(v: float) -> str:
    return f"${v:.3f}"


def fmt_mtok(v: int) -> str:
    return f"{v / 1_000_000:.2f}M"


def fmt_time(seconds: int) -> str:
    if seconds < 60:
        return f"{seconds}s"
    return f"{seconds // 60}m{seconds % 60:02d}s"


# ---------------------------------------------------------------------------
# Table rendering
# ---------------------------------------------------------------------------


COLUMNS = [
    ("Config",          "config",           lambda r: ABLATION_LABELS.get(r.config, r.config)),
    ("pass@1",          "pass_at_1",        lambda r: fmt_pct(r.pass_at_1)),
    ("Resolved",        "resolved",         lambda r: f"{r.resolved}/{r.total}"),
    ("$/resolved",      "usd_per_resolved", lambda r: fmt_usd(r.usd_per_resolved)),
    ("USD total",       "usd_total",        lambda r: fmt_usd(r.usd_total)),
    ("p50 wall",        "p50_wall_s",       lambda r: fmt_time(r.p50_wall_s)),
    ("Med turns",       "median_turns",     lambda r: str(r.median_turns)),
    ("Input",           "input_tok",        lambda r: fmt_mtok(r.input_tok)),
    ("Date",            "run_date",         lambda r: r.run_date),
    ("Model",           "model",            lambda r: r.model[:28]),
]


def render_table(runs: list[RunResult], show_ci: bool = False) -> str:
    """Render runs as a plain-text aligned table."""
    if not runs:
        return "(no results)"

    results_root = RESULTS_ROOT

    rows: list[list[str]] = []
    header = [col[0] for col in COLUMNS]
    if show_ci:
        header.append("95% CI")

    for r in runs:
        row = [fmt(r) for _, _, fmt in COLUMNS]
        if show_ci:
            run_dir = results_root / r.run_id
            outcomes = load_task_outcomes(run_dir)
            if outcomes:
                lo, hi = bootstrap_ci(outcomes, len(outcomes))
                row.append(f"[{fmt_pct(lo)}, {fmt_pct(hi)}]")
            else:
                row.append("n/a")
        rows.append(row)

    # Column widths
    widths = [len(h) for h in header]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))

    sep = "+" + "+".join("-" * (w + 2) for w in widths) + "+"
    fmt_row = lambda cells: "|" + "|".join(f" {c:<{w}} " for c, w in zip(cells, widths)) + "|"

    lines = [sep, fmt_row(header), sep]
    for row in rows:
        lines.append(fmt_row(row))
    lines.append(sep)
    return "\n".join(lines)


def render_ablation_waterfall(runs: list[RunResult]) -> str:
    """Render an ASCII waterfall showing incremental deltas A0 -> A8."""
    config_map = {r.config: r for r in runs}

    lines = [
        "",
        "Ablation waterfall (cumulative pass@1 delta)",
        "=" * 60,
    ]

    prev_pass = None
    for cfg in ABLATION_ORDER:
        if cfg not in config_map:
            lines.append(f"  {ABLATION_LABELS.get(cfg, cfg):<45}  (missing)")
            continue
        r = config_map[cfg]
        pct = r.pass_at_1 * 100

        if prev_pass is None:
            delta_str = f"{pct:>6.1f}%"
            bar = ""
        else:
            delta = pct - prev_pass
            sign = "+" if delta >= 0 else ""
            delta_str = f"{pct:>6.1f}% ({sign}{delta:.1f}pp)"
            bar_len = max(0, int(abs(delta) * 2))
            bar_char = "█" if delta >= 0 else "░"
            bar = bar_char * bar_len

        lines.append(f"  {ABLATION_LABELS.get(cfg, cfg):<45}  {delta_str}  {bar}")
        prev_pass = pct

    return "\n".join(lines)


def render_csv(runs: list[RunResult]) -> str:
    """Render runs as CSV."""
    import io

    buf = io.StringIO()
    fieldnames = [
        "run_id", "hone_sha", "model", "benchmark", "config",
        "pass_at_1", "resolved", "total", "usd_total", "usd_per_resolved",
        "p50_wall_s", "p95_wall_s", "median_turns",
        "input_tok", "output_tok", "cache_hit_rate", "run_date",
    ]
    writer = csv.DictWriter(buf, fieldnames=fieldnames)
    writer.writeheader()
    for r in runs:
        writer.writerow({f: getattr(r, f) for f in fieldnames})
    return buf.getvalue()


# ---------------------------------------------------------------------------
# Chart generation (optional — requires matplotlib)
# ---------------------------------------------------------------------------


def generate_charts(runs: list[RunResult], output_dir: pathlib.Path) -> None:
    try:
        import matplotlib.pyplot as plt
        import matplotlib.ticker as ticker
    except ImportError:
        print("matplotlib not installed. Skipping chart generation.", file=sys.stderr)
        print("  pip install matplotlib", file=sys.stderr)
        return

    output_dir.mkdir(parents=True, exist_ok=True)

    # --- Chart 1: Ablation waterfall bar chart ---
    ablation_runs = sorted(
        [r for r in runs if r.config in ABLATION_ORDER],
        key=lambda r: ABLATION_ORDER.index(r.config),
    )

    if ablation_runs:
        configs = [r.config for r in ablation_runs]
        pass_rates = [r.pass_at_1 * 100 for r in ablation_runs]
        labels = [ABLATION_LABELS.get(c, c) for c in configs]

        fig, ax = plt.subplots(figsize=(12, 6))
        bars = ax.bar(range(len(configs)), pass_rates, color="#4c72b0", edgecolor="white")
        ax.set_xticks(range(len(configs)))
        ax.set_xticklabels(labels, rotation=25, ha="right", fontsize=9)
        ax.set_ylabel("pass@1 (%)")
        ax.set_title("Hone Scaffold Ablation — Aider Polyglot (225 tasks)")
        ax.yaxis.set_major_formatter(ticker.PercentFormatter())
        ax.set_ylim(0, max(pass_rates) * 1.15 if pass_rates else 100)

        for bar, rate in zip(bars, pass_rates):
            ax.text(
                bar.get_x() + bar.get_width() / 2,
                bar.get_height() + 0.3,
                f"{rate:.1f}%",
                ha="center",
                va="bottom",
                fontsize=8,
            )

        plt.tight_layout()
        out_path = output_dir / "ablation_waterfall.png"
        plt.savefig(out_path, dpi=150)
        plt.close()
        print(f"Chart saved: {out_path}")

    # --- Chart 2: Cost vs quality scatter ---
    fig, ax = plt.subplots(figsize=(10, 7))

    for r in runs:
        label = ABLATION_LABELS.get(r.config, r.config)
        ax.scatter(r.usd_per_resolved, r.pass_at_1 * 100, s=80, zorder=5)
        ax.annotate(
            label[:20],
            (r.usd_per_resolved, r.pass_at_1 * 100),
            textcoords="offset points",
            xytext=(5, 3),
            fontsize=7,
        )

    ax.set_xlabel("$ per resolved task")
    ax.set_ylabel("pass@1 (%)")
    ax.set_title("Hone: Cost vs Quality by Scaffold Config")
    ax.yaxis.set_major_formatter(ticker.PercentFormatter())
    plt.tight_layout()
    out_path = output_dir / "cost_vs_quality.png"
    plt.savefig(out_path, dpi=150)
    plt.close()
    print(f"Chart saved: {out_path}")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Analyze Hone benchmark results",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    p.add_argument(
        "--run-id",
        metavar="RUN_ID",
        help="Show a specific run (or prefix)",
    )
    p.add_argument(
        "--compare",
        nargs=2,
        metavar=("RUN_A", "RUN_B"),
        help="Compare two runs side-by-side",
    )
    p.add_argument(
        "--results-dir",
        type=pathlib.Path,
        default=RESULTS_ROOT,
        help=f"Results root directory (default: {RESULTS_ROOT})",
    )
    p.add_argument(
        "--format",
        choices=["table", "csv", "json"],
        default="table",
        help="Output format (default: table)",
    )
    p.add_argument(
        "--chart",
        action="store_true",
        help="Generate PNG charts (requires matplotlib)",
    )
    p.add_argument(
        "--chart-dir",
        type=pathlib.Path,
        default=None,
        help="Directory for chart output (default: results/charts/)",
    )
    p.add_argument(
        "--ci",
        action="store_true",
        help="Show 95%% bootstrap confidence intervals (slow — 10k resamples)",
    )
    p.add_argument(
        "--benchmark",
        help="Filter by benchmark name (e.g. aider_polyglot, swebench_verified)",
    )
    return p.parse_args()


def main() -> None:
    args = parse_args()
    results_root: pathlib.Path = args.results_dir

    if args.compare:
        run_a = load_run_by_id(args.compare[0], results_root)
        run_b = load_run_by_id(args.compare[1], results_root)
        if run_a is None or run_b is None:
            sys.exit(1)
        runs = [run_a, run_b]
    elif args.run_id:
        result = load_run_by_id(args.run_id, results_root)
        if result is None:
            sys.exit(1)
        runs = [result]
    else:
        runs = load_all_runs(results_root)

    if args.benchmark:
        runs = [r for r in runs if r.benchmark == args.benchmark]

    if not runs:
        print("No results found.", file=sys.stderr)
        print(f"  Results root: {results_root}", file=sys.stderr)
        print("  Run `make ablation` or `make headline` first.", file=sys.stderr)
        sys.exit(0)

    # Sort: ablations by canonical order first, then by date
    def sort_key(r: RunResult) -> tuple:
        try:
            return (0, ABLATION_ORDER.index(r.config), r.run_date)
        except ValueError:
            return (1, 0, r.run_date)

    runs.sort(key=sort_key)

    # Output
    if args.format == "csv":
        print(render_csv(runs))
    elif args.format == "json":
        import dataclasses
        print(json.dumps([dataclasses.asdict(r) for r in runs], indent=2))
    else:
        print()
        print(render_table(runs, show_ci=args.ci))

        # Waterfall for ablation runs
        ablation_runs = [r for r in runs if r.config in ABLATION_ORDER]
        if len(ablation_runs) > 1:
            print(render_ablation_waterfall(ablation_runs))

        print()
        print(f"  Total runs shown: {len(runs)}")
        if any(r.benchmark == "aider_polyglot" for r in runs):
            total_usd = sum(r.usd_total for r in runs if r.benchmark == "aider_polyglot")
            print(f"  Total ablation cost: ${total_usd:.2f}")

    if args.chart:
        chart_dir = args.chart_dir or (results_root / "charts")
        generate_charts(runs, chart_dir)


if __name__ == "__main__":
    main()
