#!/usr/bin/env python3
"""
persona_eval.py — Lightweight evaluation harness for Hone's built-in agent personas.

Usage:
    python benchmarks/persona_eval.py [--url URL] [--model MODEL] [--out-dir DIR]

Defaults:
    --url     http://localhost:8080    (hone-server or hone CLI in server mode)
    --model   (uses server default)
    --out-dir benchmarks/results

Each scenario defines:
    - persona:            name of the built-in persona / system prompt
    - prompt:             user message sent to the agent
    - expected_keywords:  all of these must appear in the response (case-insensitive)
    - forbidden_keywords: none of these may appear (optional)

Exit codes:
    0  all scenarios passed
    1  one or more scenarios failed
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Optional

try:
    import requests
except ImportError:
    print("ERROR: 'requests' is not installed. Run: pip install requests", file=sys.stderr)
    sys.exit(2)

# ---------------------------------------------------------------------------
# Scenario definitions
# ---------------------------------------------------------------------------

@dataclass
class Scenario:
    persona: str
    name: str
    prompt: str
    expected_keywords: list[str]
    forbidden_keywords: list[str] = field(default_factory=list)


SCENARIOS: list[Scenario] = [
    # ── debugger ────────────────────────────────────────────────────────────
    Scenario(
        persona="debugger",
        name="asks for stack trace",
        prompt="My Python script is crashing. How do I fix it?",
        expected_keywords=["stack trace", "error message", "traceback"],
    ),
    Scenario(
        persona="debugger",
        name="asks for reproduction steps",
        prompt="Something went wrong in production last night.",
        expected_keywords=["reproduce", "steps", "logs"],
    ),
    Scenario(
        persona="debugger",
        name="does not hallucinate fixes without info",
        prompt="My app is broken.",
        expected_keywords=["error", "more information"],
        forbidden_keywords=["the fix is", "you should replace"],
    ),

    # ── security-auditor ────────────────────────────────────────────────────
    Scenario(
        persona="security-auditor",
        name="mentions OWASP when reviewing auth code",
        prompt=(
            "Review this authentication handler:\n"
            "def login(username, password):\n"
            "    query = f\"SELECT * FROM users WHERE name='{username}'\"\n"
            "    return db.execute(query)"
        ),
        expected_keywords=["owasp", "sql injection", "parameterized"],
    ),
    Scenario(
        persona="security-auditor",
        name="flags XSS in template rendering",
        prompt=(
            "Review this template:\n"
            "<p>Welcome, {{ user_input }}</p>"
        ),
        expected_keywords=["xss", "cross-site scripting", "escape"],
    ),
    Scenario(
        persona="security-auditor",
        name="warns about secrets in env",
        prompt="Is it safe to store API keys in environment variables?",
        expected_keywords=["secret", "vault", "rotation"],
    ),
    Scenario(
        persona="security-auditor",
        name="recommends least privilege",
        prompt="What database permissions should my app user have?",
        expected_keywords=["least privilege", "principle", "grant"],
    ),

    # ── rust-expert ─────────────────────────────────────────────────────────
    Scenario(
        persona="rust-expert",
        name="explains ownership",
        prompt="Why does Rust have ownership?",
        expected_keywords=["ownership", "borrow", "memory safety"],
    ),
    Scenario(
        persona="rust-expert",
        name="uses idiomatic derives",
        prompt="How do I make a struct printable and cloneable in Rust?",
        expected_keywords=["#[derive", "Debug", "Clone"],
    ),
    Scenario(
        persona="rust-expert",
        name="recommends zero-cost abstractions",
        prompt="Should I use iterators or manual loops in Rust?",
        expected_keywords=["zero-cost", "iterator", "abstraction"],
    ),

    # ── python-expert ───────────────────────────────────────────────────────
    Scenario(
        persona="python-expert",
        name="recommends type hints",
        prompt="How should I write a function that takes a list and returns a dict?",
        expected_keywords=["type hint", "list", "dict"],
    ),
    Scenario(
        persona="python-expert",
        name="mentions PEP 8",
        prompt="What are Python naming conventions?",
        expected_keywords=["pep 8", "snake_case", "convention"],
    ),
    Scenario(
        persona="python-expert",
        name="recommends pytest",
        prompt="How should I write tests in Python?",
        expected_keywords=["pytest", "assert", "test"],
    ),

    # ── ai-researcher ────────────────────────────────────────────────────────
    Scenario(
        persona="ai-researcher",
        name="cites evaluation metrics",
        prompt="How do you evaluate a language model?",
        expected_keywords=["perplexity", "benchmark", "metric"],
    ),
    Scenario(
        persona="ai-researcher",
        name="discusses training methodology",
        prompt="Explain RLHF.",
        expected_keywords=["reinforcement learning", "human feedback", "reward model"],
    ),
    Scenario(
        persona="ai-researcher",
        name="references literature",
        prompt="What is attention in transformers?",
        expected_keywords=["attention", "transformer", "paper"],
    ),

    # ── code-reviewer ────────────────────────────────────────────────────────
    Scenario(
        persona="code-reviewer",
        name="mentions tests",
        prompt="Review this PR that adds a new API endpoint.",
        expected_keywords=["test", "coverage", "edge case"],
    ),
    Scenario(
        persona="code-reviewer",
        name="comments on naming",
        prompt="Review: def x(a, b): return a + b",
        expected_keywords=["naming", "descriptive", "readable"],
    ),
    Scenario(
        persona="code-reviewer",
        name="flags complexity",
        prompt="Review a 200-line function with nested loops.",
        expected_keywords=["complexity", "refactor", "extract"],
    ),
    Scenario(
        persona="code-reviewer",
        name="checks for error handling",
        prompt="Review: result = open('file.txt').read()",
        expected_keywords=["error", "exception", "handle"],
    ),

    # ── postgresql-expert ────────────────────────────────────────────────────
    Scenario(
        persona="postgresql-expert",
        name="suggests index",
        prompt="My query on a 10M row table is slow: SELECT * FROM orders WHERE user_id = 42",
        expected_keywords=["index", "CREATE INDEX", "query plan"],
    ),
    Scenario(
        persona="postgresql-expert",
        name="recommends EXPLAIN ANALYZE",
        prompt="How do I diagnose a slow PostgreSQL query?",
        expected_keywords=["EXPLAIN", "ANALYZE", "query plan"],
    ),
    Scenario(
        persona="postgresql-expert",
        name="discusses vacuuming",
        prompt="Why is my PostgreSQL table getting bloated over time?",
        expected_keywords=["VACUUM", "dead tuples", "autovacuum"],
    ),

    # ── test-architect ───────────────────────────────────────────────────────
    Scenario(
        persona="test-architect",
        name="distinguishes test levels",
        prompt="What types of tests should my project have?",
        expected_keywords=["unit", "integration", "end-to-end"],
    ),
    Scenario(
        persona="test-architect",
        name="mentions coverage",
        prompt="How do I know if my tests are sufficient?",
        expected_keywords=["coverage", "branch", "mutation"],
    ),
    Scenario(
        persona="test-architect",
        name="recommends test isolation",
        prompt="My tests are flaky. What should I check?",
        expected_keywords=["isolation", "dependency", "mock"],
    ),

    # ── fullstack-architect ───────────────────────────────────────────────────
    Scenario(
        persona="fullstack-architect",
        name="separates frontend and backend concerns",
        prompt="Design a user profile feature.",
        expected_keywords=["api", "frontend", "backend"],
    ),
    Scenario(
        persona="fullstack-architect",
        name="mentions API contract",
        prompt="How should frontend and backend teams collaborate?",
        expected_keywords=["api contract", "schema", "openapi"],
    ),
    Scenario(
        persona="fullstack-architect",
        name="considers deployment",
        prompt="How do I deploy a React + FastAPI app?",
        expected_keywords=["deploy", "docker", "container"],
    ),

    # ── ml-recommendations-expert ─────────────────────────────────────────────
    Scenario(
        persona="ml-recommendations-expert",
        name="mentions collaborative filtering",
        prompt="How do I build a product recommendation system?",
        expected_keywords=["collaborative filtering", "user-item", "matrix"],
    ),
    Scenario(
        persona="ml-recommendations-expert",
        name="addresses cold start",
        prompt="What is the cold start problem in recommender systems?",
        expected_keywords=["cold start", "new user", "content-based"],
    ),
    Scenario(
        persona="ml-recommendations-expert",
        name="mentions evaluation metrics",
        prompt="How do I evaluate my recommendation engine?",
        expected_keywords=["precision", "recall", "ndcg"],
    ),

    # ── general-purpose ───────────────────────────────────────────────────────
    Scenario(
        persona="general-purpose",
        name="responds helpfully to simple question",
        prompt="What is the capital of France?",
        expected_keywords=["paris"],
    ),
    Scenario(
        persona="general-purpose",
        name="does not use heavy domain jargon unprompted",
        prompt="Explain what a database is.",
        expected_keywords=["data", "store", "table"],
        forbidden_keywords=["MVCC", "WAL", "write-ahead log"],
    ),

    # ── quant-advisor ─────────────────────────────────────────────────────────
    Scenario(
        persona="quant-advisor",
        name="mentions Sharpe ratio",
        prompt="How do I measure the risk-adjusted return of my portfolio?",
        expected_keywords=["sharpe", "ratio", "risk"],
    ),
    Scenario(
        persona="quant-advisor",
        name="discusses drawdown",
        prompt="What is maximum drawdown?",
        expected_keywords=["drawdown", "peak", "trough"],
    ),
    Scenario(
        persona="quant-advisor",
        name="stresses backtesting caveats",
        prompt="I backtested my strategy and got 40% annual returns. Is it good?",
        expected_keywords=["backtest", "overfitting", "out-of-sample"],
    ),
]

# ---------------------------------------------------------------------------
# Evaluation runner
# ---------------------------------------------------------------------------

@dataclass
class ScenarioResult:
    persona: str
    name: str
    passed: bool
    missing_keywords: list[str]
    found_forbidden: list[str]
    response_snippet: str


def call_agent(
    base_url: str,
    persona: str,
    prompt: str,
    model: Optional[str],
    timeout: int = 60,
) -> str:
    """Call the Hone agent endpoint and return the response text."""
    payload: dict = {
        "persona": persona,
        "messages": [{"role": "user", "content": prompt}],
    }
    if model:
        payload["model"] = model

    resp = requests.post(
        f"{base_url}/v1/agent/run",
        json=payload,
        timeout=timeout,
    )
    resp.raise_for_status()
    data = resp.json()
    # Support both {"response": "..."} and OpenAI-compat {"choices": [...]}
    if "response" in data:
        return data["response"]
    if "choices" in data and data["choices"]:
        return data["choices"][0].get("message", {}).get("content", "")
    return json.dumps(data)


def evaluate_scenario(
    scenario: Scenario,
    base_url: str,
    model: Optional[str],
) -> ScenarioResult:
    try:
        response = call_agent(base_url, scenario.persona, scenario.prompt, model)
    except Exception as exc:  # noqa: BLE001
        return ScenarioResult(
            persona=scenario.persona,
            name=scenario.name,
            passed=False,
            missing_keywords=[f"(API error: {exc})"],
            found_forbidden=[],
            response_snippet="",
        )

    lower = response.lower()

    missing = [kw for kw in scenario.expected_keywords if kw.lower() not in lower]
    forbidden = [kw for kw in scenario.forbidden_keywords if kw.lower() in lower]

    return ScenarioResult(
        persona=scenario.persona,
        name=scenario.name,
        passed=(not missing and not forbidden),
        missing_keywords=missing,
        found_forbidden=forbidden,
        response_snippet=response[:200].replace("\n", " "),
    )


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------

def print_summary(results: list[ScenarioResult]) -> None:
    total = len(results)
    passed = sum(1 for r in results if r.passed)
    failed = total - passed

    col_persona = max(len(r.persona) for r in results) + 2
    col_name = max(len(r.name) for r in results) + 2

    header = f"{'PERSONA':<{col_persona}} {'SCENARIO':<{col_name}} STATUS  DETAILS"
    print()
    print(header)
    print("-" * len(header))

    for r in results:
        status = "PASS" if r.passed else "FAIL"
        details = ""
        if r.missing_keywords:
            details += f"missing: {r.missing_keywords}"
        if r.found_forbidden:
            details += f"  forbidden present: {r.found_forbidden}"
        print(f"{r.persona:<{col_persona}} {r.name:<{col_name}} {status:<8} {details}")

    print()
    print(f"Results: {passed}/{total} passed", end="")
    if failed:
        print(f"  ({failed} failed)")
    else:
        print("  (all green)")
    print()


def write_json(results: list[ScenarioResult], out_dir: Path) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    today = date.today().isoformat()
    out_path = out_dir / f"persona_eval_{today}.json"

    payload = {
        "date": today,
        "total": len(results),
        "passed": sum(1 for r in results if r.passed),
        "failed": sum(1 for r in results if not r.passed),
        "scenarios": [
            {
                "persona": r.persona,
                "name": r.name,
                "passed": r.passed,
                "missing_keywords": r.missing_keywords,
                "found_forbidden": r.found_forbidden,
                "response_snippet": r.response_snippet,
            }
            for r in results
        ],
    }
    out_path.write_text(json.dumps(payload, indent=2))
    return out_path


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument(
        "--url",
        default=os.environ.get("HONE_URL", "http://localhost:8080"),
        help="Base URL of the Hone server (default: http://localhost:8080)",
    )
    p.add_argument("--model", default=None, help="Model ID override")
    p.add_argument(
        "--out-dir",
        default="benchmarks/results",
        help="Directory for JSON result files (default: benchmarks/results)",
    )
    p.add_argument(
        "--persona",
        default=None,
        help="Run only scenarios for this persona (e.g. debugger)",
    )
    p.add_argument(
        "--dry-run",
        action="store_true",
        help="List scenarios without calling the API",
    )
    return p.parse_args()


def main() -> None:
    args = parse_args()
    out_dir = Path(args.out_dir)

    scenarios = SCENARIOS
    if args.persona:
        scenarios = [s for s in SCENARIOS if s.persona == args.persona]
        if not scenarios:
            print(f"No scenarios found for persona '{args.persona}'.", file=sys.stderr)
            sys.exit(1)

    if args.dry_run:
        print(f"Dry run — {len(scenarios)} scenario(s) would be evaluated:")
        for s in scenarios:
            print(f"  [{s.persona}] {s.name}")
        sys.exit(0)

    print(f"Evaluating {len(scenarios)} scenario(s) against {args.url} ...")
    results: list[ScenarioResult] = []

    for scenario in scenarios:
        print(f"  [{scenario.persona}] {scenario.name} ...", end=" ", flush=True)
        result = evaluate_scenario(scenario, args.url, args.model)
        print("PASS" if result.passed else "FAIL")
        results.append(result)

    print_summary(results)

    out_path = write_json(results, out_dir)
    print(f"Results written to {out_path}")

    failed_count = sum(1 for r in results if not r.passed)
    sys.exit(1 if failed_count else 0)


if __name__ == "__main__":
    main()
