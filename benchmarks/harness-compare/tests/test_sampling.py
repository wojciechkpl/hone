"""Spec section 2: hcbench.sampling."""

import hashlib
import random

import pytest

from hcbench.sampling import allocate, manifest_digest, stratified_sample

SWE_LITE = {
    "django/django": 114,
    "sympy/sympy": 77,
    "scikit-learn/scikit-learn": 23,
    "matplotlib/matplotlib": 23,
    "pytest-dev/pytest": 17,
    "sphinx-doc/sphinx": 16,
    "astropy/astropy": 6,
    "psf/requests": 6,
    "pylint-dev/pylint": 6,
    "pydata/xarray": 5,
    "mwaskom/seaborn": 4,
    "pallets/flask": 3,
}


def test_allocate_when_swe_bench_lite_n40_should_match_worked_example():
    expected = {
        "django/django": 15,
        "sympy/sympy": 10,
        "scikit-learn/scikit-learn": 3,
        "matplotlib/matplotlib": 2,
        "pytest-dev/pytest": 2,
        "sphinx-doc/sphinx": 2,
        "astropy/astropy": 1,
        "psf/requests": 1,
        "pylint-dev/pylint": 1,
        "pydata/xarray": 1,
        "mwaskom/seaborn": 1,
        "pallets/flask": 1,
    }
    assert allocate(SWE_LITE, 40) == expected


@pytest.mark.parametrize(
    ("sizes", "n"),
    [
        ({}, 1),
        ({"a": 0, "b": 5}, 3),
        ({"a": -1, "b": 5}, 3),
        ({"a": 5, "b": 5, "c": 5}, 2),  # n < number of strata
        ({"a": 2, "b": 3}, 6),  # n > total
    ],
)
def test_allocate_when_invalid_input_should_raise_value_error(sizes, n):
    with pytest.raises(ValueError):
        allocate(sizes, n)


def test_allocate_when_n_equals_strata_count_should_give_one_each():
    assert allocate({"a": 9, "b": 1, "c": 40}, 3) == {"a": 1, "b": 1, "c": 1}


def test_allocate_when_n_equals_total_should_return_sizes():
    sizes = {"a": 9, "b": 1, "c": 4}
    assert allocate(sizes, 14) == sizes


def test_allocate_when_increment_tie_should_favour_ascending_stratum_name():
    # quotas 1.5/1.5, floors 1/1, one slot left -> tie broken by name
    assert allocate({"b": 10, "a": 10}, 3) == {"a": 2, "b": 1}


def test_allocate_when_decrement_tie_should_decrement_ascending_stratum_name():
    # a,b quota 2.913 floor 2; c,d,e floored up to 1 -> sum 7 > 6; tie a/b -> a decremented
    sizes = {"b": 50, "a": 50, "c": 1, "d": 1, "e": 1}
    assert allocate(sizes, 6) == {"a": 1, "b": 2, "c": 1, "d": 1, "e": 1}


def test_allocate_when_floor_overshoots_should_decrement_only_strata_above_one():
    assert allocate({"big": 100, "x": 1, "y": 1}, 3) == {"big": 1, "x": 1, "y": 1}


@pytest.mark.parametrize("n", range(12, 301, 17))
def test_allocate_should_satisfy_sum_and_bounds_invariants(n):
    result = allocate(SWE_LITE, n)
    assert set(result) == set(SWE_LITE)
    assert sum(result.values()) == n
    assert all(1 <= result[k] <= SWE_LITE[k] for k in SWE_LITE)


def _instances(counts):
    return [
        {"instance_id": f"{repo}-{i:03d}", "repo": repo}
        for repo, count in counts.items()
        for i in range(count)
    ]


COUNTS = {"org/alpha": 30, "org/beta": 20, "org/gamma": 4}


def test_stratified_sample_should_return_sorted_ids_with_allocated_counts():
    inst = _instances(COUNTS)
    result = stratified_sample(inst, 9, seed=1)
    assert result == sorted(result)
    assert len(result) == 9 == len(set(result))
    expected = allocate(COUNTS, 9)
    for repo, k in expected.items():
        assert sum(1 for r in result if r.startswith(repo + "-")) == k


def test_stratified_sample_should_be_deterministic():
    inst = _instances(COUNTS)
    assert stratified_sample(inst, 9, seed=3) == stratified_sample(inst, 9, seed=3)


def test_stratified_sample_should_not_depend_on_instance_order():
    inst = _instances(COUNTS)
    shuffled = list(inst)
    random.Random(99).shuffle(shuffled)
    assert stratified_sample(shuffled, 9, seed=3) == stratified_sample(inst, 9, seed=3)
    assert stratified_sample(inst[::-1], 9, seed=3) == stratified_sample(inst, 9, seed=3)


def test_stratified_sample_when_seed_differs_should_change_selection():
    inst = _instances({"org/a": 50, "org/b": 50})
    assert stratified_sample(inst, 10, seed=1) != stratified_sample(inst, 10, seed=2)


def test_stratified_sample_should_follow_preregistered_algorithm():
    inst = _instances(COUNTS)
    quotas = allocate(COUNTS, 9)
    expected = []
    for stratum, k in quotas.items():
        ids = sorted(i["instance_id"] for i in inst if i["repo"] == stratum)
        expected += random.Random(f"5:{stratum}").sample(ids, k)
    assert stratified_sample(inst, 9, seed=5) == sorted(expected)


def test_stratified_sample_when_custom_key_should_group_by_it():
    inst = [
        {"instance_id": f"{fam}{i}", "repo": "same", "family": fam}
        for fam in ("x", "y")
        for i in range(5)
    ]
    result = stratified_sample(inst, 4, seed=0, key="family")
    assert sum(r.startswith("x") for r in result) == 2
    assert sum(r.startswith("y") for r in result) == 2


@pytest.mark.parametrize("n", [1, 99])
def test_stratified_sample_when_n_out_of_range_should_raise_value_error(n):
    with pytest.raises(ValueError):
        stratified_sample(_instances(COUNTS), n, seed=0)


def test_manifest_digest_should_hash_sorted_newline_joined_ids():
    expected = hashlib.sha256(b"a\nb\nc\n").hexdigest()
    assert manifest_digest(["c", "a", "b"]) == expected


def test_manifest_digest_should_accept_any_iterable_and_be_order_independent():
    assert manifest_digest(iter(["b", "a"])) == manifest_digest(["a", "b"])


def test_manifest_digest_when_empty_should_hash_single_newline():
    assert manifest_digest([]) == hashlib.sha256(b"\n").hexdigest()
