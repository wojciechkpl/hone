"""Spec section 3/13 (round 3): Holm-Bonferroni over the primary comparison family.

`report.holm(pvalues, alpha=0.05) -> dict[str, bool]` (True = rejected / claimed);
`stats.bootstrap_pvalue(a, b, iters=10000, seed=0) -> float` is the two-sided paired-bootstrap
p-value `min(1, 2 * min(share(mean* <= 0), share(mean* >= 0)))` over resampled task diffs.
In suite A the primary family is {hone-inject - pi, hone-inject - hermes}; the pairwise table's
last column ("Holm") says `yes`/`no` for family members and `-` (or an en dash) otherwise.
"""

import pytest

from hcbench.report import holm, pairwise_table
from hcbench.stats import bootstrap_pvalue

# ---- holm ----------------------------------------------------------------


@pytest.mark.parametrize(
    ("pvals", "expected"),
    [
        ({}, {}),
        ({"a": 0.049}, {"a": True}),
        ({"a": 0.051}, {"a": False}),
        ({"a": 0.01, "b": 0.04}, {"a": True, "b": True}),  # 0.01<=0.025, 0.04<=0.05
        ({"a": 0.01, "b": 0.06}, {"a": True, "b": False}),
        ({"a": 0.03, "b": 0.04}, {"a": False, "b": False}),  # 0.03>0.025: stop, b not tested
        ({"a": 0.025, "b": 0.5}, {"a": True, "b": False}),  # boundary p == alpha/m is rejected
        ({"a": 0.02, "b": 0.02}, {"a": True, "b": True}),  # tie
        ({"a": 0.03, "b": 0.03}, {"a": False, "b": False}),  # tie, both fail first threshold
        (
            {"a": 0.001, "b": 0.2, "c": 0.0001},  # sorted c, a, b: thresholds .0167 .025 .05
            {"a": True, "b": False, "c": True},
        ),
        (
            {"a": 0.02, "b": 0.03, "c": 0.011},  # .011<=.0167, .02<=.025, .03<=.05
            {"a": True, "b": True, "c": True},
        ),
        (
            {"a": 0.2, "b": 0.001, "c": 0.04},  # step-down stops at c (.04 > .025)
            {"a": False, "b": True, "c": False},
        ),
    ],
)
def test_holm_should_follow_step_down_procedure(pvals, expected):
    assert holm(pvals) == expected


def test_holm_should_respect_custom_alpha():
    assert holm({"a": 0.004, "b": 0.02}, alpha=0.01) == {"a": True, "b": False}


def test_holm_should_not_depend_on_input_order():
    p = {"a": 0.001, "b": 0.2, "c": 0.0001}
    assert holm(dict(reversed(list(p.items())))) == holm(p)


# ---- bootstrap_pvalue ------------------------------------------------------


def test_bootstrap_pvalue_when_all_diffs_positive_should_be_zero():
    a = {f"t{i}": 1.0 for i in range(20)}
    b = {f"t{i}": 0.0 for i in range(20)}
    assert bootstrap_pvalue(a, b) == 0.0


def test_bootstrap_pvalue_when_identical_should_be_one():
    a = {f"t{i}": float(i % 2) for i in range(20)}
    assert bootstrap_pvalue(a, dict(a)) == 1.0


def test_bootstrap_pvalue_when_balanced_gains_and_losses_should_be_large_and_capped_at_one():
    a = {f"t{i}": float(i % 2) for i in range(20)}
    b = {f"t{i}": float((i + 1) % 2) for i in range(20)}
    p = bootstrap_pvalue(a, b)
    assert 0.5 <= p <= 1.0


def test_bootstrap_pvalue_should_be_symmetric_deterministic_and_validate_input():
    a = {f"t{i}": 1.0 if i < 8 else 0.0 for i in range(10)}
    b = {f"t{i}": 0.0 for i in range(10)}
    assert bootstrap_pvalue(a, b) == bootstrap_pvalue(b, a)
    assert bootstrap_pvalue(a, b, seed=3) == bootstrap_pvalue(a, b, seed=3)
    assert 0.0 <= bootstrap_pvalue(a, b) <= 1.0
    with pytest.raises(ValueError):
        bootstrap_pvalue({"x": 1.0}, {"y": 0.0})


# ---- report ------------------------------------------------------------------


def _rows(solved: dict[str, set[int]], n=20, seeds=(0, 1)):
    rows = []
    for arm, ok in solved.items():
        for seed in seeds:
            for i in range(n):
                rows.append(
                    {
                        "suite": "a",
                        "arm": arm,
                        "seed": seed,
                        "task_id": f"t{i}",
                        "resolved": i in ok,
                    }
                )
    return rows


def _cells(table: str, label: str) -> list[str]:
    for line in table.splitlines():
        if line.startswith(f"| {label} |"):
            return [c.strip() for c in line.strip().strip("|").split("|")]
    raise AssertionError(f"no row {label!r} in:\n{table}")


def test_pairwise_table_should_claim_only_primary_differences_that_holm_rejects():
    everything = set(range(20))
    rows = _rows({"hone-inject": everything, "pi": set(), "hermes": everything})
    table = pairwise_table(rows, "a")
    assert "Holm" in table.splitlines()[0].split("|")[-2]
    assert _cells(table, "hone-inject − pi")[-1] == "yes"  # p = 0
    assert _cells(table, "hone-inject − hermes")[-1] == "no"  # identical: p = 1
    # CI-excludes-0 is still reported separately
    assert _cells(table, "hone-inject − pi")[-2] == "yes"


def test_pairwise_table_should_not_claim_when_holm_rejects_nothing():
    same = set(range(0, 20, 2))
    table = pairwise_table(_rows({"hone-inject": same, "pi": same, "hermes": same}), "a")
    assert _cells(table, "hone-inject − pi")[-1] == "no"
    assert _cells(table, "hone-inject − hermes")[-1] == "no"


def test_pairwise_table_should_leave_non_primary_pairs_out_of_the_holm_family():
    everything = set(range(20))
    rows = _rows({"hone-inject": everything, "hone-noinject": set(), "pi": set()})
    table = pairwise_table(rows, "a")
    assert _cells(table, "hone-inject − hone-noinject")[-1] in {"-", "–", "n/a"}
