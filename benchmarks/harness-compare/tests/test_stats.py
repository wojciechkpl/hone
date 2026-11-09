"""Spec section 3: hcbench.stats."""

import math
import statistics

import pytest

from hcbench.stats import (
    bootstrap_mean_ci,
    discordant,
    mcnemar_exact,
    paired_bootstrap,
    pass_at_1,
    task_means,
    wilson_interval,
)

# ---- wilson ------------------------------------------------------------


def test_wilson_interval_when_n_zero_should_be_full_range():
    assert wilson_interval(0, 0) == (0.0, 1.0)


def test_wilson_interval_when_half_should_match_known_values():
    lo, hi = wilson_interval(5, 10)
    assert lo == pytest.approx(0.2366, abs=1e-3)
    assert hi == pytest.approx(0.7634, abs=1e-3)


def test_wilson_interval_when_zero_successes_should_clamp_lower_bound_to_zero():
    lo, hi = wilson_interval(0, 10)
    assert lo == pytest.approx(0.0, abs=1e-9)
    assert hi == pytest.approx(0.2775, abs=1e-3)


def test_wilson_interval_when_all_successes_should_clamp_upper_bound_to_one():
    lo, hi = wilson_interval(10, 10)
    assert hi == pytest.approx(1.0, abs=1e-9)
    assert lo == pytest.approx(0.7225, abs=1e-3)


def test_wilson_interval_when_larger_sample_should_be_narrower():
    narrow = wilson_interval(500, 1000)
    wide = wilson_interval(5, 10)
    assert narrow[1] - narrow[0] < wide[1] - wide[0]


@pytest.mark.parametrize(("s", "n"), [(-1, 5), (6, 5), (1, 0)])
def test_wilson_interval_when_invalid_counts_should_raise_value_error(s, n):
    with pytest.raises(ValueError):
        wilson_interval(s, n)


# ---- bootstrap_mean_ci -------------------------------------------------


def test_bootstrap_mean_ci_when_empty_should_raise_value_error():
    with pytest.raises(ValueError):
        bootstrap_mean_ci([])


def test_bootstrap_mean_ci_when_constant_should_collapse_to_point():
    assert bootstrap_mean_ci([0.7, 0.7, 0.7], iters=200) == (0.7, 0.7)


def test_bootstrap_mean_ci_should_be_deterministic_for_seed():
    vals = [0, 1, 1, 0, 1, 0, 0, 1, 1, 1]
    assert bootstrap_mean_ci(vals, iters=500, seed=4) == bootstrap_mean_ci(vals, iters=500, seed=4)


def test_bootstrap_mean_ci_should_bracket_the_sample_mean_within_data_range():
    vals = [0, 1] * 30
    lo, hi = bootstrap_mean_ci(vals, iters=1000, seed=1)
    assert 0.0 <= lo <= 0.5 <= hi <= 1.0
    assert hi - lo > 0


def test_bootstrap_mean_ci_when_wider_alpha_should_be_narrower_interval():
    vals = [i / 20 for i in range(20)]
    lo95, hi95 = bootstrap_mean_ci(vals, iters=2000, seed=1, alpha=0.05)
    lo50, hi50 = bootstrap_mean_ci(vals, iters=2000, seed=1, alpha=0.5)
    assert (hi50 - lo50) < (hi95 - lo95)


# ---- paired_bootstrap --------------------------------------------------


def test_paired_bootstrap_when_identical_maps_should_have_zero_diff():
    m = {"t1": 1.0, "t2": 0.0, "t3": 0.5}
    res = paired_bootstrap(m, dict(m), iters=200)
    assert res["diff"] == 0
    assert res["lo"] == 0
    assert res["hi"] == 0
    assert res["n_tasks"] == 3


def test_paired_bootstrap_should_report_mean_difference_and_bracket_it():
    a = {f"t{i}": float(i % 2) for i in range(20)}
    b = {f"t{i}": 0.0 for i in range(20)}
    res = paired_bootstrap(a, b, iters=500, seed=2)
    assert res["diff"] == pytest.approx(0.5)
    assert res["lo"] <= res["diff"] + 1e-9
    assert res["diff"] <= res["hi"] + 1e-9
    assert res["n_tasks"] == 20


def test_paired_bootstrap_should_use_only_shared_keys():
    a = {"t1": 1.0, "t2": 1.0, "only_a": 0.0}
    b = {"t1": 0.0, "t2": 0.0, "only_b": 1.0}
    res = paired_bootstrap(a, b, iters=100)
    assert res["n_tasks"] == 2
    assert res["diff"] == pytest.approx(1.0)


def test_paired_bootstrap_should_be_deterministic_for_seed():
    a = {f"t{i}": float(i % 3 == 0) for i in range(15)}
    b = {f"t{i}": float(i % 5 == 0) for i in range(15)}
    assert paired_bootstrap(a, b, iters=300, seed=7) == paired_bootstrap(a, b, iters=300, seed=7)


def test_paired_bootstrap_when_no_shared_keys_should_raise_value_error():
    with pytest.raises(ValueError):
        paired_bootstrap({"a": 1.0}, {"b": 1.0})


# ---- mcnemar -----------------------------------------------------------


@pytest.mark.parametrize(
    ("b", "c", "expected"),
    [
        (0, 0, 1.0),
        (0, 5, 0.0625),
        (5, 0, 0.0625),
        (5, 5, 1.0),
        (3, 3, 1.0),
        (1, 9, 22 / 1024),
        (2, 8, 2 * (1 + 10 + 45) / 1024),
        (0, 1, 1.0),
    ],
)
def test_mcnemar_exact_should_match_known_values(b, c, expected):
    assert mcnemar_exact(b, c) == pytest.approx(expected)


def test_mcnemar_exact_should_never_exceed_one():
    assert mcnemar_exact(20, 21) <= 1.0


# ---- discordant --------------------------------------------------------


def test_discordant_should_count_a_only_and_b_only_on_shared_keys():
    a = {"t1": True, "t2": True, "t3": False, "t4": False, "only_a": True}
    b = {"t1": False, "t2": True, "t3": True, "t4": False, "only_b": True}
    assert discordant(a, b) == (1, 1)


def test_discordant_when_all_a_wins_should_return_counts_in_a_b_order():
    a = {"x": True, "y": True, "z": True}
    b = {"x": False, "y": False, "z": True}
    assert discordant(a, b) == (2, 0)
    assert discordant(b, a) == (0, 2)


# ---- task_means / pass_at_1 -------------------------------------------


def _row(arm, task, seed, resolved):
    return {"arm": arm, "task_id": task, "seed": seed, "resolved": resolved}


def test_task_means_should_average_over_seeds_and_filter_by_arm():
    rows = [
        _row("pi", "t1", 0, True),
        _row("pi", "t1", 1, False),
        _row("pi", "t2", 0, True),
        _row("hone", "t1", 0, False),
    ]
    assert task_means(rows, "pi") == {"t1": 0.5, "t2": 1.0}
    assert task_means(rows, "hone") == {"t1": 0.0}


def test_task_means_when_custom_metric_should_use_it():
    rows = [
        {"arm": "pi", "task_id": "t1", "seed": 0, "resolved": 0, "applied": 1},
        {"arm": "pi", "task_id": "t1", "seed": 1, "resolved": 0, "applied": 0},
    ]
    assert task_means(rows, "pi", metric="applied") == {"t1": 0.5}


def test_task_means_should_accept_a_generator():
    gen = (_row("pi", "t1", s, True) for s in range(3))
    assert task_means(gen, "pi") == {"t1": 1.0}


def test_pass_at_1_when_multiple_seeds_should_report_all_fields():
    rows = [
        _row("pi", "t1", 0, True),
        _row("pi", "t1", 1, True),
        _row("pi", "t2", 0, False),
        _row("pi", "t2", 1, True),
        _row("hone", "t1", 0, False),
    ]
    res = pass_at_1(rows, "pi", iters=500)
    assert set(res) == {"mean", "ci_lo", "ci_hi", "n_tasks", "n_runs", "seed_sd"}
    assert res["mean"] == pytest.approx(0.75)
    assert res["n_tasks"] == 2
    assert res["n_runs"] == 4
    # per-seed pass rates: seed0 = 1/2, seed1 = 2/2 -> sample sd
    assert res["seed_sd"] == pytest.approx(statistics.stdev([0.5, 1.0]))
    assert res["ci_lo"] <= res["mean"] <= res["ci_hi"]


def test_pass_at_1_when_single_seed_should_have_zero_seed_sd():
    rows = [_row("pi", f"t{i}", 0, i % 2 == 0) for i in range(6)]
    res = pass_at_1(rows, "pi", iters=200)
    assert res["seed_sd"] == 0.0
    assert res["mean"] == pytest.approx(0.5)
    assert res["n_runs"] == 6


def test_pass_at_1_should_be_deterministic_for_seed():
    rows = [_row("pi", f"t{i}", s, (i + s) % 3 == 0) for i in range(8) for s in range(2)]
    assert pass_at_1(rows, "pi", iters=300, seed=3) == pass_at_1(rows, "pi", iters=300, seed=3)


def test_pass_at_1_when_arm_has_no_rows_should_raise_value_error():
    with pytest.raises(ValueError):
        pass_at_1([_row("pi", "t1", 0, True)], "ghost")


def test_pass_at_1_mean_should_be_finite_float():
    res = pass_at_1([_row("pi", "t1", 0, True)], "pi", iters=50)
    assert math.isfinite(res["mean"])
    assert res["mean"] == 1.0
