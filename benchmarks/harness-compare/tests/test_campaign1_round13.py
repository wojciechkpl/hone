"""Spec section 11.16, K88-K91: the recorded suite A text can be followed without the code.

Assumptions: the two calls in `[suite_a_rules].stratify` are written as
`random.Random(f"...").sample(...)` code, with the names `seed`, `repos`, `ids`, `repo`, `n`.
"""

import random

import pytest
from test_campaign1_prereg import EXAMPLE, load_cb, pin, run_cb
from test_campaign1_round12 import example_env, make

from hcbench import prereg, sampling

# ---- K88 ---------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "token",
    ["YYYY_MM", "test", "as text", "min_split", "min_created", "revision"],
)
def test_k88_eligible_should_mention(env, token):
    assert token in load_cb(env)["suite_a_rules"]["eligible"], token


def test_k88_eligible_should_say_a_task_without_a_date_is_not_eligible(env):
    text = " ".join(load_cb(env)["suite_a_rules"]["eligible"].lower().split())
    assert "no creation date" in text
    assert "not eligible" in text


def test_k88_suite_a_text_should_define_eligible():
    assert "eligible" in prereg.SUITE_A_TEXT


def test_k88_min_created_comment_should_say_no_date_is_not_eligible():
    lines = EXAMPLE.read_text().splitlines()
    idx = next(i for i, ln in enumerate(lines) if ln.startswith("HC_A_MIN_CREATED="))
    block = []
    j = idx - 1
    while j >= 0 and lines[j].startswith("#"):
        block.append(lines[j])
        j -= 1
    text = " ".join(" ".join(block).lower().split())
    assert "no creation date" in text
    assert "not eligible" in text
    assert "when the dataset carries one" not in EXAMPLE.read_text()


# ---- K89 ---------------------------------------------------------------------------------


def test_k89_contamination_should_name_the_date_and_keep_the_k85_phrases(env):
    text = load_cb(env)["suite_a_rules"]["contamination"].lower()
    assert "2026-11-20" in text
    for phrase in ("qwen3.8", "august 2026", "possibly contaminated", "same model"):
        assert phrase in text, phrase


def test_k89_contamination_should_say_later_splits_may_add_newer_tasks(env):
    text = load_cb(env)["suite_a_rules"]["contamination"].lower()
    assert "later" in text
    assert "creation date" in text


# ---- K90 ---------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "token",
    [
        "selection_seed",
        "repo",
        "instance_id",
        "hcbench.sampling.stratified_sample",
        "repo.head",
        "hcbench.sampling.select_suite_a",
    ],
)
def test_k90_stratify_should_mention(env, token):
    assert token in load_cb(env)["suite_a_rules"]["stratify"], token


def test_k90_if_fewer_should_say_n_or_fewer_and_stage_a_day(env):
    text = load_cb(env)["suite_a_rules"]["if_fewer"]
    assert "n or fewer" in text
    assert "stage a" in text


def _close(text: str, open_at: int) -> int:
    """Index just past the parenthesis that closes the one at `open_at`."""
    depth = 0
    for i in range(open_at, len(text)):
        depth += {"(": 1, ")": -1}.get(text[i], 0)
        if depth == 0:
            return i + 1
    raise AssertionError(f"unbalanced parentheses in {text[open_at:]!r}")


def _calls(text: str) -> list[str]:
    """Pull each `random.Random(...).sample(...)` expression out of the sentence."""
    out, pos = [], 0
    while (start := text.find("random.Random(", pos)) >= 0:
        end = _close(text, start + len("random.Random"))
        assert text.startswith(".sample(", end), text[start:end]
        end = _close(text, end + len(".sample"))
        out.append(text[start:end])
        pos = end
    return out


def _populations():
    pops = []
    # 79 repositories, 110 tasks, with ids in a scrambled order.
    spec = {f"org/repo{r:02d}": 2 if r < 31 else 1 for r in range(79)}
    pops.append((make(spec), (60, 40, 79), 20261109))
    pops.append((make({f"o/r{i}": 1 + i % 3 for i in range(9)}), (3, 5, 9), 1))
    pops.append((make({f"x/y{i}": 1 for i in range(12)}), (4, 12), 0))
    return pops


def test_k90_stratify_should_hold_exactly_two_runnable_calls(env):
    calls = _calls(load_cb(env)["suite_a_rules"]["stratify"])
    assert len(calls) == 2
    assert "repos" in calls[0] and "ids" in calls[1]


@pytest.mark.parametrize("pop_index", [0, 1, 2])
def test_k91_calls_in_stratify_should_reproduce_select_suite_a(env, pop_index):
    draw_repos, draw_one = _calls(load_cb(env)["suite_a_rules"]["stratify"])
    inst, ns, base = _populations()[pop_index]
    for n in ns:
        for seed in (base, base + 1, base + 2):
            by_repo: dict[str, list[str]] = {}
            for i in inst:
                by_repo.setdefault(i["repo"], []).append(i["instance_id"])
            repos = list({i["repo"] for i in inst})
            random.Random(seed).shuffle(repos)
            drawn = eval(draw_repos, {"random": random, "seed": seed, "repos": repos, "n": n})
            got = []
            for repo in drawn:
                ids = list(by_repo[repo])
                got += eval(draw_one, {"random": random, "seed": seed, "repo": repo, "ids": ids})
            assert sorted(got) == sampling.select_suite_a(inst, n, seed), (n, seed)


# ---- K91 ---------------------------------------------------------------------------------


@pytest.mark.parametrize("n", [3, 4, 50])
def test_k91_duplicate_id_should_raise_even_when_n_covers_all_instances(n):
    inst = [*make({"a/a": 2, "b/b": 1}), {"instance_id": "a/a__t0", "repo": "a/a"}]
    with pytest.raises(ValueError):
        sampling.select_suite_a(inst, n, 1)


@pytest.mark.parametrize("value", ["2026_03\n", " 2026_03", "2026_03 ", "\t2026_03"])
@pytest.mark.parametrize("draft", [True, False])
def test_k91_validate_stage_cb_should_refuse_split_with_whitespace(value, draft):
    vals = example_env()
    vals["HC_A_MIN_SPLIT"] = value
    if not draft:
        pytest.importorskip("hcbench")
        from fakebin import SHA40, VLLM_PINNED

        vals.update({"HC_SERVER": "vllm", "HC_MODEL_REVISION": SHA40, "HC_VLLM_IMAGE": VLLM_PINNED})
    with pytest.raises(SystemExit) as exc:
        prereg.validate_stage_cb(vals, draft=draft)
    assert "HC_A_MIN_SPLIT" in str(exc.value)


@pytest.mark.parametrize("value", ["2026_03\n", " 2026_03"])
def test_k91_prereg_cli_should_refuse_split_with_whitespace(env, value):
    pin(env)
    env["mp"].setenv("HC_A_MIN_SPLIT", value)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_A_MIN_SPLIT" in str(exc.value)
