#!/usr/bin/env python3
"""
SWE-bench evaluation harness — applies patches and runs tests in Docker.

Requires Docker to be running. Each task is evaluated in an isolated
container with the repo at the correct commit.

Usage:
    python benchmarks/swebench_evaluate.py \
        --predictions benchmarks/results/swe-lite-2026-04-09/predictions.jsonl

    # Evaluate specific instances
    python benchmarks/swebench_evaluate.py \
        --predictions predictions.jsonl \
        --instance-ids astropy__astropy-12907

    # Use more workers for faster evaluation
    python benchmarks/swebench_evaluate.py \
        --predictions predictions.jsonl \
        --max-workers 4
"""

import argparse
import json
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description="Evaluate SWE-bench predictions")
    parser.add_argument("--predictions", required=True,
                        help="Path to predictions.jsonl from inference step")
    parser.add_argument("--dataset", default="princeton-nlp/SWE-bench_Lite",
                        help="HuggingFace dataset name")
    parser.add_argument("--split", default="test")
    parser.add_argument("--instance-ids", nargs="+", default=None)
    parser.add_argument("--max-workers", type=int, default=2)
    parser.add_argument("--timeout", type=int, default=900,
                        help="Per-instance evaluation timeout (seconds)")
    parser.add_argument("--run-id", default="hone-eval",
                        help="Run identifier for the evaluation")
    parser.add_argument("--report-dir", default=None,
                        help="Directory for evaluation reports")
    args = parser.parse_args()

    predictions_path = Path(args.predictions)
    if not predictions_path.exists():
        print(f"ERROR: {predictions_path} not found", file=sys.stderr)
        sys.exit(1)

    # Read instance IDs from predictions
    instance_ids = []
    if args.instance_ids:
        instance_ids = args.instance_ids
    else:
        with open(predictions_path) as f:
            for line in f:
                obj = json.loads(line)
                if obj.get("model_patch"):  # skip empty patches
                    instance_ids.append(obj["instance_id"])

    if not instance_ids:
        print("No predictions with patches found.", file=sys.stderr)
        sys.exit(1)

    report_dir = args.report_dir or str(predictions_path.parent)

    print(f"Evaluating {len(instance_ids)} predictions")
    print(f"  Dataset: {args.dataset}")
    print(f"  Workers: {args.max_workers}")
    print(f"  Reports: {report_dir}")
    print()

    # Import and run SWE-bench evaluation
    from swebench.harness.run_evaluation import main as run_eval

    run_eval(
        dataset_name=args.dataset,
        split=args.split,
        instance_ids=instance_ids,
        predictions_path=str(predictions_path),
        max_workers=args.max_workers,
        force_rebuild=False,
        cache_level="env",
        clean=False,
        open_file_limit=4096,
        run_id=args.run_id,
        timeout=args.timeout,
        namespace=None,
        rewrite_reports=True,
        modal=False,
        report_dir=report_dir,
    )

    # Parse results
    report_file = Path(report_dir) / f"{args.run_id}.json"
    if report_file.exists():
        with open(report_file) as f:
            report = json.load(f)

        resolved = sum(1 for r in report.get("resolved", []))
        total = len(instance_ids)
        print(f"\n{'='*50}")
        print(f"  Resolved: {resolved}/{total} ({resolved*100/total:.1f}%)")
        print(f"  Report: {report_file}")
        print(f"{'='*50}")
    else:
        print(f"\nEvaluation complete. Check {report_dir} for reports.")


if __name__ == "__main__":
    main()
