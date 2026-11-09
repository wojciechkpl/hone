"""Flatten one run (meta + proxy summary + harness summary + outcome) into a CSV row."""

from __future__ import annotations

from collections.abc import Mapping

COLUMNS: tuple[str, ...] = (
    "run_id",
    "suite",
    "arm",
    "seed",
    "task_id",
    "resolved",
    "exit_code",
    "timed_out",
    "wall_s",
    "requests",
    "prompt_tokens",
    "completion_tokens",
    "total_tokens",
    "tool_calls",
    "read_calls",
    "edit_calls",
    "exec_calls",
    "read_edit_ratio",
    "length_finishes",
    "http_errors",
    "budget_exceeded",
    "doom_loop",
    "context_full",
    "output_truncated",
    "compactions",
    "harness_success",
    "harness_error",
    "cost_usd",
    "completion_tokens_per_s",
    "patch_bytes",
)

_META = ("run_id", "suite", "arm", "seed", "task_id", "exit_code", "timed_out", "wall_s")
_PROXY = (
    "requests",
    "prompt_tokens",
    "completion_tokens",
    "total_tokens",
    "tool_calls",
    "read_calls",
    "edit_calls",
    "exec_calls",
    "read_edit_ratio",
    "length_finishes",
    "http_errors",
    "budget_exceeded",
    "completion_tokens_per_s",
)
_GUARDS = ("doom_loop", "context_full", "output_truncated")


def cost_usd(
    prompt_tokens: int, completion_tokens: int, in_per_m: float, out_per_m: float
) -> float:
    return (prompt_tokens * in_per_m + completion_tokens * out_per_m) / 1_000_000


def make_row(
    meta: Mapping,
    proxy_summary: Mapping,
    harness: Mapping,
    outcome: Mapping,
    prices: Mapping[str, float],
) -> dict:
    row: dict = dict.fromkeys(COLUMNS)
    for k in _META:
        row[k] = meta.get(k)
    row["patch_bytes"] = meta.get("patch_bytes")
    row["resolved"] = outcome.get("resolved")
    for k in _PROXY:
        row[k] = proxy_summary.get(k)
    guards = proxy_summary.get("guard_events") or {}
    for k in _GUARDS:
        row[k] = guards.get(k, 0)
    row["compactions"] = harness.get("compactions", 0)
    row["harness_success"] = harness.get("success")
    row["harness_error"] = harness.get("error")
    row["cost_usd"] = round(
        cost_usd(
            proxy_summary.get("prompt_tokens") or 0,
            proxy_summary.get("completion_tokens") or 0,
            prices.get("in_per_m", 0.0),
            prices.get("out_per_m", 0.0),
        ),
        6,
    )
    return row
