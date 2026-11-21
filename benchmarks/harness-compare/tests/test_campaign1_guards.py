"""Spec section 11.4, K18-K22 and K30: fail before doing harm, run the right arms.

The scripts run against a fake docker whose argv log proves that nothing was removed or
started when a script stops.
"""

import os
import subprocess

import pytest
from fakebin import (
    LLAMA_PINNED,
    ROOT,
    SHA40,
    VLLM_PINNED,
    calls,
    docker_calls,
    env_file_without,
    harmful,
    make_fake_bin,
    make_rig,
    run_script,
)

IMAGE = {"HC_VLLM_IMAGE": VLLM_PINNED, "HC_MODEL_REVISION": SHA40}
REQUIRED = [
    "HC_TOP_K",
    "HC_MIN_P",
    "HC_PRESENCE_PENALTY",
    "HC_REPETITION_PENALTY",
    "HC_ENABLE_THINKING",
]


def go(tmp_path, script, args, env, shell="bash"):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    return rig, log, run_script(rig, script, args, bindir, env, shell=shell)


# ---- K19 thinking flag -------------------------------------------------------------------


@pytest.mark.parametrize("script", ["02_serve_model.sh", "03_stack.sh"])
@pytest.mark.parametrize("value", ["0", "1", "False", "TRUE", "yes", ""])
def test_k19_bad_thinking_value_should_stop_before_any_container_is_touched(
    tmp_path, script, value
):
    _, log, r = go(tmp_path, script, ["up"], {**IMAGE, "HC_ENABLE_THINKING": value})
    assert r.returncode != 0
    assert "HC_ENABLE_THINKING" in r.stderr
    assert harmful(log) == []


@pytest.mark.parametrize("script", ["02_serve_model.sh", "03_stack.sh"])
def test_k19_true_and_false_should_be_accepted(tmp_path, script):
    for value in ("true", "false"):
        _, _, r = go(tmp_path / value, script, ["up"], {**IMAGE, "HC_ENABLE_THINKING": value})
        assert r.returncode == 0, r.stderr


# ---- K20 required keys -------------------------------------------------------------------


@pytest.mark.parametrize("script", ["02_serve_model.sh", "03_stack.sh"])
@pytest.mark.parametrize("key", REQUIRED)
def test_k20_key_missing_from_an_old_style_env_file_should_stop_before_any_container(
    tmp_path, script, key
):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    old = env_file_without(rig, key)
    r = run_script(rig, script, ["up"], bindir, {**IMAGE, "HC_ENV_FILE": str(old)})
    assert r.returncode != 0
    assert key in r.stderr
    assert "bench.env.example" in r.stderr
    assert harmful(log) == []


@pytest.mark.parametrize("script", ["02_serve_model.sh", "03_stack.sh"])
@pytest.mark.parametrize("key", [k for k in REQUIRED if k != "HC_ENABLE_THINKING"])
def test_k20_empty_key_should_stop_before_any_container(tmp_path, script, key):
    _, log, r = go(tmp_path, script, ["up"], {**IMAGE, key: ""})
    assert r.returncode != 0
    assert key in r.stderr
    assert "bench.env.example" in r.stderr
    assert harmful(log) == []


def test_k20_old_style_env_without_top_k_should_not_remove_the_running_proxy(tmp_path):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    old = env_file_without(rig, "HC_TOP_K")
    r = run_script(rig, "03_stack.sh", ["up"], bindir, {"HC_ENV_FILE": str(old)})
    assert r.returncode != 0
    assert not any(c[:1] == ["rm"] and "hc-proxy" in c for c in docker_calls(log))


# ---- K21 pinned weights ------------------------------------------------------------------


@pytest.mark.parametrize("revision", ["", "CHANGE-ME-commit-sha"])
def test_k21_vllm_without_a_pinned_revision_should_stop_before_any_container(tmp_path, revision):
    env = {**IMAGE, "HC_MODEL_HF": "org/model", "HC_MODEL_REVISION": revision}
    _, log, r = go(tmp_path, "02_serve_model.sh", ["up"], env)
    assert r.returncode != 0
    assert "HC_MODEL_REVISION" in r.stderr
    assert harmful(log) == []


def test_k21_llamacpp_does_not_need_a_pinned_revision(tmp_path):
    env = {
        "HC_SERVER": "llamacpp",
        "HC_MODEL_REVISION": "CHANGE-ME-commit-sha",
        "HC_LLAMACPP_IMAGE": LLAMA_PINNED,
    }
    _, log, r = go(tmp_path, "02_serve_model.sh", ["up"], env)
    assert r.returncode == 0, r.stderr
    assert any(c[:1] == ["run"] for c in docker_calls(log))


@pytest.mark.skipif(not os.path.exists("/bin/bash"), reason="no /bin/bash")
def test_k30_k4_serve_should_run_under_the_system_bash_with_every_optional_flag_off(tmp_path):
    # macOS ships bash 3.2, where a bare empty-array expansion under `set -u` aborts.
    env = {
        **IMAGE,
        "HC_ENFORCE_EAGER": "0",
        "HC_LANGUAGE_MODEL_ONLY": "0",
        "HC_REASONING_PARSER": "",
        "HC_SPECULATIVE_CONFIG": "",
        "HC_ENABLE_THINKING": "true",
    }
    _, log, r = go(tmp_path, "02_serve_model.sh", ["up"], env, shell="/bin/bash")
    assert r.returncode == 0, r.stderr
    assert any(c[:1] == ["run"] for c in docker_calls(log))


# ---- K22 stage spelling ------------------------------------------------------------------

SELECT = "hcbench.select_tasks"


def prereg_run(tmp_path, stage_args):
    from test_campaign1_prereg_script import prereg_env

    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    env = {**prereg_env(rig, tmp_path), "FAKE_UV_STUB_MODULES": SELECT}
    r = run_script(rig, "05_preregister.sh", ["t1", "--draft", *stage_args], bindir, env)
    return rig, log, r


def selected(log):
    return any(SELECT in " ".join(c) for c in calls(log))


@pytest.mark.parametrize("stage_args", [["--stage", "cb"], ["--stage=cb"]])
def test_k22_stage_cb_in_either_spelling_should_skip_task_selection(tmp_path, stage_args):
    rig, log, r = prereg_run(tmp_path, stage_args)
    assert r.returncode == 0, r.stderr
    assert not selected(log)
    assert not any(c[:1] == ["pull"] for c in docker_calls(log))
    assert (rig / "prereg" / "t1" / "preregistration.toml").exists()


@pytest.mark.parametrize("stage_args", [["--stage", "a"], ["--stage=a"]])
def test_k22_stage_a_in_either_spelling_should_select_tasks(tmp_path, stage_args):
    # Round 14 (K100): stage a selects with `hcbench.rebench select`, not select_tasks, and
    # needs --rules (K97).
    from test_campaign1_prereg_script import prereg_env

    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    # The K117 preflight (hcbench.prereg) is stubbed like the other K100 script tests do.
    stub = "hcbench.rebench,hcbench.prereg"
    env = {**prereg_env(rig, tmp_path), "FAKE_UV_STUB_MODULES": stub}
    args = ["t1", "--draft", *stage_args, "--rules", "prereg/c1/preregistration.toml"]
    run_script(rig, "05_preregister.sh", args, bindir, env)
    assert any("hcbench.rebench" in " ".join(c) and "select" in c for c in calls(log))
    assert not selected(log)


@pytest.mark.parametrize(
    "stage_args", [["--stage", "x"], ["--stage=x"], ["--stage", "CB"], ["--stage="], ["--stage"]]
)
def test_k22_any_other_stage_should_stop_before_selection_and_pulls(tmp_path, stage_args):
    rig, log, r = prereg_run(tmp_path, stage_args)
    assert r.returncode != 0
    assert "stage" in r.stderr.lower()
    assert not selected(log)
    assert not any(c[:1] == ["pull"] for c in docker_calls(log))
    assert not (rig / "prereg" / "t1" / "preregistration.toml").exists()


def make_n(args):
    return subprocess.run(
        ["make", "-n", "prereg", *args], cwd=ROOT, capture_output=True, text=True, check=False
    )


@pytest.mark.parametrize("stage", ["a", "cb"])
def test_k22_make_prereg_should_accept_the_two_stages(stage):
    # Round 14 (K100): stage a also needs RULES=.
    rules = ["RULES=c1"] if stage == "a" else []
    r = make_n(["NAME=x", f"STAGE={stage}", *rules])
    assert r.returncode == 0, r.stderr


@pytest.mark.parametrize("stage", ["CB", "b", "a,cb"])
def test_k22_make_prereg_with_any_other_stage_should_fail_without_running_the_script(
    tmp_path, stage
):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = subprocess.run(
        ["make", "prereg", "NAME=x", f"STAGE={stage}"],
        cwd=rig,
        capture_output=True,
        text=True,
        env={**os.environ, "PATH": f"{bindir}:{os.environ['PATH']}"},
        check=False,
    )
    assert r.returncode != 0
    assert "STAGE" in r.stdout + r.stderr
    assert not any("hcbench.prereg" in " ".join(c) for c in calls(log))


# ---- K18 suite B arms --------------------------------------------------------------------

SIX = "pi,pi-guarded,hone-inject,hone-guarded,hermes,hermes-guarded"


def run_b(tmp_path, env=None):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(rig, "10_run.sh", ["b", "camp", "--dry-run"], bindir, env or {})
    matrix = [c for c in calls(log) if c[0] == "uv" and "hcbench.matrix" in c]
    return r, matrix


def test_k18_example_should_define_the_six_suite_b_arms():
    from hcbench.config import load_env_file

    env = load_env_file(ROOT / "config" / "bench.env.example", base={"HOME": "/h"})
    assert env["HC_ARMS_B"] == SIX


def test_k18_run_b_should_use_the_six_arms(tmp_path):
    r, matrix = run_b(tmp_path)
    assert r.returncode == 0, r.stderr
    (argv,) = matrix
    assert argv[argv.index("--arms") + 1] == SIX
    assert "runs=180" in r.stdout  # 6 arms x 10 tasks x 3 repeats


def test_k18_run_b_should_follow_hc_arms_b_not_hc_arms_main(tmp_path):
    r, matrix = run_b(tmp_path, {"HC_ARMS_B": "pi,hermes", "HC_ARMS_MAIN": "hone-inject"})
    assert r.returncode == 0, r.stderr
    (argv,) = matrix
    assert argv[argv.index("--arms") + 1] == "pi,hermes"


def test_k18_run_c_should_still_use_the_redteam_arms(tmp_path):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    run_script(rig, "10_run.sh", ["c", "camp", "--dry-run"], bindir, {"HC_ARMS_B": "pi"})
    (argv,) = [c for c in calls(log) if "hcbench.matrix" in c]
    assert argv[argv.index("--arms") + 1].count(",") == 5


# ---- K30 misc ----------------------------------------------------------------------------


def test_k30_k1_fallback_served_name():
    from hcbench.config import BenchConfig

    assert BenchConfig.from_env({}).model == "qwen3.8-27b"
