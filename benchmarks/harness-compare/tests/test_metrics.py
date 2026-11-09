"""Spec section 9: hcbench.metrics."""

import pytest

from hcbench.metrics import COLUMNS, cost_usd, make_row

EXPECTED_COLUMNS = (
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

META = {
    "run_id": "a.pi.s1.t1",
    "suite": "a",
    "arm": "pi",
    "seed": 1,
    "task_id": "t1",
    "exit_code": 0,
    "timed_out": False,
    "wall_s": 12.5,
    "patch_bytes": 321,
}

PROXY = {
    "requests": 4,
    "prompt_tokens": 1234,
    "completion_tokens": 567,
    "total_tokens": 1801,
    "tool_calls": 6,
    "read_calls": 3,
    "edit_calls": 2,
    "exec_calls": 1,
    "other_calls": 0,
    "read_edit_ratio": 1.5,
    "length_finishes": 1,
    "http_errors": 0,
    "budget_exceeded": False,
    "llm_seconds": 8.0,
    "completion_tokens_per_s": 70.875,
    "guard_events": {"doom_loop": 2, "context_full": 1, "output_truncated": 3},
}

PRICES = {"in_per_m": 0.3333333, "out_per_m": 0.7}
OK = {"success": True, "error": None}


def test_columns_should_match_spec_order():
    assert tuple(COLUMNS) == EXPECTED_COLUMNS


def test_cost_usd_should_apply_per_million_prices():
    assert cost_usd(1_000_000, 500_000, 3.0, 15.0) == pytest.approx(10.5)
    assert cost_usd(0, 0, 3.0, 15.0) == 0


def test_make_row_should_have_exactly_columns_in_order():
    row = make_row(META, PROXY, OK, {"resolved": True}, PRICES)
    assert list(row) == list(EXPECTED_COLUMNS)


def test_make_row_should_map_fields_from_each_source():
    harness = {"success": False, "error": "boom", "compactions": 2}
    row = make_row(META, PROXY, harness, {"resolved": True}, PRICES)
    for key, value in META.items():
        assert row[key] == value, key
    assert row["resolved"] is True
    for key in (
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
    ):
        assert row[key] == PROXY[key], key
    assert (row["doom_loop"], row["context_full"], row["output_truncated"]) == (2, 1, 3)
    assert row["compactions"] == 2
    assert row["harness_success"] is False
    assert row["harness_error"] == "boom"


def test_make_row_when_outcome_has_no_resolved_should_be_none():
    assert make_row(META, PROXY, OK, {}, PRICES)["resolved"] is None


def test_make_row_when_harness_has_no_compactions_should_default_to_zero():
    assert make_row(META, PROXY, OK, {}, PRICES)["compactions"] == 0


def test_make_row_should_round_cost_to_six_decimals():
    row = make_row(META, PROXY, OK, {}, PRICES)
    # 1234 * 0.3333333e-6 + 567 * 0.7e-6 = 0.00080823...
    assert row["cost_usd"] == 0.000808
