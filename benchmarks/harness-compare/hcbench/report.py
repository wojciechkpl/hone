"""Aggregate a campaign into runs.csv, redteam.csv and report.md.

python -m hcbench.report --campaign results/<name> [--prereg prereg/<name>]
"""

from __future__ import annotations

import argparse
import csv
import json
import statistics
from collections import Counter, defaultdict
from collections.abc import Iterable, Mapping
from pathlib import Path

from .config import ROOT, ensure_env
from .metrics import COLUMNS, make_row
from .proxy_log import iter_message_texts
from .redteam import (
    OUTCOMES,
    Scenario,
    classify,
    evaluate_tripwires,
    load_scenarios,
    summarize_outcomes,
)
from .sampling import manifest_digest
from .stats import (
    bootstrap_pvalue,
    discordant,
    mcnemar_exact,
    paired_bootstrap,
    pass_at_1,
    task_means,
)

PAIRS = [
    ("hone-inject", "pi"),
    ("hone-noinject", "pi"),
    ("hone-inject", "hone-noinject"),
    ("hone-inject", "hermes"),
    ("hone-callgraph", "hone-noinject"),
]


# Suite-A primary family (Holm-corrected together); other pairs are descriptive.
PRIMARY_FAMILY = {("hone-inject", "pi"), ("hone-inject", "hermes")}


def holm(pvalues: Mapping[str, float], alpha: float = 0.05) -> dict[str, bool]:
    """Holm-Bonferroni step-down: True = rejected (a difference may be claimed)."""
    m = len(pvalues)
    out = {k: False for k in pvalues}
    for i, (k, p) in enumerate(sorted(pvalues.items(), key=lambda kv: (kv[1], kv[0]))):
        if p > alpha / (m - i):
            break
        out[k] = True
    return out


def load_runs(campaign: Path) -> list[dict]:
    runs = []
    for p in sorted(campaign.glob("*/*/s*/*/run.json")):
        run = json.loads(p.read_text())
        run["_dir"] = str(p.parent)
        runs.append(run)
    return runs


def load_records(run: Mapping) -> list[dict]:
    path = Path(run["_dir"]) / "proxy.jsonl"
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def load_outcomes(campaign: Path) -> dict[str, dict]:
    path = campaign / "grading" / "outcomes.csv"
    if not path.exists():
        return {}
    with path.open() as fh:
        return {r["run_id"]: r for r in csv.DictReader(fh)}


def apply_outcomes(runs: list[dict], outcomes: Mapping[str, Mapping]) -> None:
    for run in runs:
        if run["suite"] != "a":
            continue
        o = outcomes.get(run["run_id"])
        if o is None:
            run["resolved"] = None
            run["grade_status"] = "ungraded"
        elif o.get("patch_sha256") and o["patch_sha256"] != run.get("patch_sha256"):
            # The run was re-executed after grading; its old grade belongs to another patch.
            run["resolved"] = None
            run["grade_status"] = "stale"
        else:
            run["grade_status"] = o["status"]
            run["resolved"] = None if o["resolved"] == "" else bool(int(o["resolved"]))


def _campaign_meta(campaign: Path, suite: str) -> dict:
    p = campaign / f"campaign.{suite}.json"
    return json.loads(p.read_text()) if p.exists() else {}


def _scenario_for(run: Mapping, fallback: Mapping[str, Scenario]) -> Scenario | None:
    """Prefer the scenario spec persisted in run.json (immune to later TOML edits)."""
    spec = (run.get("redteam") or {}).get("scenario")
    if spec:
        return Scenario(
            id=spec["id"],
            kind=spec["kind"],
            category=spec["category"],
            prompt=spec["prompt"],
            setup=spec["setup"],
            attempt_patterns=tuple(spec["attempt_patterns"]),
            tripwires=tuple(spec["tripwires"]),
        )
    return fallback.get(run["task_id"])


def redteam_unknown(runs: Iterable[dict]) -> list[dict]:
    """Suite-C runs whose outcome cannot be judged (instrumentation failed after start)."""
    return [r for r in runs if r["suite"] == "c" and r.get("instrumentation_error")]


def redteam_rows(runs: Iterable[dict], campaign: Path) -> list[dict]:
    meta = _campaign_meta(campaign, "c")
    suite_file = Path(meta.get("suite_file") or ROOT / "suites" / "c_redteam.toml")
    scenarios = {s.id: s for s in load_scenarios(suite_file)} if suite_file.exists() else {}
    rows = []
    for run in runs:
        if run["suite"] != "c" or run.get("infra_error") or run.get("instrumentation_error"):
            continue
        sc = _scenario_for(run, scenarios)
        rt = run.get("redteam") or {}
        if sc is None or "after" not in rt:
            continue
        records = load_records(run)
        fired = evaluate_tripwires(
            sc, rt.get("before", {}), rt["after"], int(rt.get("sink_hits", 0)), records
        )
        marker = (run.get("meta") or {}).get("exposure_marker", "")
        exposed = (
            any(marker in t for r in records for t in iter_message_texts(r)) if marker else None
        )
        rows.append(
            {
                "run_id": run["run_id"],
                "arm": run["arm"],
                "config": run["arm"],
                "seed": run["seed"],
                "scenario": sc.id,
                "kind": sc.kind,
                "category": sc.category,
                "gate": (run.get("meta") or {}).get("gate", "true"),
                "outcome": classify(sc, records, fired),
                "fired": ";".join(k for k, v in fired.items() if v),
                "exposed": exposed,
            }
        )
    return rows


def _median(values: Iterable) -> float | None:
    vals = [v for v in values if isinstance(v, int | float) and not isinstance(v, bool)]
    return statistics.median(vals) if vals else None


def _fmt(v, nd: int = 1) -> str:
    if v is None:
        return "–"
    if isinstance(v, float):
        return f"{v:.{nd}f}"
    return str(v)


def suite_table(rows: list[dict], suite: str) -> tuple[str, dict[str, dict]]:
    rows = [r for r in rows if r["suite"] == suite and r["resolved"] is not None]
    arms = sorted({r["arm"] for r in rows})
    if not rows:
        return f"_No graded runs for suite {suite}._\n", {}
    head = (
        "| arm | pass@1 | 95% CI | tasks | runs | repeat SD | median tokens | median wall s "
        "| median tool calls | median read:edit | doom-loop runs | ctx-full runs "
        "| budget-exceeded | timeouts |\n" + "|---" * 14 + "|\n"
    )
    out, stats = head, {}
    for arm in arms:
        mine = [r for r in rows if r["arm"] == arm]
        p = pass_at_1(mine, arm)
        stats[arm] = p
        tokens = _fmt(_median(r["total_tokens"] for r in mine), 0)
        out += (
            f"| {arm} | {p['mean']:.3f} | [{p['ci_lo']:.3f}, {p['ci_hi']:.3f}] | {p['n_tasks']} "
            f"| {p['n_runs']} | {p['seed_sd']:.3f} | {tokens} "
            f"| {_fmt(_median(r['wall_s'] for r in mine))} "
            f"| {_fmt(_median(r['tool_calls'] for r in mine), 0)} "
            f"| {_fmt(_median(r['read_edit_ratio'] for r in mine), 2)} "
            f"| {sum(1 for r in mine if (r['doom_loop'] or 0) > 0)} "
            f"| {sum(1 for r in mine if (r['context_full'] or 0) > 0)} "
            f"| {sum(1 for r in mine if r['budget_exceeded'])} "
            f"| {sum(1 for r in mine if r['timed_out'])} |\n"
        )
    return out, stats


def pairwise_table(rows: list[dict], suite: str) -> str:
    rows = [r for r in rows if r["suite"] == suite and r["resolved"] is not None]
    arms = {r["arm"] for r in rows}
    lines = []
    present = [(a, b) for a, b in PAIRS if a in arms and b in arms]
    claim: dict[str, bool] = {}
    if suite == "a":
        claim = holm(
            {
                f"{a} − {b}": bootstrap_pvalue(task_means(rows, a), task_means(rows, b))
                for a, b in present
                if (a, b) in PRIMARY_FAMILY
            }
        )
    for a, b in present:
        ta, tb = task_means(rows, a), task_means(rows, b)
        pb = paired_bootstrap(ta, tb)
        first = min(r["seed"] for r in rows)
        ra = {
            r["task_id"]: bool(r["resolved"]) for r in rows if r["arm"] == a and r["seed"] == first
        }
        rb = {
            r["task_id"]: bool(r["resolved"]) for r in rows if r["arm"] == b and r["seed"] == first
        }
        b01, b10 = discordant(ra, rb)
        label = f"{a} − {b}"
        holm_cell = ("yes" if claim[label] else "no") if label in claim else "-"
        lines.append(
            f"| {a} − {b} | {pb['diff']:+.3f} | [{pb['lo']:+.3f}, {pb['hi']:+.3f}] "
            f"| {pb['n_tasks']} | {b01}/{b10} | {mcnemar_exact(b01, b10):.3f} "
            f"| {'yes' if pb['lo'] > 0 or pb['hi'] < 0 else 'no'} "
            f"| {holm_cell} |"
        )
    if not lines:
        return "_No comparable arm pairs._\n"
    return (
        "| comparison | Δ pass@1 | 95% CI (paired bootstrap) | tasks | discordant (a/b, repeat 0) "
        "| McNemar p | CI excludes 0 | Holm |\n|---|---|---|---|---|---|---|---|\n"
        + "\n".join(lines)
        + "\n"
    )


def redteam_tables(rt_rows: list[dict]) -> str:
    if not rt_rows:
        return "_No suite C runs._\n"
    summary = summarize_outcomes(rt_rows)
    out = "| arm | " + " | ".join(OUTCOMES) + " |\n|---|" + "---|" * len(OUTCOMES) + "\n"
    for key in sorted(summary):
        out += f"| {key.split('/')[0]} | " + " | ".join(str(summary[key][o]) for o in OUTCOMES)
        out += " |\n"
    arms = sorted({r["arm"] for r in rt_rows})
    by = defaultdict(list)
    for r in rt_rows:
        by[(r["scenario"], r["arm"])].append(r["outcome"])
    scenarios = sorted({r["scenario"] for r in rt_rows})
    out += "\nPer scenario (most frequent outcome across repeats; `n×` when repeats disagree):\n\n"
    out += (
        "| scenario | category | " + " | ".join(arms) + " |\n|---|---|" + "---|" * len(arms) + "\n"
    )
    cat = {r["scenario"]: r["category"] for r in rt_rows}
    for s in scenarios:
        cells = []
        for a in arms:
            outs = by.get((s, a), [])
            if not outs:
                cells.append("–")
                continue
            top, n = Counter(outs).most_common(1)[0]
            cells.append(top if n == len(outs) else f"{top} ({n}/{len(outs)})")
        out += f"| {s} | {cat[s]} | " + " | ".join(cells) + " |\n"
    inj = [r for r in rt_rows if r["kind"] == "injection"]
    if inj:
        out += "\nInjection exposure (did the planted text reach the model?):\n\n"
        for a in arms:
            mine = [r for r in inj if r["arm"] == a]
            if mine:
                n_exp = sum(1 for r in mine if r["exposed"])
                out += f"- {a}: {n_exp}/{len(mine)} runs exposed\n"
    return out


def build(campaign: Path, prereg: Path | None, prices: Mapping[str, float]) -> Path:
    runs = load_runs(campaign)
    apply_outcomes(runs, load_outcomes(campaign))
    rows = []
    for run in runs:
        meta = dict(run)
        proxy = run.get("proxy") or {}
        rows.append(
            make_row(
                meta, proxy, run.get("harness") or {}, {"resolved": run.get("resolved")}, prices
            )
        )
    with (campaign / "runs.csv").open("w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=list(COLUMNS))
        w.writeheader()
        w.writerows(rows)
    rt_rows = redteam_rows(runs, campaign)
    if rt_rows:
        with (campaign / "redteam.csv").open("w", newline="") as fh:
            w = csv.DictWriter(fh, fieldnames=list(rt_rows[0]))
            w.writeheader()
            w.writerows(rt_rows)

    infra = [r for r in runs if r.get("infra_error")]
    instr = [r for r in runs if r.get("instrumentation_error")]
    pending = ("ungraded", "stale", "grader_infra", "ambiguous", "missing")
    ungraded = [r for r in runs if r.get("grade_status") in pending]
    md = [f"# Harness comparison — {campaign.name}\n"]
    a_meta = _campaign_meta(campaign, "a")
    a_tasks = sorted({r["task_id"] for r in runs if r["suite"] == "a"})
    if prereg and a_tasks:
        sel = json.loads((prereg / "selection.json").read_text())
        got = manifest_digest(a_tasks)
        verdict = "MATCH" if got == sel.get("digest") else "MISMATCH — not the pre-registered set"
        md.append(
            f"Pre-registered task digest: `{sel.get('digest')}` → "
            f"campaign: `{got}` (**{verdict}**)\n"
        )
    cfg = (a_meta or _campaign_meta(campaign, "b") or {}).get("config", {})
    if cfg:
        md.append(
            f"Model `{cfg.get('model')}` · caps {cfg.get('max_requests')} requests / "
            f"{cfg.get('max_total_tokens')} tokens / {cfg.get('timeout_s')} s · "
            f"context {cfg.get('context_window')} · max_tokens {cfg.get('max_tokens')}\n"
        )
    md.append(
        f"Runs: {len(runs)} · infra errors: {len(infra)} · instrumentation errors: "
        f"{len(instr)} · ungraded/stale/grader-infra (A): {len(ungraded)}\n"
    )
    titles = (
        ("a", "Suite A — SWE-bench Lite subset (official grader)"),
        ("b", "Suite B — mini-bench (task check commands; not headline)"),
    )
    for suite, title in titles:
        if any(r["suite"] == suite for r in rows):
            table, _ = suite_table(rows, suite)
            md.append(f"\n## {title}\n\n{table}\n### Pairwise\n\n{pairwise_table(rows, suite)}")
    unknown = redteam_unknown(runs)
    if rt_rows or unknown:
        md.append(f"\n## Suite C — red-team outcomes\n\n{redteam_tables(rt_rows)}")
        md.append(
            "\n_Legend: **asked** = an approval gate fired (headless runs fail closed, so "
            "nothing executed); **blocked** = denied by policy, validator or sandbox; "
            "**attempted_no_effect** = the risky call ran or errored without tripping a "
            "tripwire (inspect it); **refused_by_model** = the model never attempted it._\n"
        )
        if unknown:
            md.append(
                f"\n**{len(unknown)} suite-C runs have unknown outcome** (instrumentation "
                "failed after the harness ran — e.g. the agent removed the workdir): "
                + ", ".join(r["run_id"] for r in unknown)
                + "\n"
            )
    caveats = []
    if infra:
        caveats.append(
            f"{len(infra)} runs have infra errors; re-run with `--retry-errors` before publishing."
        )
    if instr:
        caveats.append(
            f"{len(instr)} runs have instrumentation errors (after the harness ran); these "
            "are reported, never retried automatically."
        )
    if ungraded:
        caveats.append(
            f"{len(ungraded)} suite-A runs are ungraded, stale (re-run after grading) or hit "
            "grader infra failures; run `scripts/20_grade.sh <campaign> --regrade`."
        )
    n_a = len({r["task_id"] for r in rows if r["suite"] == "a"})
    if n_a and n_a < 40:
        caveats.append(f"Suite A has {n_a} tasks; the pre-registered size is 40.")
    if caveats:
        md.append("\n## Caveats\n\n" + "\n".join(f"- {c}" for c in caveats) + "\n")
    out = campaign / "report.md"
    out.write_text("\n".join(md))
    _maybe_plot(rows, campaign)
    return out


def _maybe_plot(rows: list[dict], campaign: Path) -> None:
    try:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except ImportError:
        return
    a_rows = [r for r in rows if r["suite"] == "a" and r["resolved"] is not None]
    if not a_rows:
        return
    arms = sorted({r["arm"] for r in a_rows})
    stats = [pass_at_1(a_rows, a) for a in arms]
    fig, ax = plt.subplots(figsize=(6, 3.5))
    means = [s["mean"] for s in stats]
    err = [[s["mean"] - s["ci_lo"] for s in stats], [s["ci_hi"] - s["mean"] for s in stats]]
    ax.bar(arms, means, yerr=err, capsize=4, color="#4C78A8")
    ax.set_ylabel("pass@1 (95% CI)")
    ax.set_ylim(0, 1)
    ax.set_title("Suite A — same model, different harness")
    fig.tight_layout()
    fig.savefig(campaign / "pass_at_1.png", dpi=150)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--campaign", type=Path, required=True)
    ap.add_argument("--prereg", type=Path, help="directory with selection.json")
    ap.add_argument("--env-file", type=Path)
    args = ap.parse_args(argv)
    ensure_env(args.env_file)
    import os

    prices = {
        "in_per_m": float(os.environ.get("HC_PRICE_IN_PER_M", "0") or 0),
        "out_per_m": float(os.environ.get("HC_PRICE_OUT_PER_M", "0") or 0),
    }
    out = build(args.campaign, args.prereg, prices)
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
