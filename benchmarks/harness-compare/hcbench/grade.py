"""Grade suite-A patches with the official SWE-bench harness.

    uv run --with 'swebench==5.0.2' python -m hcbench.grade --campaign results/<name> \
        --dataset prereg/<name>/dataset.json

One predictions file per (arm, repeat), so each SWE-bench run sees one patch
per instance. The swebench run_id and report directory include a hash of the
predictions, so a grade is never reused for different patches. Writes
<campaign>/grading/outcomes.csv (run_id, patch_sha256, resolved, status).

Status → resolved:
  resolved                                  1
  unresolved | empty_patch                  0
  apply_failed | error (tests errored/timed out, patch-caused)   0
  grader_infra | ambiguous | missing        blank → re-grade (exit 1)
  ambiguous_repeated (fork mode: 2+ ambiguous attempts after the last grader_infra)   0
  instrumentation_error (the run broke its own measurement; not graded)   0
  grader_infra_final (fork mode: 3 grader_infra attempts in a row)        blank, never re-graded
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import importlib.metadata
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

from .suites import parse_digests

APPLY_PATCH_FAIL = ">>>>> Patch Apply Failed"
REGRADE = {"grader_infra", "ambiguous", "missing"}
BLANK = REGRADE | {"grader_infra_final"}  # no 0 or 1: left out of pass@1
FORK_LOG_PREFIX = r"\d{4}-\d\d-\d\d \d\d:\d\d:\d\d,\d{3}"  # %(asctime)s of the fork
GRADER_INFRA_FINAL_AFTER = 3  # consecutive grader_infra attempts (K135)

# Fork mode (K129): the fork (swebench 4.0.3) has no infrastructure classifier, so its logs are
# classified here. The signature table below, from `INFRA_FAILURE_SIGNATURES` in swebench 5.0.2
# (swebench/harness/infra_failure.py, MIT licence, (c) the SWE-bench authors), is ported in the
# same order and tiers. The first three entries are written only by the fork.
TIER_ENVIRONMENT = "environment"  # cannot plausibly be caused by the patch
TIER_AMBIGUOUS = "ambiguous"  # a broken environment or a bad patch
INFRA_FAILURE_SIGNATURES: tuple[tuple[str, str, str], ...] = (
    (
        "image_unavailable",
        TIER_ENVIRONMENT,
        r"Error building image|Error occurred while pulling image",
    ),
    ("container_create_failed", TIER_ENVIRONMENT, r"Error creating container"),
    ("docker_api_timeout", TIER_ENVIRONMENT, r"Read timed out|UnixHTTPConnectionPool"),
    ("browser_launch_failed", TIER_ENVIRONMENT, r"Failed to launch|Failed to connect to the bus"),
    (
        "display_unavailable",
        TIER_ENVIRONMENT,
        r"cannot open display|Missing X server|unable to open X display",
    ),
    ("out_of_memory", TIER_ENVIRONMENT, r"Cannot allocate memory|OutOfMemoryError|^Killed$"),
    (
        "container_unavailable",
        TIER_ENVIRONMENT,
        r"Error response from daemon|Cannot connect to the Docker daemon",
    ),
    (
        "network_unreachable",
        TIER_ENVIRONMENT,
        r"Could not resolve host|Temporary failure in name resolution",
    ),
    ("missing_module", TIER_AMBIGUOUS, r"Cannot find module|MODULE_NOT_FOUND|ModuleNotFoundError"),
    ("no_tests_collected", TIER_AMBIGUOUS, r"no tests ran|collected 0 items"),
    (
        "tests_timed_out",
        TIER_AMBIGUOUS,
        r"Timeout error: \d+ seconds exceeded|Test timed out after \d+ seconds",
    ),
)
_COMPILED = tuple((r, t, re.compile(p, re.MULTILINE)) for r, t, p in INFRA_FAILURE_SIGNATURES)


def collect(campaign: Path) -> dict[tuple[str, int], list[dict]]:
    groups: dict[tuple[str, int], list[dict]] = defaultdict(list)
    for run_json in sorted((campaign / "a").glob("*/s*/*/run.json")):
        run = json.loads(run_json.read_text())
        if run.get("infra_error") or run.get("instrumentation_error"):
            continue
        patch = (run_json.parent / "patch.diff").read_text()
        groups[(run["arm"], run["seed"])].append(
            {
                "run_id": run["run_id"],
                "instance_id": run["task_id"],
                "patch": patch,
                "patch_sha256": hashlib.sha256(patch.encode()).hexdigest(),
            }
        )
    return groups


def collect_instrumented(campaign: Path) -> list[str]:
    """K145: run ids of suite A runs that failed after the harness started (no patch to grade)."""
    out = []
    for run_json in sorted((campaign / "a").glob("*/s*/*/run.json")):
        run = json.loads(run_json.read_text())
        if run.get("instrumentation_error") and not run.get("infra_error"):
            out.append(run["run_id"])
    return out


def read_report(report_dir: Path) -> dict | None:
    for p in sorted(report_dir.glob("*.json")):
        try:
            data = json.loads(p.read_text())
        except json.JSONDecodeError:
            continue
        if isinstance(data, dict) and "resolved_ids" in data:
            return data
    return None


def _apply_failed(rdir: Path, swe_run_id: str, iid: str, fork: bool = False) -> bool:
    """Lite: the marker anywhere in a log. Fork mode (K143): a ``run_instance.log`` line that
    begins with the fork's logging prefix and ``INFO - >>>>> Patch Apply Failed``, which is the
    harness's own line; the same text in the agent's diff or another log doesn't count."""
    if fork:
        line = re.compile(rf"^{FORK_LOG_PREFIX} - INFO - {re.escape(APPLY_PATCH_FAIL)}", re.M)
        logs = rdir.glob(f"logs/run_evaluation/{swe_run_id}/*/{iid}/run_instance.log")
    else:
        logs = rdir.glob(f"logs/run_evaluation/{swe_run_id}/*/{iid}/*.log")
    for log in logs:
        try:
            text = log.read_text(errors="replace")
        except OSError:
            continue
        if (line.search(text) is not None) if fork else (APPLY_PATCH_FAIL in text):
            return True
    return False


def status_for(iid: str, report: dict, rdir: Path, swe_run_id: str, fork: bool = False) -> str:
    def has(key: str) -> bool:
        return iid in set(report.get(key) or [])

    if has("resolved_ids"):
        return "resolved"
    if has("unresolved_ids"):
        return "unresolved"
    if has("empty_patch_ids"):
        return "empty_patch"
    if has("infra_failure_ids"):
        return "grader_infra"
    if has("ambiguous_failure_ids"):
        return "ambiguous"
    if has("error_ids"):
        # Patch-caused failures (does not apply, tests error or time out) are deterministic
        # and count as unresolved; retrying cannot clear them.
        return "apply_failed" if _apply_failed(rdir, swe_run_id, iid, fork) else "error"
    return "missing"


def write_pinned_dataset(args, gdir: Path) -> Path:
    """K108: the rows of --dataset with both image fields replaced by the pinned reference.
    Refuses an empty namespace and a row with no pinned image. Writes grader.json too."""
    if not (args.namespace or "").strip():
        raise SystemExit("--namespace must not be empty: the fork would build environment images")
    pinned = parse_digests(args.image_digests)
    rows = json.loads(Path(args.dataset).read_text())
    for r in rows:
        image = r.get("image_name") or r.get("docker_image")
        if image not in pinned:
            raise SystemExit(f"task {r['instance_id']} has no pinned image for {image}")
        r["image_name"] = r["docker_image"] = pinned[image]
    gdir.mkdir(parents=True, exist_ok=True)
    out = gdir / "dataset.pinned.json"
    out.write_text(json.dumps(rows, indent=1, ensure_ascii=False) + "\n")
    record = {
        "repo": args.grader_repo,
        "commit": args.grader_commit,
        "namespace": args.namespace,
        "swebench_version": package_version("swebench"),
        "docker_version": package_version("docker"),
        "dataset_sha256": hashlib.sha256(out.read_bytes()).hexdigest(),
    }
    (gdir / "grader.json").write_text(json.dumps(record, indent=1) + "\n")
    return out


def _error_block(rdir: Path, swe_run_id: str, iid: str) -> str:
    """K134, K138: the harness's own error, the text after the last line of run_instance.log
    that starts with the fork's logging prefix and ``Error in evaluating model for <iid>:``, up
    to the end of the file. The fork logs ``%(asctime)s - %(levelname)s - %(message)s``; a Git
    diff line or ``git apply`` output can't take that form. The patch sections and
    test_output.txt are text the agent controls, and are never read."""
    start = re.compile(
        rf"^{FORK_LOG_PREFIX} - ERROR - Error in evaluating model for {re.escape(iid)}:",
        re.MULTILINE,
    )
    blocks = []
    for log in sorted(rdir.glob(f"logs/run_evaluation/{swe_run_id}/*/{iid}/run_instance.log")):
        try:
            text = log.read_text(errors="replace")
        except OSError:
            continue
        last = None
        for last in start.finditer(text):  # noqa: B007  (only the last match matters)
            pass
        if last is not None:
            blocks.append(text[last.end() :])
    return "\n".join(blocks)


def fork_status_for(iid: str, report: dict, rdir: Path, swe_run_id: str) -> str:
    """K129, K134: status_for, then a fork ``error`` is told apart from the patch's failure by
    the harness's error block. ``unresolved`` ran its tests and stays; an apply failure (found
    by status_for) keeps precedence; the first signature that matches decides the tier."""
    status = status_for(iid, report, rdir, swe_run_id, fork=True)
    if status != "error":
        return status
    text = _error_block(rdir, swe_run_id, iid)
    for _, tier, pattern in _COMPILED:
        if pattern.search(text):
            return "grader_infra" if tier == TIER_ENVIRONMENT else "ambiguous"
    return status


def package_version(name: str) -> str:
    """The installed version of a package, as the grading environment sees it (K132)."""
    try:
        return importlib.metadata.version(name)
    except importlib.metadata.PackageNotFoundError:
        return ""


def evaluate(args, pred_path: Path, out_dir: Path, run_id: str, ids: list[str]) -> dict | None:
    out_dir.mkdir(parents=True, exist_ok=True)
    if args.image_digests:  # fork mode: a local pinned file; the fork ignores --split
        dataset = ["--dataset_name", str(args.pinned_dataset.resolve()),
                   "--namespace", args.namespace, "--cache_level", args.cache_level]  # fmt: skip
    else:
        dataset = ["--dataset_name", args.dataset, "--split", args.split]
    cmd = [
        sys.executable, "-m", "swebench.harness.run_evaluation", *dataset,
        "--predictions_path", str(pred_path.resolve()),
        "--run_id", run_id,
        "--max_workers", str(args.max_workers),
        "--timeout", str(args.timeout),
        "--report_dir", str(out_dir.resolve()),
        "--instance_ids", *ids,
    ]  # fmt: skip
    subprocess.run(cmd, cwd=out_dir, check=False)
    return read_report(out_dir)


def latest_status(iid: str, attempts: list[tuple[dict, Path, str]], fork: bool = False) -> str:
    """Later attempts (re-grades) override earlier ones for the instances they cover.

    Fork mode (K135): after three attempts in a row that end ``grader_infra`` the instance is
    ``grader_infra_final`` (never re-graded, blank). With at least two attempts after its last
    ``grader_infra``, all ``ambiguous``, it is ``ambiguous_repeated`` (counts as unresolved)."""
    judge = fork_status_for if fork else status_for
    seen = [s for s in (judge(iid, r, d, rid) for r, d, rid in attempts) if s != "missing"]
    if not seen:
        return "missing"
    final = seen[-1]
    if not fork:
        return final
    trailing = 0
    for s in reversed(seen):
        if s != "grader_infra":
            break
        trailing += 1
    if final == "grader_infra" and trailing >= GRADER_INFRA_FINAL_AFTER:
        return "grader_infra_final"
    last_infra = max((k for k, s in enumerate(seen) if s == "grader_infra"), default=-1)
    after = seen[last_infra + 1 :]
    if len(after) >= 2 and all(s == "ambiguous" for s in after):
        return "ambiguous_repeated"
    return final


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--campaign", type=Path, required=True)
    ap.add_argument(
        "--dataset",
        default="SWE-bench/SWE-bench_Lite",
        help="HF dataset name, or the pinned prereg/<name>/dataset.json (preferred)",
    )
    ap.add_argument("--split", default="test")
    ap.add_argument("--image-digests", type=Path, help="fork mode: prereg images.digests")
    ap.add_argument("--namespace", help="fork mode: registry namespace (never empty)")
    ap.add_argument("--cache-level", default="instance", help="fork mode")
    ap.add_argument("--grader-repo", help="fork mode: recorded in grading/grader.json")
    ap.add_argument("--grader-commit", help="fork mode: recorded in grading/grader.json")
    ap.add_argument("--max-workers", type=int, default=8)
    ap.add_argument("--timeout", type=int, default=1800)
    ap.add_argument(
        "--regrade",
        action="store_true",
        help="re-run grader_infra/ambiguous/missing instances in a new retry directory",
    )
    args = ap.parse_args(argv)

    groups = collect(args.campaign)
    if not groups:
        raise SystemExit(f"no finished suite-a runs under {args.campaign}/a")
    gdir = args.campaign / "grading"
    fork = bool(args.image_digests)
    if args.image_digests:
        args.pinned_dataset = write_pinned_dataset(args, gdir)
    gdir.mkdir(parents=True, exist_ok=True)
    rows: list[dict] = []
    for (arm, seed), preds in sorted(groups.items()):
        name = f"{arm}.s{seed}"
        lines = [
            json.dumps(
                {
                    "instance_id": p["instance_id"],
                    "model_name_or_path": f"hcbench.{name}",
                    "model_patch": p["patch"],
                },
                sort_keys=True,
            )
            for p in sorted(preds, key=lambda p: p["instance_id"])
        ]
        blob = "\n".join(lines) + "\n"
        tag = hashlib.sha256(blob.encode()).hexdigest()[:10]
        swe_run_id = f"{args.campaign.name}.{name}.{tag}"
        rdir = gdir / f"{name}.{tag}"
        rdir.mkdir(exist_ok=True)
        pred_path = rdir / "predictions.jsonl"
        pred_path.write_text(blob)
        report = read_report(rdir)
        if report is None:
            print(f"[grade] {name} ({tag}): {len(preds)} predictions", flush=True)
            report = evaluate(args, pred_path, rdir, swe_run_id, [p["instance_id"] for p in preds])
        if report is None:
            print(f"[grade] {name}: no report produced", file=sys.stderr)
            report = {}
        attempts = [(report, rdir, swe_run_id)]
        for retry in sorted(rdir.glob("retry*"), key=lambda d: int(d.name[5:] or 0)):
            rep = read_report(retry)
            if rep is not None:
                attempts.append((rep, retry, f"{swe_run_id}.{retry.name}"))
        pending = [
            p["instance_id"]
            for p in preds
            if latest_status(p["instance_id"], attempts, fork) in REGRADE
        ]
        if pending and args.regrade:
            n = len(attempts)
            retry_dir = rdir / f"retry{n}"
            print(f"[grade] {name}: re-grading {len(pending)} instances (attempt {n})", flush=True)
            rep = evaluate(args, pred_path, retry_dir, f"{swe_run_id}.retry{n}", pending)
            if rep is not None:
                attempts.append((rep, retry_dir, f"{swe_run_id}.retry{n}"))
        tally: dict[str, int] = defaultdict(int)
        for p in preds:
            status = latest_status(p["instance_id"], attempts, fork)
            tally[status] += 1
            rows.append(
                {
                    "run_id": p["run_id"],
                    "patch_sha256": p["patch_sha256"],
                    "resolved": "" if status in BLANK else int(status == "resolved"),
                    "status": status,
                }
            )
        print(f"[grade] {name}: " + ", ".join(f"{k}={v}" for k, v in sorted(tally.items())))

    for run_id in collect_instrumented(args.campaign):  # K145: unresolved, never graded
        rows.append(
            {"run_id": run_id, "patch_sha256": "", "resolved": 0, "status": "instrumentation_error"}
        )
    with (gdir / "outcomes.csv").open("w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["run_id", "patch_sha256", "resolved", "status"])
        w.writeheader()
        w.writerows(rows)
    pending = sum(r["status"] in REGRADE for r in rows)
    print(f"[grade] wrote {gdir / 'outcomes.csv'} ({len(rows)} rows, {pending} need re-grading)")
    for r in rows:
        if r["status"] == "grader_infra_final":  # K140: hard to miss, every time
            print(
                f"WARN: {r['run_id']} is grader_infra_final: unjudged, not re-graded, "
                "left out of pass@1",
                file=sys.stderr,
            )
    if pending:
        print("[grade] re-run with --regrade to re-grade grader_infra/ambiguous/missing instances")
    return 1 if pending else 0


if __name__ == "__main__":
    raise SystemExit(main())
