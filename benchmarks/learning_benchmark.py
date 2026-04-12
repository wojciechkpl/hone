#!/usr/bin/env python3
"""
Cross-session learning benchmark.

Runs the same set of tasks twice:
  Round 1: Fresh agent (no prior session history; memory is built up during the round)
  Round 2: Agent with memory accumulated from Round 1

Measures:
  - Resolve rate improvement (Round 2 vs Round 1)
  - Wall-clock time reduction (fewer exploration calls expected in Round 2)
  - Token savings (not directly observable from the outside, but time is a proxy)

Usage:
    # Minimal smoke test (2 tasks)
    python benchmarks/learning_benchmark.py --limit 2

    # Full run (all 10 mini-benchmark tasks)
    python benchmarks/learning_benchmark.py

    # Keep the shared memory directory across runs
    python benchmarks/learning_benchmark.py --memory-dir /tmp/hone-learning-mem

Environment variables (same as mini_bench.sh):
    OPENAI_API_KEY
    HONE_OPENAI_BASE_URL
    HONE_OPENAI_MODEL   (default: gpt-4o)
    HONE_BINARY         (default: target/release/hone)
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import TypedDict


# ---------------------------------------------------------------------------
# Task registry — mirrors the tasks defined in mini_bench.sh
# ---------------------------------------------------------------------------

class Task(TypedDict):
    name: str
    setup: str        # Shell commands that populate the task work-dir
    prompt: str       # Prompt sent to hone
    check: str        # Shell command that exits 0 on success


TASKS: list[Task] = [
    {
        "name": "fix-typo",
        "setup": textwrap.dedent("""\
            cat > main.py << 'PYEOF'
            def calcualte_average(numbers):
                \"\"\"Calcualte the average of a list of numbers.\"\"\"
                if len(numbers) == 0:
                    return 0
                return sum(numbers) / len(numbers)

            def main():
                result = calcualte_average([1, 2, 3, 4, 5])
                print(f"Average: {result}")

            if __name__ == "__main__":
                main()
            PYEOF
        """),
        "prompt": (
            "Fix the typo in the function name in main.py — "
            "it should be calculate_average not calcualte_average"
        ),
        "check": "grep -q 'def calculate_average' main.py",
    },
    {
        "name": "add-function",
        "setup": textwrap.dedent("""\
            cat > math_utils.py << 'PYEOF'
            def add(a, b):
                return a + b

            def subtract(a, b):
                return a - b
            PYEOF
        """),
        "prompt": (
            "Add a multiply function to math_utils.py that takes two arguments "
            "and returns their product"
        ),
        "check": (
            "grep -q 'def multiply' math_utils.py && "
            "python3 -c \"from math_utils import multiply; assert multiply(3,4)==12\""
        ),
    },
    {
        "name": "fix-bug-off-by-one",
        "setup": textwrap.dedent("""\
            cat > range_check.py << 'PYEOF'
            def is_in_range(value, low, high):
                \"\"\"Check if value is in range [low, high] inclusive.\"\"\"
                return low < value < high

            assert is_in_range(5, 1, 10) == True
            assert is_in_range(1, 1, 10) == True
            assert is_in_range(10, 1, 10) == True
            PYEOF
        """),
        "prompt": (
            "Fix the bug in range_check.py — is_in_range should be inclusive on "
            "both ends (use <= instead of <)"
        ),
        "check": "python3 range_check.py",
    },
    {
        "name": "add-error-handling",
        "setup": textwrap.dedent("""\
            cat > divide.py << 'PYEOF'
            def safe_divide(a, b):
                return a / b
            PYEOF
        """),
        "prompt": (
            "Add error handling to safe_divide in divide.py — return None when "
            "dividing by zero instead of raising an exception"
        ),
        "check": (
            'python3 -c "from divide import safe_divide; '
            'assert safe_divide(10,2)==5.0; assert safe_divide(10,0) is None"'
        ),
    },
    {
        "name": "create-file",
        "setup": "mkdir -p src",
        "prompt": (
            "Create a file src/greeting.py with a function greet(name) that "
            "returns the string 'Hello, {name}!'"
        ),
        "check": (
            'python3 -c "from src.greeting import greet; '
            'assert greet(\\"World\\")==\\"Hello, World!\\""'
        ),
    },
    {
        "name": "rename-variable",
        "setup": textwrap.dedent("""\
            cat > config.py << 'PYEOF'
            x = "localhost"
            y = 5432
            z = "mydb"

            def get_connection_string():
                return f"postgresql://{x}:{y}/{z}"
            PYEOF
        """),
        "prompt": (
            "Rename the variables in config.py to be descriptive: "
            "x->host, y->port, z->database"
        ),
        "check": (
            "grep -q 'host' config.py && grep -q 'port' config.py && "
            "grep -q 'database' config.py && ! grep -q '^x ' config.py"
        ),
    },
    {
        "name": "add-docstring",
        "setup": textwrap.dedent("""\
            cat > utils.py << 'PYEOF'
            def flatten(nested_list):
                result = []
                for item in nested_list:
                    if isinstance(item, list):
                        result.extend(flatten(item))
                    else:
                        result.append(item)
                return result
            PYEOF
        """),
        "prompt": "Add a docstring to the flatten function in utils.py explaining what it does",
        "check": (
            'python3 -c "from utils import flatten; '
            'assert flatten.__doc__ is not None and len(flatten.__doc__) > 10"'
        ),
    },
    {
        "name": "fix-import",
        "setup": textwrap.dedent("""\
            cat > app.py << 'PYEOF'
            from mathutils import calculate_sum

            def main():
                result = calculate_sum([1, 2, 3])
                print(result)
            PYEOF
            cat > math_utils.py << 'PYEOF'
            def calculate_sum(numbers):
                return sum(numbers)
            PYEOF
        """),
        "prompt": "Fix the import in app.py — the module is called math_utils not mathutils",
        "check": "python3 app.py",
    },
    {
        "name": "add-test",
        "setup": textwrap.dedent("""\
            cat > calculator.py << 'PYEOF'
            def add(a, b):
                return a + b

            def subtract(a, b):
                return a - b

            def multiply(a, b):
                return a * b
            PYEOF
        """),
        "prompt": (
            "Create a test file test_calculator.py with pytest tests for all "
            "three functions in calculator.py"
        ),
        "check": (
            "python3 -m pytest test_calculator.py -q 2>/dev/null || "
            "python3 test_calculator.py"
        ),
    },
    {
        "name": "refactor-duplication",
        "setup": textwrap.dedent("""\
            cat > handlers.py << 'PYEOF'
            def handle_get(request):
                if not request.get("auth_token"):
                    return {"error": "unauthorized", "code": 401}
                if not request.get("user_id"):
                    return {"error": "missing user_id", "code": 400}
                return {"data": "get_result", "code": 200}

            def handle_post(request):
                if not request.get("auth_token"):
                    return {"error": "unauthorized", "code": 401}
                if not request.get("user_id"):
                    return {"error": "missing user_id", "code": 400}
                return {"data": "post_result", "code": 200}

            def handle_delete(request):
                if not request.get("auth_token"):
                    return {"error": "unauthorized", "code": 401}
                if not request.get("user_id"):
                    return {"error": "missing user_id", "code": 400}
                return {"data": "delete_result", "code": 200}
            PYEOF
        """),
        "prompt": (
            "Refactor handlers.py to extract the duplicated auth_token and "
            "user_id validation into a shared helper function"
        ),
        "check": textwrap.dedent("""\
            grep -c "auth_token" handlers.py | grep -q "^[1-4]$" && python3 -c "
            from handlers import handle_get, handle_post, handle_delete
            assert handle_get({})['code'] == 401
            assert handle_get({'auth_token': 'x'})['code'] == 400
            assert handle_get({'auth_token': 'x', 'user_id': '1'})['code'] == 200
            "
        """),
    },
]


# ---------------------------------------------------------------------------
# Task runner
# ---------------------------------------------------------------------------

class TaskResult(TypedDict):
    name: str
    round: int
    passed: bool
    elapsed: float
    patch: str


def _init_git_repo(work_dir: Path) -> None:
    subprocess.run(["git", "init", "-q"], cwd=work_dir, check=True, capture_output=True)
    subprocess.run(
        ["git", "config", "user.email", "bench@hone"],
        cwd=work_dir, check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "Hone Bench"],
        cwd=work_dir, check=True, capture_output=True,
    )


def run_task(
    task: Task,
    hone_binary: str,
    model: str,
    round_num: int,
    memory_dir: Path,
    results_dir: Path,
    timeout_secs: int = 120,
) -> TaskResult:
    """Run a single task in a fresh temp directory and return its result."""
    work_dir = Path(tempfile.mkdtemp(prefix=f"hone-learn-r{round_num}-{task['name']}-"))
    task_results_dir = results_dir / f"round{round_num}" / task["name"]
    task_results_dir.mkdir(parents=True, exist_ok=True)

    try:
        _init_git_repo(work_dir)

        # Apply setup script.
        subprocess.run(
            ["bash", "-c", task["setup"]],
            cwd=work_dir,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "add", "-A"],
            cwd=work_dir, check=True, capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-q", "-m", "initial"],
            cwd=work_dir, check=True, capture_output=True,
        )

        env = os.environ.copy()
        env["HONE_OPENAI_MODEL"] = env.get("HONE_OPENAI_MODEL", model)
        # Point hone at the shared memory directory so Round 2 reuses Round 1's memory.
        env["HONE_MEMORY_DIR"] = str(memory_dir)

        start = time.monotonic()
        hone_proc = subprocess.run(
            [hone_binary, "--model", f"openai:{model}", task["prompt"]],
            cwd=work_dir,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_secs,
        )
        elapsed = time.monotonic() - start

        # Capture the patch produced.
        diff_result = subprocess.run(
            ["git", "diff"],
            cwd=work_dir, capture_output=True, text=True, timeout=10,
        )
        patch = diff_result.stdout.strip()

        # Evaluate acceptance criteria.
        check_proc = subprocess.run(
            ["bash", "-c", task["check"]],
            cwd=work_dir,
            capture_output=True,
            timeout=30,
        )
        passed = check_proc.returncode == 0

        # Persist artefacts.
        (task_results_dir / "hone_output.txt").write_text(
            hone_proc.stdout + hone_proc.stderr, errors="replace"
        )
        (task_results_dir / "diff.txt").write_text(patch)
        (task_results_dir / "status.txt").write_text("PASS" if passed else "FAIL")
        (task_results_dir / "duration.txt").write_text(f"{elapsed:.2f}")

    except subprocess.TimeoutExpired:
        elapsed = float(timeout_secs)
        patch = ""
        passed = False
        (task_results_dir / "status.txt").write_text("TIMEOUT")
        (task_results_dir / "duration.txt").write_text(f"{elapsed:.2f}")
    except Exception as exc:
        elapsed = 0.0
        patch = ""
        passed = False
        (task_results_dir / "status.txt").write_text("ERROR")
        (task_results_dir / "duration.txt").write_text("0")
        (task_results_dir / "error.txt").write_text(str(exc))
    finally:
        shutil.rmtree(work_dir, ignore_errors=True)

    return TaskResult(
        name=task["name"],
        round=round_num,
        passed=passed,
        elapsed=elapsed,
        patch=patch,
    )


# ---------------------------------------------------------------------------
# Comparison and reporting
# ---------------------------------------------------------------------------

def compare_rounds(
    round1: list[TaskResult],
    round2: list[TaskResult],
    output_dir: Path,
) -> dict:
    """Compute per-task deltas and write a comparison report."""
    r1_by_name = {r["name"]: r for r in round1}
    r2_by_name = {r["name"]: r for r in round2}

    rows = []
    for name in r1_by_name:
        r1 = r1_by_name[name]
        r2 = r2_by_name.get(name)
        if r2 is None:
            continue

        time_delta = r1["elapsed"] - r2["elapsed"]
        time_pct = (time_delta / r1["elapsed"] * 100) if r1["elapsed"] > 0 else 0.0
        rows.append({
            "task": name,
            "r1_passed": r1["passed"],
            "r2_passed": r2["passed"],
            "r1_elapsed": round(r1["elapsed"], 2),
            "r2_elapsed": round(r2["elapsed"], 2),
            "time_saved_s": round(time_delta, 2),
            "time_saved_pct": round(time_pct, 1),
            "improved": not r1["passed"] and r2["passed"],
            "regressed": r1["passed"] and not r2["passed"],
        })

    r1_rate = sum(1 for r in round1 if r["passed"]) / len(round1) if round1 else 0.0
    r2_rate = sum(1 for r in round2 if r["passed"]) / len(round2) if round2 else 0.0
    r1_time = sum(r["elapsed"] for r in round1)
    r2_time = sum(r["elapsed"] for r in round2)

    summary = {
        "round1_resolve_rate": round(r1_rate * 100, 1),
        "round2_resolve_rate": round(r2_rate * 100, 1),
        "resolve_rate_delta_pp": round((r2_rate - r1_rate) * 100, 1),
        "round1_total_time_s": round(r1_time, 2),
        "round2_total_time_s": round(r2_time, 2),
        "time_saved_s": round(r1_time - r2_time, 2),
        "time_saved_pct": round((r1_time - r2_time) / r1_time * 100, 1) if r1_time > 0 else 0.0,
        "tasks_improved": sum(1 for r in rows if r["improved"]),
        "tasks_regressed": sum(1 for r in rows if r["regressed"]),
        "task_details": rows,
    }

    report_path = output_dir / "comparison.json"
    report_path.write_text(json.dumps(summary, indent=2))

    # Human-readable summary to stdout.
    print("\n" + "=" * 60)
    print("Learning Benchmark Results")
    print("=" * 60)
    print(f"  Round 1 resolve rate : {summary['round1_resolve_rate']}%")
    print(f"  Round 2 resolve rate : {summary['round2_resolve_rate']}%")
    delta = summary["resolve_rate_delta_pp"]
    direction = "+" if delta >= 0 else ""
    print(f"  Delta                : {direction}{delta} pp")
    print(f"  Round 1 total time   : {summary['round1_total_time_s']}s")
    print(f"  Round 2 total time   : {summary['round2_total_time_s']}s")
    print(f"  Time saved           : {summary['time_saved_s']}s ({summary['time_saved_pct']}%)")
    print(f"  Tasks improved       : {summary['tasks_improved']}")
    print(f"  Tasks regressed      : {summary['tasks_regressed']}")
    print(f"\nDetailed report: {report_path}")
    print("=" * 60)

    return summary


# ---------------------------------------------------------------------------
# Entrypoint
# ---------------------------------------------------------------------------

def run_learning_benchmark(
    tasks: list[Task],
    hone_binary: str,
    model: str,
    output_dir: Path,
    memory_dir: Path,
    timeout_secs: int = 120,
) -> dict:
    """Run two rounds and return the comparison summary dict."""
    print(f"\nLearning Benchmark")
    print(f"  Model      : {model}")
    print(f"  Tasks      : {len(tasks)}")
    print(f"  Output     : {output_dir}")
    print(f"  Memory dir : {memory_dir}")
    print()

    round1_results: list[TaskResult] = []
    print("--- Round 1 (fresh memory) ---")
    for i, task in enumerate(tasks):
        print(f"  [{i+1}/{len(tasks)}] {task['name']}...", end=" ", flush=True)
        result = run_task(task, hone_binary, model, 1, memory_dir, output_dir, timeout_secs)
        round1_results.append(result)
        status = "PASS" if result["passed"] else "FAIL"
        print(f"{status} ({result['elapsed']:.1f}s)")

    print()
    print("--- Round 2 (with accumulated memory) ---")
    round2_results: list[TaskResult] = []
    for i, task in enumerate(tasks):
        print(f"  [{i+1}/{len(tasks)}] {task['name']}...", end=" ", flush=True)
        result = run_task(task, hone_binary, model, 2, memory_dir, output_dir, timeout_secs)
        round2_results.append(result)
        status = "PASS" if result["passed"] else "FAIL"
        print(f"{status} ({result['elapsed']:.1f}s)")

    # Persist raw results.
    (output_dir / "round1_results.json").write_text(json.dumps(round1_results, indent=2))
    (output_dir / "round2_results.json").write_text(json.dumps(round2_results, indent=2))

    return compare_rounds(round1_results, round2_results, output_dir)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Cross-session learning benchmark for Hone",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--limit", type=int, default=None,
        help="Run only the first N tasks (default: all 10)",
    )
    parser.add_argument(
        "--output-dir", default=None,
        help="Output directory (default: benchmarks/results/learning-<date>)",
    )
    parser.add_argument(
        "--memory-dir", default=None,
        help="Shared memory directory for both rounds. "
             "Created fresh by default; pass an existing dir to reuse prior memory.",
    )
    parser.add_argument(
        "--model", default=os.environ.get("HONE_OPENAI_MODEL", "gpt-4o"),
        help="Model name (default: gpt-4o or HONE_OPENAI_MODEL env var)",
    )
    parser.add_argument(
        "--timeout", type=int, default=120,
        help="Per-task timeout in seconds (default: 120)",
    )
    args = parser.parse_args()

    project_root = Path(__file__).resolve().parent.parent
    hone_binary = os.environ.get(
        "HONE_BINARY", str(project_root / "target" / "release" / "hone")
    )

    if not Path(hone_binary).exists():
        print(f"ERROR: hone binary not found at {hone_binary}", file=sys.stderr)
        print("Run: cargo build --release", file=sys.stderr)
        sys.exit(1)

    if not os.environ.get("OPENAI_API_KEY"):
        print("ERROR: OPENAI_API_KEY not set", file=sys.stderr)
        sys.exit(1)

    date_str = datetime.now(tz=timezone.utc).strftime("%Y-%m-%d-%H%M")
    output_dir = (
        Path(args.output_dir)
        if args.output_dir
        else project_root / "benchmarks" / "results" / f"learning-{date_str}"
    )
    output_dir.mkdir(parents=True, exist_ok=True)

    # Memory directory: persistent across both rounds so Round 2 benefits from Round 1.
    memory_dir: Path
    if args.memory_dir:
        memory_dir = Path(args.memory_dir)
        memory_dir.mkdir(parents=True, exist_ok=True)
    else:
        memory_dir = output_dir / "hone_memory"
        memory_dir.mkdir(parents=True, exist_ok=True)

    tasks = TASKS[: args.limit] if args.limit else TASKS

    summary = run_learning_benchmark(
        tasks=tasks,
        hone_binary=hone_binary,
        model=args.model,
        output_dir=output_dir,
        memory_dir=memory_dir,
        timeout_secs=args.timeout,
    )

    # Non-zero exit if learning degraded performance.
    if summary["resolve_rate_delta_pp"] < 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
