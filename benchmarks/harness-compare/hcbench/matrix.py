"""Run a suite across arms × repeats. Resumable: finished runs are skipped.

    python -m hcbench.matrix --suite a --out results/2026-11-14-main \
        --arms pi,hone-noinject,hone-inject --seeds 0,1,2 --concurrency 4

Order is repeat-major, then task, then arm, so a campaign stopped early still
has complete, balanced repeats for every arm.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

from .arms import load_arms, render
from .config import ROOT, BenchConfig, ensure_env
from .prereg import dirty_paths, pre_registered_files, repo_head, sha256_file
from .runner import execute_run
from .suites import (
    Task,
    load_redteam_suite,
    load_swebench_suite,
    load_task_suite,
    parse_digests,
)

DEFAULT_SUITE_FILES = {
    "b": ROOT / "suites" / "b_mini.toml",
    "c": ROOT / "suites" / "c_redteam.toml",
    "smoke": ROOT / "suites" / "smoke.toml",
}


def load_tasks(args: argparse.Namespace, cfg: BenchConfig) -> list[Task]:
    if args.suite == "a":
        if not args.instances:
            raise SystemExit("suite a needs --instances (from prereg: instances.jsonl)")
        ids = None
        if args.tasks_file:
            ids = [
                ln.strip() for ln in Path(args.tasks_file).read_text().splitlines() if ln.strip()
            ]
        if not args.image_digests:
            raise SystemExit("suite a needs --image-digests (prereg images.digests)")
        pinned = parse_digests(Path(args.image_digests))
        try:
            return load_swebench_suite(
                Path(args.instances), Path(args.prompt), cfg, ids, pinned=pinned
            )
        except ValueError as e:
            raise SystemExit(str(e)) from e
    path = Path(args.suite_file) if args.suite_file else DEFAULT_SUITE_FILES[args.suite]
    if args.suite == "c":
        return load_redteam_suite(path, cfg)
    return load_task_suite(path, args.suite, cfg)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--suite", required=True, choices=["a", "b", "c", "smoke"])
    ap.add_argument("--out", required=True, type=Path, help="campaign directory")
    ap.add_argument("--arms", required=True, help="comma-separated arm names from arms.toml")
    ap.add_argument("--seeds", default="0", help="repeat indices, e.g. 0,1,2 (seed = base + i)")
    ap.add_argument("--arms-file", type=Path, default=ROOT / "config" / "arms.toml")
    ap.add_argument("--suite-file", help="override suite TOML (b, c, smoke)")
    ap.add_argument("--instances", help="suite a: instances.jsonl from select_tasks")
    ap.add_argument("--tasks-file", help="suite a: pre-registered tasks.txt (one id per line)")
    ap.add_argument("--image-digests", help="suite a: prereg images.digests (pinned references)")
    ap.add_argument("--prompt", default=str(ROOT / "config" / "prompts" / "swebench.md"))
    ap.add_argument("--task-ids", help="comma-separated subset of task ids")
    ap.add_argument("--limit", type=int, help="first N tasks only (debugging)")
    ap.add_argument("--concurrency", type=int, default=1)
    ap.add_argument("--force", action="store_true", help="re-run finished runs")
    ap.add_argument("--retry-errors", action="store_true", help="re-run runs with infra errors")
    ap.add_argument("--dry-run", action="store_true", help="print the run list and exit")
    ap.add_argument("--env-file", type=Path)
    args = ap.parse_args(argv)

    ensure_env(args.env_file)
    cfg = BenchConfig.from_env()
    arms_all = load_arms(args.arms_file)
    arm_names = [a.strip() for a in args.arms.split(",") if a.strip()]
    unknown = [a for a in arm_names if a not in arms_all]
    if unknown:
        raise SystemExit(f"unknown arms: {unknown}; known: {sorted(arms_all)}")
    seeds = [int(s) for s in args.seeds.split(",") if s.strip()]
    tasks = load_tasks(args, cfg)
    if args.task_ids:
        keep = {t.strip() for t in args.task_ids.split(",")}
        tasks = [t for t in tasks if t.task_id in keep]
    if args.limit:
        tasks = tasks[: args.limit]

    # Fail fast on template errors (unknown {{placeholders}}) before any container starts.
    probe = {k: "x" for k in ("prompt", "model", "base_url", "run_id", "workdir",
                               "context_window", "max_tokens")}  # fmt: skip
    for name in arm_names:
        try:
            render(arms_all[name], probe)
        except KeyError as e:
            raise SystemExit(f"arm {name!r} uses unknown placeholder {e}") from e

    plan = [(t, arms_all[a], s) for s in seeds for t in tasks for a in arm_names]
    print(f"[matrix] suite={args.suite} tasks={len(tasks)} arms={arm_names} seeds={seeds} "
          f"runs={len(plan)} out={args.out}", flush=True)  # fmt: skip
    if args.dry_run:
        for t, a, s in plan:
            print(f"  {t.suite}.{a.name}.s{s}.{t.task_id}  image={t.image}")
        return 0

    args.out.mkdir(parents=True, exist_ok=True)
    manifest = Path(cfg.bundle_dir) / "manifest.json"
    try:
        bundle = json.loads(manifest.read_text())
    except (OSError, json.JSONDecodeError):
        bundle = {}
    # Hash the suite and arms files actually used (K49), the rest from the rig.
    used = pre_registered_files()
    paths: dict[str, str] = {}
    suite_key = {"b": "suite_b", "c": "suite_c", "smoke": "suite_smoke"}.get(args.suite)
    if suite_key:
        used[suite_key] = (
            Path(args.suite_file) if args.suite_file else DEFAULT_SUITE_FILES[args.suite]
        )
        paths[suite_key] = str(used[suite_key])
    used["arms_toml"] = args.arms_file
    paths["arms_toml"] = str(args.arms_file)
    files = {k: sha256_file(p) for k, p in used.items() if p.exists()}
    dirty = dirty_paths(ignore_dir=ROOT / "prereg")  # prereg/ is where the pre-registration lives
    invocation = {
        "suite": args.suite,
        "arms": arm_names,
        "seeds": seeds,
        "tasks": [t.task_id for t in tasks],
        "suite_file": str(args.suite_file or DEFAULT_SUITE_FILES.get(args.suite, "")),
        "instances": args.instances,
        "tasks_file": args.tasks_file,
        "image_digests": args.image_digests,
        "prompt": args.prompt if args.suite == "a" else None,
        "config": cfg.as_dict(),
        # Evidence for the report's conformance check (spec 11.6 K43).
        "bundle": bundle,
        "files": files,
        "paths": paths,
        "head": repo_head(),
        "dirty": bool(dirty),
        "dirty_paths": dirty,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "argv": sys.argv,
    }
    # The first invocation defines the campaign; later ones (resume, retries) are appended.
    first = args.out / f"campaign.{args.suite}.json"
    if not first.exists():
        first.write_text(json.dumps(invocation, indent=2))
    with (args.out / "invocations.jsonl").open("a") as fh:
        fh.write(json.dumps(invocation) + "\n")

    # The red-team sink is shared, so suite C runs one container at a time.
    workers = 1 if args.suite == "c" else max(1, args.concurrency)
    counts = {"infra": 0, "instrumentation": 0, "crashed": 0, "mismatch": 0}
    done = 0

    def one(item):
        t, a, s = item
        return execute_run(t, a, s, cfg, args.out, force=args.force, retry_errors=args.retry_errors)

    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {pool.submit(one, item): item for item in plan}
        for fut in as_completed(futures):
            done += 1
            t, a, s = futures[fut]
            try:
                res = fut.result()
            except Exception as e:  # keep the campaign going; reported at the end
                counts["crashed"] += 1
                print(f"[{done}/{len(plan)}] CRASH {t.suite}.{a.name}.s{s}.{t.task_id}: "
                      f"{type(e).__name__}: {e}", flush=True)  # fmt: skip
                continue
            counts["infra"] += bool(res.get("infra_error"))
            counts["instrumentation"] += bool(res.get("instrumentation_error"))
            counts["mismatch"] += bool(res.get("fingerprint_mismatch"))
            px = res.get("proxy") or {}
            tag = "MISMATCH" if res.get("fingerprint_mismatch") else (
                "skip" if res.get("skipped") else "done")  # fmt: skip
            err = res.get("infra_error") or res.get("instrumentation_error") or "-"
            print(
                f"[{done}/{len(plan)}] {tag} {res['run_id']} exit={res.get('exit_code')} "
                f"resolved={res.get('resolved')} wall={res.get('wall_s')}s "
                f"req={px.get('requests')} tok={px.get('total_tokens')} err={err}",
                flush=True,
            )
    print(
        f"[matrix] finished: {done} runs; infra errors {counts['infra']}, instrumentation "
        f"errors {counts['instrumentation']}, crashes {counts['crashed']}",
        flush=True,
    )
    if counts["mismatch"]:
        print(f"[matrix] {counts['mismatch']} finished runs were produced by a different bundle/"
              "arm/config/task and were NOT reused. Use a new campaign name, or --force to "
              "overwrite (log it in DEVIATIONS.md).", flush=True)  # fmt: skip
    return 1 if any(counts.values()) else 0


if __name__ == "__main__":
    raise SystemExit(main())
