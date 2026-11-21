"""Suite A on SWE-rebench: select the tasks by the recorded rules, then check their images.

    python -m hcbench.rebench select --dataset D --revision R --min-split S --min-created DAY \
        --n N --seed SEED --grader G --out DIR
    python -m hcbench.rebench check-images --dir DIR

The pure functions (``split_eligible``, ``created_date``, ``eligible_rows``, ``choose_tasks``) need
no network. The four seams (``resolve_revision``, ``list_splits``, ``load_split``,
``run_command``) are module-level so tests can replace them. Spec section 11.17, K93 to K104.
"""

from __future__ import annotations

import argparse
import datetime
import json
import platform
import re
import subprocess
import time
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from .sampling import manifest_digest, select_suite_a
from .suites import REBENCH_PATH

SPLIT_NAME = re.compile(r"[0-9]{4}_(0[1-9]|1[0-2])")
ISO_DAY = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}")
PINNED = re.compile(r"\S+@sha256:[0-9a-f]{64}")
METHOD = "hcbench.sampling.select_suite_a"
IMAGE_WORKDIR = "/testbed"
BLOCK_SEPARATOR = "---"
CHECK_SCRIPT = (
    f"cd {IMAGE_WORKDIR} && git rev-parse HEAD; echo {BLOCK_SEPARATOR}; "
    f"git status --porcelain; echo {BLOCK_SEPARATOR}; "
    f"PATH={REBENCH_PATH} python -c 'import sys; print(sys.version)'; true"
)


# ---- K93 pure functions ------------------------------------------------------------------


def split_eligible(name: str, min_split: str) -> bool:
    """True for a ``YYYY_MM`` split name that is ``min_split`` or later, compared as text."""
    return isinstance(name, str) and SPLIT_NAME.fullmatch(name) is not None and name >= min_split


def created_date(value: Any) -> str | None:
    """The ISO day (ten characters) a creation value starts with, or None."""
    if value is None:
        return None
    day = str(value).strip()[:10]
    if not ISO_DAY.fullmatch(day):
        return None
    try:
        datetime.date.fromisoformat(day)
    except ValueError:
        return None
    return day


def eligible_rows(
    rows_by_split: Mapping[str, Sequence[Mapping[str, Any]]], min_split: str, min_created: str
) -> list[dict[str, Any]]:
    """Rows of eligible splits created on ``min_created`` or later, sorted by instance id."""
    kept = []
    for split, rows in rows_by_split.items():
        if not split_eligible(split, min_split):
            continue
        for src in rows:
            day = created_date(src.get("created_at"))
            if day is None or day < min_created:
                continue
            kept.append({**src, "instance_id": str(src["instance_id"]),
                         "repo": str(src["repo"]), "split": split})  # fmt: skip
    return sorted(kept, key=lambda r: r["instance_id"])


def choose_tasks(rows: Sequence[Mapping[str, str]], n: int, seed: int) -> list[str]:
    return select_suite_a(rows, n, seed)


# ---- seams -------------------------------------------------------------------------------


def resolve_revision(dataset: str, revision: str) -> str | None:
    """The commit SHA of ``revision`` (empty: the hub's main), or None when it can't be read."""
    try:
        from huggingface_hub import HfApi

        return HfApi().dataset_info(dataset, revision=revision or None).sha
    except (ImportError, OSError, ValueError):
        return None


def list_splits(dataset: str, sha: str) -> list[str]:
    from datasets import get_dataset_split_names

    return list(get_dataset_split_names(dataset, revision=sha))


def load_split(dataset: str, split: str, sha: str) -> list[dict[str, Any]]:
    from datasets import load_dataset

    return [dict(r) for r in load_dataset(dataset, split=split, revision=sha)]


def run_command(argv: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(argv, capture_output=True, text=True, check=False)


# ---- select ------------------------------------------------------------------------------


def row_image(row: Mapping[str, Any]) -> str:
    """The row's image (K94): image_name or docker_image; refuse none, or two that differ."""
    name, docker = (str(row.get(k) or "").strip() for k in ("image_name", "docker_image"))
    if not name and not docker:
        raise SystemExit(
            f"task {row['instance_id']} has no image (image_name and docker_image empty)"
        )
    if name and docker and name != docker:
        raise SystemExit(
            f"task {row['instance_id']}: image_name {name!r} differs from docker_image {docker!r}"
        )
    return name or docker


def check_unique(rows: Sequence[Mapping[str, Any]]) -> None:
    seen: dict[str, str] = {}
    for r in rows:
        iid = r["instance_id"]
        if iid in seen:
            raise SystemExit(
                f"instance_id {iid} appears twice in eligible splits {seen[iid]} and {r['split']}"
            )
        seen[iid] = r["split"]


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=1, ensure_ascii=False, default=str) + "\n")


def select(args: argparse.Namespace) -> int:
    sha = resolve_revision(args.dataset, args.revision)
    if not sha:
        raise SystemExit(
            f"cannot resolve revision {args.revision or 'main'!r} of {args.dataset}; "
            "refusing an unpinned load"
        )
    names = sorted(s for s in list_splits(args.dataset, sha) if split_eligible(s, args.min_split))
    rows_by_split = {s: load_split(args.dataset, s, sha) for s in names}
    rows = eligible_rows(rows_by_split, args.min_split, args.min_created)
    if not rows:
        raise SystemExit(
            f"no eligible task in {args.dataset} at revision {args.revision or 'main'} ({sha}) "
            f"with min_split {args.min_split} and min_created {args.min_created}"
        )
    check_unique(rows)
    images = {r["instance_id"]: row_image(r) for r in rows}
    chosen = choose_tasks(rows, args.n, args.seed)
    by_id = {r["instance_id"]: r for r in rows}
    selection = {
        "dataset": args.dataset,
        "revision": args.revision,
        "resolved_revision": sha,
        "splits": sorted({r["split"] for r in rows}),
        "min_split": args.min_split,
        "min_created": args.min_created,
        "n": args.n,
        "seed": args.seed,
        "method": METHOD,
        "eligible_count": len(rows),
        "eligible_repos": len({r["repo"] for r in rows}),
        "eligible_digest": manifest_digest(by_id),
        "digest": manifest_digest(chosen),
        "python_version": platform.python_version(),
        "python_implementation": platform.python_implementation(),
        "grader": args.grader,
        "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    instances = [
        {
            "instance_id": i,
            "repo": by_id[i]["repo"],
            "base_commit": by_id[i].get("base_commit"),
            "problem_statement": by_id[i].get("problem_statement"),
            "created_at": str(by_id[i]["created_at"]),
            "split": by_id[i]["split"],
            "image": images[i],
        }
        for i in sorted(chosen)
    ]
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "tasks.txt").write_text("".join(f"{i}\n" for i in sorted(chosen)))
    (out / "instances.jsonl").write_text(
        "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in instances)
    )
    write_json(
        out / "dataset.json",
        [{k: v for k, v in by_id[i].items() if k != "split"} for i in sorted(chosen)],
    )
    write_json(out / "selection.json", selection)
    print(f"selected {len(chosen)} of {len(rows)} eligible tasks into {out}")
    return 0


# ---- check-images ------------------------------------------------------------------------


def parse_digests(path: Path) -> dict[str, str]:
    pinned: dict[str, str] = {}
    for line in path.read_text().splitlines():
        parts = line.split()
        if len(parts) == 2 and PINNED.fullmatch(parts[1]):
            pinned[parts[0]] = parts[1]
    return pinned


def judge(proc: subprocess.CompletedProcess, base_commit: str) -> dict[str, Any]:
    """One task's result: ``ok`` and the failed parts (HEAD, git status, Python)."""
    if proc.returncode != 0:
        return {"ok": False, "failed": [f"container did not run (exit {proc.returncode})"]}
    blocks = [b.strip() for b in re.split(rf"(?m)^{BLOCK_SEPARATOR}\s*$", proc.stdout or "")]
    blocks += [""] * (3 - len(blocks))
    head, status, python = blocks[:3]
    failed = []
    if head != base_commit:
        failed.append(f"HEAD {head or 'unreadable'} is not the base commit {base_commit}")
    if status:
        failed.append("git status is not empty")
    if not python:
        failed.append("python does not run")
    return {"ok": not failed, "head": head, "status": status, "python": python, "failed": failed}


def check_images(args: argparse.Namespace) -> int:
    d = Path(args.dir)
    ids = [ln for ln in (d / "tasks.txt").read_text().splitlines() if ln.strip()]
    recs = {
        r["instance_id"]: r
        for r in map(json.loads, (d / "instances.jsonl").read_text().splitlines())
    }
    pinned = parse_digests(d / "images.digests")
    for i in ids:
        if recs[i]["image"] not in pinned:
            raise SystemExit(f"no pinned digest for the image of {i} ({recs[i]['image']})")
    selection = json.loads((d / "selection.json").read_text())
    results, excluded = {}, dict(selection.get("excluded") or {})
    for i in ids:
        argv = ["docker", "run", "--rm", "--network", "none", "--entrypoint", "bash",
                pinned[recs[i]["image"]], "-c", CHECK_SCRIPT]  # fmt: skip
        results[i] = judge(run_command(argv), str(recs[i]["base_commit"]))
        if not results[i]["ok"]:
            excluded[i] = "; ".join(results[i]["failed"])
    passing = [i for i in ids if results[i]["ok"]]
    write_json(d / "images.check.json", results)
    (d / "tasks.txt").write_text("".join(f"{i}\n" for i in passing))
    selection["excluded"] = excluded
    selection["run_digest"] = manifest_digest(passing)
    write_json(d / "selection.json", selection)
    print(f"{len(passing)} of {len(ids)} images passed the check")
    if not passing:
        raise SystemExit("every task failed the image check; suite A would be empty")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    sub = ap.add_subparsers(dest="command", required=True)
    s = sub.add_parser("select", help="select suite A tasks (K93-K96)")
    s.add_argument("--dataset", required=True)
    s.add_argument("--revision", default="", help="empty: the hub's main")
    s.add_argument("--min-split", required=True)
    s.add_argument("--min-created", required=True)
    s.add_argument("--n", type=int, required=True)
    s.add_argument("--seed", type=int, required=True)
    s.add_argument("--grader", required=True)
    s.add_argument("--out", type=Path, required=True)
    c = sub.add_parser("check-images", help="check each pinned image (K104)")
    c.add_argument("--dir", type=Path, required=True)
    args = ap.parse_args(argv)
    return select(args) if args.command == "select" else check_images(args)


if __name__ == "__main__":
    raise SystemExit(main())
