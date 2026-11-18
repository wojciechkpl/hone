"""Spec section 11.2, K1-K3: config/bench.env.example carries the campaign 1 defaults."""

import json
import re
import subprocess
from pathlib import Path

import pytest

from hcbench.config import load_env_file

ROOT = Path(__file__).resolve().parent.parent
EXAMPLE = ROOT / "config" / "bench.env.example"


@pytest.fixture(scope="module")
def env():
    return load_env_file(EXAMPLE, base={"HOME": "/h"})


# ---- K1 model ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("HC_SERVED_NAME", "qwen3.8-27b"),
        ("HC_MODEL_HF", "Inferact/Qwen3.8-27B-NVFP4"),
        ("HC_MODEL_REVISION", "CHANGE-ME-commit-sha"),
        ("HC_TOOL_PARSER", "qwen3_coder"),
        ("HC_REASONING_PARSER", "qwen3"),
        ("HC_ENFORCE_EAGER", "1"),
        ("HC_LANGUAGE_MODEL_ONLY", "1"),
    ],
)
def test_k1_model_default_should_be_pinned(env, key, value):
    assert env.get(key) == value


def test_k1_speculative_config_should_be_the_mtp_json(env):
    assert json.loads(env["HC_SPECULATIVE_CONFIG"]) == {
        "method": "mtp",
        "num_speculative_tokens": 3,
    }


def test_k1_speculative_config_should_survive_being_sourced_by_bash():
    # The scripts `source` this file: unquoted JSON would lose its double quotes.
    r = subprocess.run(
        ["bash", "-c", f'set -a; source "{EXAMPLE}"; printf %s "$HC_SPECULATIVE_CONFIG"'],
        capture_output=True,
        text=True,
        env={"PATH": "/usr/bin:/bin", "HOME": "/h", "HC_ROOT": str(ROOT)},
        check=True,
    )
    assert json.loads(r.stdout) == {"method": "mtp", "num_speculative_tokens": 3}


def test_k1_gguf_path_should_name_a_qwen38_27b_q4_k_m_file(env):
    p = env["HC_GGUF_PATH"].lower()
    assert "qwen3.8-27b" in p
    assert "q4_k_m" in p
    assert p.endswith(".gguf")


def test_k1_file_should_no_longer_mention_qwen3_coder_30b():
    assert not re.search(r"qwen3-coder-30b", EXAMPLE.read_text(), re.IGNORECASE)


# ---- K2 sampling ------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("HC_TEMPERATURE", "0.7"),
        ("HC_TOP_P", "0.8"),
        ("HC_TOP_K", "20"),
        ("HC_MIN_P", "0"),
        ("HC_PRESENCE_PENALTY", "1.5"),
        ("HC_REPETITION_PENALTY", "1.0"),
        ("HC_ENABLE_THINKING", "false"),
        ("HC_SEED_BASE", "20261117"),
    ],
)
def test_k2_sampling_default_should_be_pinned(env, key, value):
    assert env.get(key) == value


# ---- K3 suite A rules -------------------------------------------------------------------


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("HC_A_DATASET", "nebius/SWE-rebench-leaderboard"),
        ("HC_A_MIN_SPLIT", "2026_09"),
        ("HC_A_N", "60"),
        ("HC_A_REPEATS", "0,1"),
        ("HC_A_GRADER", "SWE-rebench/SWE-bench-fork"),
    ],
)
def test_k3_suite_a_rule_should_be_pinned(env, key, value):
    assert env.get(key) == value


def test_k3_swe_keys_should_stay(env):
    for key in ("HC_SWE_DATASET", "HC_SWE_SPLIT", "HC_SWE_N", "HC_SWE_NAMESPACE"):
        assert key in env


def test_k3_a_comment_should_say_swe_bench_lite_is_retired():
    comments = [ln for ln in EXAMPLE.read_text().splitlines() if ln.lstrip().startswith("#")]
    assert any("retired" in c.lower() and "lite" in c.lower() for c in comments)
