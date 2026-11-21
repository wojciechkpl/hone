"""Spec section 11.15, K83-K86: suite A window, split check, contamination, selection rule.

Interface assumptions:
* `validate_stage_cb(env, draft=...)` takes the env mapping, as today;
* `select_suite_a(instances, n, seed)` takes mappings with `instance_id` and `repo`.
"""

import random
import tomllib

import pytest
from test_campaign1_prereg import EXAMPLE, load_cb, pin, run_cb

from hcbench import prereg, sampling
from hcbench.config import load_env_file


def example_env() -> dict[str, str]:
    return dict(load_env_file(EXAMPLE, base={"HOME": "/h"}))


# ---- K83 ---------------------------------------------------------------------------------


def test_k83_example_should_pin_the_newest_split_and_date():
    vals = example_env()
    assert vals["HC_A_MIN_SPLIT"] == "2026_03"
    assert vals["HC_A_MIN_CREATED"] == "2026-03-01"


def test_k83_other_suite_a_values_should_be_unchanged():
    vals = example_env()
    assert vals["HC_A_DATASET"] == "nebius/SWE-rebench-leaderboard"
    assert vals["HC_A_N"] == "60"
    assert vals["HC_A_REPEATS"] == "0,1"
    assert vals["HC_A_GRADER"] == "SWE-rebench/SWE-bench-fork"


def test_k83_comment_should_state_the_facts():
    lines = EXAMPLE.read_text().splitlines()
    idx = next(i for i, ln in enumerate(lines) if ln.startswith("HC_A_MIN_SPLIT="))
    start = idx
    while start > 0 and lines[start - 1].startswith("#"):
        start -= 1
    # The comment block above the key, plus the comment between split and date keys.
    block = " ".join(lines[start : idx + 4]).lower()
    for token in ("2026-11-20", "2026-05-12", "110", "79", "qwen3.8", "august 2026"):
        assert token in block, token
    assert "contaminat" in block or "may have seen" in block


def test_k83_stage_cb_prereg_should_record_the_new_window(env):
    cb = load_cb(env)
    assert cb["suite_a_rules"]["min_split"] == "2026_03"
    assert cb["suite_a_rules"]["min_created"] == "2026-03-01"
    assert cb["config"]["HC_A_MIN_SPLIT"] == "2026_03"


# ---- K84 ---------------------------------------------------------------------------------

BAD_SPLITS = [
    "2026-03",
    "2026_13",
    "2026_00",
    "test",
    "26_03",
    "2026_3",
    "2026_031",
    "x2026_03",
    "2026_03\n",
    " 2026_03",
]


@pytest.mark.parametrize("value", BAD_SPLITS)
def test_k84_prereg_bad_min_split_should_stop_naming_key_and_value(env, value):
    pin(env)
    env["mp"].setenv("HC_A_MIN_SPLIT", value)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_A_MIN_SPLIT" in str(exc.value)
    assert repr(value) in str(exc.value)
    assert not (env["tmp"] / "pre.toml").exists()


@pytest.mark.parametrize("value", BAD_SPLITS)
def test_k84_prereg_bad_min_split_should_stop_in_draft_mode_too(env, value):
    env["mp"].setenv("HC_A_MIN_SPLIT", value)
    with pytest.raises(SystemExit) as exc:
        run_cb(env, "--draft")
    assert "HC_A_MIN_SPLIT" in str(exc.value)


@pytest.mark.parametrize("value", BAD_SPLITS)
def test_k84_validate_stage_cb_bad_min_split_should_raise_system_exit(value):
    vals = example_env()
    vals["HC_A_MIN_SPLIT"] = value
    with pytest.raises(SystemExit) as exc:
        prereg.validate_stage_cb(vals, draft=True)
    assert "HC_A_MIN_SPLIT" in str(exc.value)


@pytest.mark.parametrize("value", ["2026_03", "2025_12", "2026_12", "2025_01"])
def test_k84_validate_stage_cb_good_min_split_should_pass(value):
    vals = example_env()
    vals["HC_A_MIN_SPLIT"] = value
    prereg.validate_stage_cb(vals, draft=True)


@pytest.mark.parametrize("value", ["2026_03", "2025_12"])
def test_k84_prereg_good_min_split_should_be_recorded(env, value):
    pin(env)
    env["mp"].setenv("HC_A_MIN_SPLIT", value)
    rc, out = run_cb(env)
    assert rc == 0
    assert tomllib.loads(out.read_text())["suite_a_rules"]["min_split"] == value


# ---- K85 ---------------------------------------------------------------------------------


def test_k85_rules_should_record_the_contamination_risk(env):
    rules = load_cb(env)["suite_a_rules"]
    text = rules["contamination"].lower()
    assert "qwen3.8" in text
    assert "august 2026" in text
    assert "possibly contaminated" in text
    assert "same model" in text


def test_k85_existing_rule_keys_should_stay(env):
    rules = load_cb(env)["suite_a_rules"]
    for key in ("dataset", "min_split", "min_created", "n", "repeats", "grader", "stratify"):
        assert key in rules
    assert "if_fewer" in rules and "grader_pin" in rules


# ---- K86 ---------------------------------------------------------------------------------


def test_k86_stratify_should_name_both_generators_and_the_fallback(env):
    text = load_cb(env)["suite_a_rules"]["stratify"]
    assert "{seed}:repos" in text
    assert "{seed}:{repo}" in text
    low = text.lower()
    assert "largest-remainder" in low or "largest remainder" in low
    assert "at least one" in low


def make(spec: dict[str, int]) -> list[dict[str, str]]:
    return [
        {"instance_id": f"{repo}__t{i}", "repo": repo} for repo, k in spec.items() for i in range(k)
    ]


def reference(instances, n, seed):
    repos: dict[str, list[str]] = {}
    for inst in instances:
        repos.setdefault(inst["repo"], []).append(inst["instance_id"])
    drawn = random.Random(f"{seed}:repos").sample(sorted(repos), n)
    out = []
    for repo in drawn:
        out.extend(random.Random(f"{seed}:{repo}").sample(sorted(repos[repo]), 1))
    return sorted(out)


def realistic(total=110, repos=79):
    # 79 repositories, 110 tasks: 31 repositories have two tasks, the rest have one.
    spec = {f"org/repo{r:02d}": 2 if r < total - repos else 1 for r in range(repos)}
    return make(spec)


@pytest.mark.parametrize("n", [0, -1])
def test_k86_n_below_one_should_raise_value_error(n):
    with pytest.raises(ValueError):
        sampling.select_suite_a(make({"a/a": 2, "b/b": 1}), n, 1)


def test_k86_duplicate_instance_id_should_raise_value_error():
    inst = [*make({"a/a": 2, "b/b": 1}), {"instance_id": "a/a__t0", "repo": "a/a"}]
    with pytest.raises(ValueError):
        sampling.select_suite_a(inst, 2, 1)


@pytest.mark.parametrize("n", [4, 5, 100])
def test_k86_n_at_least_instances_should_return_all_ids_sorted(n):
    inst = make({"b/b": 2, "a/a": 2})
    assert sampling.select_suite_a(inst, n, 7) == sorted(i["instance_id"] for i in inst)


def test_k86_n_at_least_instances_should_return_all_even_with_few_repos():
    inst = make({"a/a": 3})
    assert sampling.select_suite_a(inst, 3, 7) == ["a/a__t0", "a/a__t1", "a/a__t2"]


@pytest.mark.parametrize(("n", "seed"), [(3, 1), (5, 20261109), (7, 0)])
def test_k86_repos_at_least_n_should_match_the_named_generators(n, seed):
    inst = make({f"o/r{i}": 1 + i % 3 for i in range(9)})
    got = sampling.select_suite_a(inst, n, seed)
    assert got == reference(inst, n, seed)
    assert len(got) == n
    assert len({g.split("__")[0] for g in got}) == n


def test_k86_repos_equal_n_should_take_one_task_from_each_repo():
    inst = make({"a/a": 3, "b/b": 2, "c/c": 1})
    got = sampling.select_suite_a(inst, 3, 11)
    assert got == reference(inst, 3, 11)
    assert {g.split("__")[0] for g in got} == {"a/a", "b/b", "c/c"}


@pytest.mark.parametrize("n", [5, 8, 10])
def test_k86_fewer_repos_than_n_should_equal_stratified_sample(n):
    inst = make({"a/a": 6, "b/b": 4, "c/c": 3, "d/d": 2})
    assert sampling.select_suite_a(inst, n, 5) == sampling.stratified_sample(inst, n, 5)


def test_k86_output_should_be_sorted():
    inst = make({f"o/r{i}": 2 for i in range(10)})
    for n in (4, 12):
        got = sampling.select_suite_a(inst, n, 3)
        assert got == sorted(got)


@pytest.mark.parametrize("n", [4, 12])
def test_k86_result_should_not_depend_on_input_order(n):
    inst = make({f"o/r{i}": 1 + i % 2 for i in range(9)})
    want = sampling.select_suite_a(inst, n, 9)
    for s in range(3):
        shuffled = list(inst)
        random.Random(s).shuffle(shuffled)
        assert sampling.select_suite_a(shuffled, n, 9) == want


def test_k86_should_be_deterministic_and_seed_sensitive():
    inst = realistic()
    a = sampling.select_suite_a(inst, 60, 1)
    assert a == sampling.select_suite_a(inst, 60, 1)
    assert any(sampling.select_suite_a(inst, 60, s) != a for s in range(2, 6))


@pytest.mark.parametrize("n", [60, 40])
def test_k86_realistic_shape_should_give_n_tasks_from_n_repos(n):
    inst = realistic()
    assert len(inst) == 110 and len({i["repo"] for i in inst}) == 79
    got = sampling.select_suite_a(inst, n, 20261109)
    assert got == reference(inst, n, 20261109)
    assert len(got) == n == len(set(got))
    assert len({g.rsplit("__", 1)[0] for g in got}) == n


def test_k86_allocate_should_still_refuse_n_below_strata():
    with pytest.raises(ValueError):
        sampling.allocate({"a": 1, "b": 1, "c": 1}, 2)
