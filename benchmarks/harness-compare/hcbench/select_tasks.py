"""Select the suite-A task list (stratified by repo) for pre-registration.

    uv run --with datasets python -m hcbench.select_tasks --out prereg/2026-11

Writes tasks.txt, instances.jsonl (prompt fields), dataset.json (full rows, used
by the grader) and selection.json. Commit them *before*
any graded run; the report verifies the campaign against the digest.
"""

from __future__ import annotations

import argparse
import json
import time
from collections import Counter
from pathlib import Path

from .sampling import allocate, manifest_digest, stratified_sample

KEEP = ("instance_id", "repo", "base_commit", "problem_statement", "version", "created_at", "image")


def _resolved_revision(dataset: str, revision: str | None) -> str | None:
    try:
        from huggingface_hub import HfApi

        return HfApi().dataset_info(dataset, revision=revision).sha
    except Exception:  # best effort: offline or hub API change
        return None


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--dataset", default="SWE-bench/SWE-bench_Lite")
    ap.add_argument("--split", default="test")
    ap.add_argument("--revision", default=None)
    ap.add_argument("--n", type=int, default=40)
    ap.add_argument("--seed", type=int, default=20261109)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args(argv)

    from datasets import load_dataset

    ds = load_dataset(args.dataset, split=args.split, revision=args.revision or None)
    full_rows = {row["instance_id"]: dict(row) for row in ds}
    population = [{k: row.get(k) for k in KEEP} for row in full_rows.values()]
    ids = stratified_sample(population, args.n, args.seed)
    chosen = set(ids)
    sizes = Counter(p["repo"] for p in population)

    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "tasks.txt").write_text("\n".join(ids) + "\n")
    with (args.out / "instances.jsonl").open("w") as fh:
        for p in sorted(population, key=lambda p: p["instance_id"]):
            if p["instance_id"] in chosen:
                fh.write(json.dumps(p, ensure_ascii=False) + "\n")
    # Full rows (incl. test lists) for the grader, so grading is pinned to what was
    # pre-registered rather than whatever the hub serves on grading day.
    (args.out / "dataset.json").write_text(
        json.dumps([full_rows[i] for i in ids], indent=1, ensure_ascii=False, default=str)
    )
    selection = {
        "dataset": args.dataset,
        "split": args.split,
        "revision": args.revision,
        "resolved_revision": _resolved_revision(args.dataset, args.revision),
        "n": args.n,
        "seed": args.seed,
        "method": "largest-remainder allocation by repo, floor 1; random.Random(f'{seed}:{repo}')",
        "population_size": len(population),
        "population_digest": manifest_digest(p["instance_id"] for p in population),
        "allocation": allocate(dict(sizes), args.n),
        "digest": manifest_digest(ids),
        "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    (args.out / "selection.json").write_text(json.dumps(selection, indent=2) + "\n")
    print(f"selected {len(ids)} tasks from {len(population)}; digest {selection['digest']}")
    for repo, k in sorted(selection["allocation"].items()):
        print(f"  {repo:32s} {k:3d} / {sizes[repo]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
