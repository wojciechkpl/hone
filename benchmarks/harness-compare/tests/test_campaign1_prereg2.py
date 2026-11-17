"""Spec section 11.4, K19/K20 (hcbench.prereg), K24, K25, K29."""

import hashlib
import json
import tomllib

import pytest
from test_campaign1_prereg import EXAMPLE, ROOT, load_cb, pin, run_cb

from hcbench import prereg


def squash(value) -> str:
    text = json.dumps(value, sort_keys=True, ensure_ascii=False)
    return " ".join(text.replace("−", "-").replace("\\u2212", "-").lower().split())


# ---- K19 / K20 in hcbench.prereg ---------------------------------------------------------


@pytest.mark.parametrize("value", ["0", "1", "False", "TRUE", "yes", ""])
def test_k19_prereg_bad_thinking_value_should_stop_and_name_the_key(env, value):
    pin(env)
    env["mp"].setenv("HC_ENABLE_THINKING", value)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_ENABLE_THINKING" in str(exc.value)
    assert not (env["tmp"] / "pre.toml").exists()


def test_k19_prereg_true_should_force_and_record_true(env):
    pin(env)
    env["mp"].setenv("HC_ENABLE_THINKING", "true")
    rc, out = run_cb(env)
    data = tomllib.loads(out.read_text())
    assert rc == 0
    assert data["config"]["HC_ENABLE_THINKING"] == "true"
    assert data["sampling"]["chat_template_kwargs"] == {"enable_thinking": True}


REQUIRED_CB = [
    "HC_TOP_K",
    "HC_MIN_P",
    "HC_PRESENCE_PENALTY",
    "HC_REPETITION_PENALTY",
    "HC_ENABLE_THINKING",
    "HC_SEED_BASE",
    "HC_SELECTION_SEED",
    "HC_A_DATASET",
    "HC_A_MIN_SPLIT",
    "HC_A_N",
    "HC_A_REPEATS",
    "HC_A_GRADER",
]


@pytest.mark.parametrize("key", REQUIRED_CB)
def test_k20_prereg_empty_key_should_stop_naming_the_key_and_the_example(env, key):
    pin(env)
    env["mp"].setenv(key, "")
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert key in str(exc.value)
    assert "bench.env.example" in str(exc.value)


@pytest.mark.parametrize("key", REQUIRED_CB)
def test_k20_prereg_unset_key_should_stop_naming_the_key_and_the_example(env, key):
    pin(env)
    old = env["tmp"] / "old.env"
    old.write_text(
        "\n".join(
            ln for ln in EXAMPLE.read_text().splitlines() if not ln.lstrip().startswith(f"{key}=")
        )
        + "\n"
    )
    with pytest.raises(SystemExit) as exc:
        prereg.main(["--stage", "cb", "--out", str(env["tmp"] / "p.toml"), "--env-file", str(old)])
    assert key in str(exc.value)
    assert "bench.env.example" in str(exc.value)


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("HC_SEED_BASE", "abc"),
        ("HC_SELECTION_SEED", "1.5x"),
        ("HC_A_N", "sixty"),
        ("HC_A_REPEATS", "0,x"),
        ("HC_TOP_K", "twenty"),
        ("HC_MIN_P", "zero"),
        ("HC_PRESENCE_PENALTY", "lots"),
        ("HC_REPETITION_PENALTY", "none"),
    ],
)
def test_k20_prereg_non_numeric_value_should_stop_with_a_message_not_a_traceback(env, key, value):
    pin(env)
    env["mp"].setenv(key, value)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert key in str(exc.value)


# ---- K24 more in the pre-registration ----------------------------------------------------


@pytest.mark.parametrize(
    "key",
    [
        "HC_ARMS_B",
        "HC_CONCURRENCY",
        "HC_PIDS_LIMIT",
        "HC_MINI_IMAGE",
        "HC_REDTEAM_IMAGE",
        "HC_TOOLS_IMAGE",
        "HC_SELECTION_SEED",
    ],
)
def test_k24_config_should_record_the_key(env, key):
    assert key in load_cb(env)["config"]


def test_k24_config_should_record_values_from_the_example(env):
    cfg = load_cb(env)["config"]
    assert cfg["HC_ARMS_B"] == "pi,pi-guarded,hone-inject,hone-guarded,hermes,hermes-guarded"
    assert cfg["HC_SELECTION_SEED"] == "20261109"
    assert cfg["HC_CONCURRENCY"] == "4"


def test_k24_files_sha256_should_add_the_docker_files(env):
    files = load_cb(env)["files_sha256"]
    expected = {
        "docker/mini.Dockerfile": ROOT / "docker" / "mini.Dockerfile",
        "docker/redteam.Dockerfile": ROOT / "docker" / "redteam.Dockerfile",
        "docker/redteam/plant.sh": ROOT / "docker" / "redteam" / "plant.sh",
        "docker/tools.Dockerfile": ROOT / "docker" / "tools.Dockerfile",
    }
    # Assumption: the entries are keyed by the repo-relative path named in K24.
    for key, path in expected.items():
        assert files[key] == hashlib.sha256(path.read_bytes()).hexdigest(), key


# ---- K25 suite A rules in full -----------------------------------------------------------


def test_k25_example_should_define_min_created():
    from hcbench.config import load_env_file

    vals = load_env_file(EXAMPLE, base={"HOME": "/h"})
    assert vals["HC_A_MIN_CREATED"] == "2026-09-01"


def test_k25_config_should_record_min_created(env):
    assert load_cb(env)["config"]["HC_A_MIN_CREATED"] == "2026-09-01"


def test_k25_repeats_should_be_a_list_of_integers(env):
    repeats = load_cb(env)["suite_a_rules"]["repeats"]
    assert repeats == [0, 1]
    assert all(type(x) is int for x in repeats)


def test_k25_min_created_should_be_recorded(env):
    assert load_cb(env)["suite_a_rules"]["min_created"] == "2026-09-01"


def test_k25_stratify_if_fewer_and_grader_pin_should_state_the_section_10_3_rules(env):
    rules = load_cb(env)["suite_a_rules"]
    stratify = rules["stratify"].lower()
    assert "repositor" in stratify
    assert "largest-remainder" in stratify or "largest remainder" in stratify
    assert "at least one" in stratify
    if_fewer = rules["if_fewer"].lower()
    assert "all eligible" in if_fewer
    assert "fewer" in if_fewer
    pin_text = rules["grader_pin"].lower()
    assert "commit" in pin_text
    assert "stage a" in pin_text
    for key in ("stratify", "if_fewer", "grader_pin"):
        assert rules[key].strip().endswith(".")


# ---- K29 the pre-registered text matches section 11.3 ------------------------------------


@pytest.mark.parametrize(
    "needle",
    [
        "escape rate",
        "pass rate",
        "unjudged",
        "counts as escaped",
        "counts as failed",
        "c-guard",
        "c-cross",
        "b-guard",
        "pi-guarded - pi",
        "hone-guarded - hone-inject",
        "hermes-guarded - hermes",
        "hone-guarded - pi-guarded",
        "hone-guarded - hermes-guarded",
        "95%",
        "percentile",
        "paired bootstrap",
        "10,000",
        "seed 0",
        "two-sided",
        "p-value",
        "holm",
        "0.05",
        "no detectable difference",
        "mcnemar",
        "0 of 75",
        "24 of 30",
        # round 3: K31 unjudged text, K33 left-out rule, K34 verdicts, K32/K35 claim rule
        "infrastructure error",
        "re-run",
        "instrumentation error",
        "not re-run",
        "counted",
        "dropped",
        "left out",
        # K41 (replaces the two-treatment needle): worst-case bounds
        "worst-case",
        "bound",
        "against",
        "favours",
        "neither",
        "favoured arm",
        # K46: the longer descriptive list
        "outcome counts",
        "injection",
        "incomplete",
        "per-scenario outcome table",
        "wall time",
        "tool calls",
    ],
)
def test_k29_stage_cb_analysis_should_state(env, needle):
    assert needle in squash(load_cb(env)["analysis"])


def test_k29_analysis_should_no_longer_carry_the_round_1_wording(env):
    text = squash(load_cb(env)["analysis"])
    assert "n=40" not in text
    assert "swe" not in text


@pytest.mark.parametrize(
    "phrase", ["asks and blocks", "benign calls", "guard events", "both treatments"]
)
def test_k35_analysis_should_not_list_descriptive_items_the_report_does_not_produce(env, phrase):
    assert phrase not in squash(load_cb(env)["analysis"])
