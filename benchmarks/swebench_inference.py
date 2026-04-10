#!/usr/bin/env python3
"""
SWE-bench inference harness for Hone.

Runs Hone against SWE-bench tasks and collects model_patch predictions
in the format expected by swebench.harness.run_evaluation.

Usage:
    # Dry run (5 tasks)
    python benchmarks/swebench_inference.py --limit 5

    # Full SWE-bench Lite (300 tasks)
    python benchmarks/swebench_inference.py --dataset princeton-nlp/SWE-bench_Lite

    # SWE-bench Verified (500 tasks)
    python benchmarks/swebench_inference.py --dataset princeton-nlp/SWE-bench_Verified

Environment variables:
    OPENAI_API_KEY          API key for the model provider
    HONE_OPENAI_BASE_URL    Custom endpoint (DeepSeek, OpenRouter, etc.)
    HONE_OPENAI_MODEL       Model name (default: deepseek/deepseek-chat)
    HONE_BINARY             Path to hone binary (default: target/release/hone)
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path

def load_dataset_tasks(dataset_name: str, split: str, limit: int | None, instance_ids: list[str] | None):
    """Load SWE-bench tasks from HuggingFace."""
    from datasets import load_dataset
    ds = load_dataset(dataset_name, split=split)

    if instance_ids:
        ds = ds.filter(lambda x: x["instance_id"] in instance_ids)

    tasks = list(ds)
    if limit:
        tasks = tasks[:limit]

    return tasks


def clone_repo(repo: str, base_commit: str, work_dir: Path) -> Path:
    """Clone a repo at a specific commit into work_dir."""
    repo_url = f"https://github.com/{repo}.git"
    repo_dir = work_dir / repo.replace("/", "__")

    if repo_dir.exists():
        # Reset to the base commit
        subprocess.run(
            ["git", "checkout", "-f", base_commit],
            cwd=repo_dir, capture_output=True, timeout=60
        )
        subprocess.run(
            ["git", "clean", "-fdx"],
            cwd=repo_dir, capture_output=True, timeout=60
        )
    else:
        subprocess.run(
            ["git", "clone", "--quiet", repo_url, str(repo_dir)],
            capture_output=True, timeout=300
        )
        subprocess.run(
            ["git", "checkout", "-f", base_commit],
            cwd=repo_dir, capture_output=True, timeout=60
        )

    return repo_dir


STRUCTURED_PROMPT_TEMPLATE = """\
You are an expert software engineer fixing a bug in a real open-source repository.

## Issue
{problem_statement}

## Relevant Files (pre-localized)
The following files are most likely to contain the bug. Read them first before exploring further.

{file_context}

## Workflow
Follow these phases in order:

### Phase 1: Understand
- Read the issue carefully. Identify what behavior is expected vs actual.
- Note any file paths, function names, or error messages mentioned.

### Phase 2: Localize
- Use grep/glob to find the relevant files and functions.
- Read the specific code sections that are likely buggy.
- Do NOT edit any files yet.

### Phase 3: Fix
- Make the minimal edit needed to fix the issue.
- Only edit files directly related to the bug.
- Do NOT modify test files.
- Do NOT add new tests.

### Phase 4: Verify
- After editing, re-read the changed file to confirm the fix looks correct.
- Check for syntax errors or obvious mistakes.

## Rules
- Make minimal changes. Do not refactor unrelated code.
- If you cannot find the bug after reading 5 files, focus on the most likely candidate and attempt a fix.
- Do not explain your changes at the end — just make the fix.
"""

LOCALIZATION_PROMPT_TEMPLATE = """\
Given this repository file tree and issue description, list the top 5 most likely files \
that need to be fixed to resolve the issue. Return ONLY file paths, one per line, \
with no explanation or commentary.

## Issue
{problem_statement}

## Repository File Tree
{file_tree}
"""


def get_file_tree(repo_dir: Path) -> str:
    """Return a filtered list of Python source files in the repo (max 200)."""
    result = subprocess.run(
        ["find", ".", "-name", "*.py", "-not", "-path", "./.git/*"],
        cwd=repo_dir,
        capture_output=True,
        text=True,
        timeout=30,
    )
    lines = [l.strip() for l in result.stdout.splitlines() if l.strip()]
    lines.sort()
    return "\n".join(lines[:200])


def localize_files(
    repo_dir: Path,
    problem_statement: str,
    hone_binary: str,
    model: str,
    env: dict,
    timeout_secs: int = 60,
) -> list[str]:
    """
    Stage 1 localization: ask the model for the top 5 likely buggy files.

    Runs hone with a short, focused prompt and parses the response as a list of
    file paths.  Returns an empty list on any failure so the caller can
    gracefully degrade.
    """
    file_tree = get_file_tree(repo_dir)
    localization_prompt = LOCALIZATION_PROMPT_TEMPLATE.format(
        problem_statement=problem_statement,
        file_tree=file_tree,
    )

    try:
        result = subprocess.run(
            [hone_binary, "--model", f"openai:{model}", localization_prompt],
            cwd=repo_dir,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_secs,
        )
    except subprocess.TimeoutExpired:
        return []

    if result.returncode != 0:
        return []

    # Parse the model output: extract lines that look like file paths.
    candidates: list[str] = []
    for line in result.stdout.splitlines():
        line = line.strip().lstrip("- ").strip()
        if not line or line.startswith("#"):
            continue
        # Normalise: strip leading "./"
        if line.startswith("./"):
            line = line[2:]
        # Only accept paths that exist in the repo.
        if (repo_dir / line).exists():
            candidates.append(line)
        if len(candidates) >= 5:
            break

    return candidates


def read_file_truncated(path: Path, max_lines: int = 200) -> str:
    """Read a file and return up to max_lines lines with a truncation notice."""
    try:
        lines = path.read_text(errors="replace").splitlines()
    except OSError:
        return f"(could not read {path})"
    if len(lines) <= max_lines:
        return "\n".join(lines)
    return "\n".join(lines[:max_lines]) + f"\n... (truncated at {max_lines} lines)"


def build_file_context(repo_dir: Path, localized_paths: list[str]) -> str:
    """Render the pre-read file contents for inclusion in the agent prompt."""
    if not localized_paths:
        return "(no files pre-localized)"
    parts: list[str] = []
    for rel_path in localized_paths:
        abs_path = repo_dir / rel_path
        content = read_file_truncated(abs_path)
        parts.append(f"### {rel_path}\n```python\n{content}\n```")
    return "\n\n".join(parts)


def run_hone(repo_dir: Path, problem_statement: str, hone_binary: str, model: str,
             timeout_secs: int = 300) -> tuple[str, float, bool]:
    """Run Hone on a task and return (patch, elapsed_secs, success)."""
    env = os.environ.copy()
    env["HONE_OPENAI_MODEL"] = env.get("HONE_OPENAI_MODEL", model)

    # ------------------------------------------------------------------
    # Change 2: Two-stage localization
    # Run a cheap localization call first to identify the top 5 candidate
    # files, then pre-read their content into the main agent prompt.
    # ------------------------------------------------------------------
    localized_paths = localize_files(
        repo_dir, problem_statement, hone_binary, model, env,
        timeout_secs=min(60, timeout_secs // 5),
    )
    file_context = build_file_context(repo_dir, localized_paths)

    # ------------------------------------------------------------------
    # Change 1: Structured 4-phase SWE-bench prompt
    # ------------------------------------------------------------------
    prompt = STRUCTURED_PROMPT_TEMPLATE.format(
        problem_statement=problem_statement,
        file_context=file_context,
    )

    start = time.time()
    try:
        result = subprocess.run(
            [hone_binary, "--model", f"openai:{model}", prompt],
            cwd=repo_dir,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_secs,
        )
        elapsed = time.time() - start
        success = result.returncode == 0
    except subprocess.TimeoutExpired:
        elapsed = timeout_secs
        success = False

    # Generate patch from git diff
    diff_result = subprocess.run(
        ["git", "diff"],
        cwd=repo_dir, capture_output=True, text=True, timeout=30
    )
    patch = diff_result.stdout.strip()

    return patch, elapsed, success


def main():
    parser = argparse.ArgumentParser(description="SWE-bench inference with Hone")
    parser.add_argument("--dataset", default="princeton-nlp/SWE-bench_Lite",
                        help="HuggingFace dataset name")
    parser.add_argument("--split", default="test")
    parser.add_argument("--limit", type=int, default=None,
                        help="Max tasks to run (for testing)")
    parser.add_argument("--instance-ids", nargs="+", default=None,
                        help="Specific instance IDs to run")
    parser.add_argument("--output-dir", default=None,
                        help="Output directory (default: benchmarks/results/swebench-<date>)")
    parser.add_argument("--model", default="deepseek/deepseek-chat")
    parser.add_argument("--timeout", type=int, default=300,
                        help="Per-task timeout in seconds")
    parser.add_argument("--resume", action="store_true",
                        help="Skip tasks that already have predictions")
    args = parser.parse_args()

    # Resolve paths
    project_root = Path(__file__).parent.parent
    hone_binary = os.environ.get("HONE_BINARY", str(project_root / "target" / "release" / "hone"))

    if not Path(hone_binary).exists():
        print(f"ERROR: hone binary not found at {hone_binary}", file=sys.stderr)
        print("Run: cargo build --release", file=sys.stderr)
        sys.exit(1)

    if not os.environ.get("OPENAI_API_KEY"):
        print("ERROR: OPENAI_API_KEY not set", file=sys.stderr)
        sys.exit(1)

    # Output directory
    date_str = datetime.now().strftime("%Y-%m-%d-%H%M")
    dataset_short = args.dataset.split("/")[-1].lower().replace("swe-bench_", "swe-")
    output_dir = Path(args.output_dir) if args.output_dir else (
        project_root / "benchmarks" / "results" / f"{dataset_short}-{date_str}"
    )
    output_dir.mkdir(parents=True, exist_ok=True)

    predictions_file = output_dir / "predictions.jsonl"
    metadata_file = output_dir / "metadata.json"

    # Load tasks
    print(f"Loading {args.dataset} ({args.split})...")
    tasks = load_dataset_tasks(args.dataset, args.split, args.limit, args.instance_ids)
    print(f"  {len(tasks)} tasks to process")

    # Load existing predictions for resume
    existing = set()
    if args.resume and predictions_file.exists():
        with open(predictions_file) as f:
            for line in f:
                obj = json.loads(line)
                existing.add(obj["instance_id"])
        print(f"  Resuming: {len(existing)} already completed")

    # Write metadata
    metadata = {
        "dataset": args.dataset,
        "split": args.split,
        "model": args.model,
        "base_url": os.environ.get("HONE_OPENAI_BASE_URL", "default"),
        "hone_binary": hone_binary,
        "date": datetime.now().isoformat(),
        "total_tasks": len(tasks),
        "timeout_per_task": args.timeout,
    }
    with open(metadata_file, "w") as f:
        json.dump(metadata, f, indent=2)

    # Process tasks
    work_dir = Path(tempfile.mkdtemp(prefix="hone-swebench-"))
    passed = 0
    failed = 0
    skipped = 0
    total_cost = 0.0

    print(f"\nRunning inference (output: {output_dir})")
    print(f"Work dir: {work_dir}\n")

    for i, task in enumerate(tasks):
        instance_id = task["instance_id"]

        if instance_id in existing:
            skipped += 1
            continue

        repo = task["repo"]
        base_commit = task["base_commit"]
        problem = task["problem_statement"]

        print(f"[{i+1}/{len(tasks)}] {instance_id}...", end=" ", flush=True)

        try:
            repo_dir = clone_repo(repo, base_commit, work_dir)
            patch, elapsed, success = run_hone(
                repo_dir, problem, hone_binary, args.model, args.timeout
            )

            prediction = {
                "instance_id": instance_id,
                "model_patch": patch,
                "model_name_or_path": f"hone+{args.model}",
            }

            with open(predictions_file, "a") as f:
                f.write(json.dumps(prediction) + "\n")

            if patch:
                passed += 1
                print(f"patch ({len(patch)} chars, {elapsed:.0f}s)")
            else:
                failed += 1
                print(f"no patch ({elapsed:.0f}s)")

        except Exception as e:
            failed += 1
            print(f"ERROR: {e}")

            # Write empty prediction so evaluation doesn't skip it
            prediction = {
                "instance_id": instance_id,
                "model_patch": "",
                "model_name_or_path": f"hone+{args.model}",
            }
            with open(predictions_file, "a") as f:
                f.write(json.dumps(prediction) + "\n")

    # Summary
    total = passed + failed
    print(f"\n{'='*50}")
    print(f"Inference complete: {passed} patches, {failed} empty, {skipped} skipped")
    print(f"Predictions: {predictions_file}")
    print(f"{'='*50}")

    # Update metadata
    metadata["completed"] = total
    metadata["patches_generated"] = passed
    metadata["empty_patches"] = failed
    metadata["skipped"] = skipped
    with open(metadata_file, "w") as f:
        json.dump(metadata, f, indent=2)

    print(f"\nTo evaluate:")
    print(f"  python benchmarks/swebench_evaluate.py --predictions {predictions_file}")


if __name__ == "__main__":
    main()
