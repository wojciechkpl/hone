"""Spec section 11.2, K9-K15: stage-aware pre-registration (hcbench.prereg).

Assumptions the implementer must honour (the spec leaves them open):
* inside [suite_a_rules] the keys are those named in K11/K25;
* [sampling] holds the eight non-model forced values and `seed_base` (an `n` key is tolerated);
* `cut_order` is a list of three items whose text names suite G, then D, then A.
"""

import hashlib
import json
import tomllib
from pathlib import Path

import pytest
from fakebin import SHA40, VLLM_PINNED

from hcbench import prereg

ROOT = Path(__file__).resolve().parent.parent
EXAMPLE = ROOT / "config" / "bench.env.example"

TODAYS_CONFIG_KEYS = [
    "HC_SERVER", "HC_SERVED_NAME", "HC_MODEL_HF", "HC_MODEL_REVISION", "HC_TOOL_PARSER",
    "HC_VLLM_IMAGE", "HC_LLAMACPP_IMAGE", "HC_GGUF_PATH", "HC_MAX_MODEL_LEN", "HC_KV_CACHE_DTYPE",
    "HC_MAX_NUM_SEQS", "HC_GPU_MEM_UTIL", "HC_TEMPERATURE", "HC_TOP_P", "HC_MAX_TOKENS",
    "HC_MAX_REQUESTS", "HC_MAX_TOTAL_TOKENS", "HC_TIMEOUT_S", "HC_CPUS", "HC_MEMORY",
    "HC_REPEATS", "HC_ARMS_MAIN", "HC_ARMS_REDTEAM", "HC_PRICE_IN_PER_M", "HC_PRICE_OUT_PER_M",
    "HC_PI_VERSION", "HC_NODE_VERSION", "HC_HONE_REF", "HC_HERMES_REF", "HC_HERMES_SHA",
]  # fmt: skip
NEW_CONFIG_KEYS = [
    "HC_REASONING_PARSER", "HC_ENFORCE_EAGER", "HC_LANGUAGE_MODEL_ONLY", "HC_SPECULATIVE_CONFIG",
    "HC_TOP_K", "HC_MIN_P", "HC_PRESENCE_PENALTY", "HC_REPETITION_PENALTY", "HC_ENABLE_THINKING",
    "HC_SEED_BASE",
    "HC_A_DATASET", "HC_A_MIN_SPLIT", "HC_A_N", "HC_A_REPEATS", "HC_A_GRADER",
]  # fmt: skip

SAMPLING = {
    "temperature": 0.7,
    "top_p": 0.8,
    "top_k": 20,
    "min_p": 0,
    "presence_penalty": 1.5,
    "repetition_penalty": 1.0,
    "max_tokens": 6144,
    "chat_template_kwargs": {"enable_thinking": False},
}


def pin(env):
    for k in env["placeholders"]:
        # K47: a non-draft stage-cb pre-registration needs a 40-hex weights revision.
        # K58: a non-draft pre-registration needs a digest-pinned server image.
        pinned = {"HC_MODEL_REVISION": SHA40, "HC_VLLM_IMAGE": VLLM_PINNED}
        env["mp"].setenv(k, pinned.get(k, "pinned-for-test"))


def run_cb(env, *extra):
    out = env["tmp"] / "pre.toml"
    rc = prereg.main(["--stage", "cb", "--out", str(out), "--env-file", str(EXAMPLE), *extra])
    return rc, out


def load_cb(env):
    pin(env)
    rc, out = run_cb(env)
    assert rc == 0
    return tomllib.loads(out.read_text())


def flat(value) -> str:
    return json.dumps(value, sort_keys=True)


# ---- K9 stages / K10 no task list -------------------------------------------------------


def test_k9_stage_cb_should_need_no_selection_file(env):
    pin(env)
    rc, out = run_cb(env)
    assert rc == 0
    assert out.exists()


def test_k9_unknown_stage_should_be_rejected(env, capsys):
    with pytest.raises(SystemExit):
        prereg.main(["--stage", "x", "--out", str(env["tmp"] / "p.toml")])
    assert "invalid choice" in capsys.readouterr().err


def test_k9_default_stage_should_be_a_and_still_require_selection(env, capsys):
    pin(env)
    with pytest.raises(SystemExit):
        prereg.main(["--out", str(env["tmp"] / "p.toml"), "--env-file", str(EXAMPLE)])
    err = capsys.readouterr().err
    assert "--selection" in err
    assert "unrecognized" not in err


def test_k9_stage_a_explicit_should_still_require_selection(env, capsys):
    with pytest.raises(SystemExit):
        prereg.main(["--stage", "a", "--out", str(env["tmp"] / "p.toml")])
    err = capsys.readouterr().err
    assert "--selection" in err
    assert "unrecognized" not in err


def test_k10_stage_cb_should_not_include_a_task_list(env):
    cb = load_cb(env)
    assert "tasks" not in cb or not any(cb["tasks"].values())
    assert "tasks_txt" not in cb["files_sha256"]
    assert "instance_id" not in flat(cb)


# ---- K11 contents -----------------------------------------------------------------------


def test_k11_stage_and_suite_scope(env):
    cb = load_cb(env)
    assert cb["stage"] == "cb"
    assert cb["suites_in_scope"] == ["c", "b"]
    assert cb["suites_deferred"] == ["d", "e", "g"]


def test_k11_config_should_hold_todays_keys_and_the_k1_k2_k3_keys(env):
    cb = load_cb(env)
    missing = [k for k in TODAYS_CONFIG_KEYS + NEW_CONFIG_KEYS if k not in cb["config"]]
    assert missing == []


def test_k11_config_should_record_the_example_values(env):
    cb = load_cb(env)
    cfg = cb["config"]
    assert cfg["HC_SERVED_NAME"] == "qwen3.8-27b"
    assert cfg["HC_SEED_BASE"] == "20261117"
    assert json.loads(cfg["HC_SPECULATIVE_CONFIG"]) == {
        "method": "mtp",
        "num_speculative_tokens": 3,
    }
    assert cfg["HC_A_N"] == "60"


def test_k11_sampling_table_should_hold_the_forced_values_and_seed_base(env):
    cb = load_cb(env)
    s = dict(cb["sampling"])
    s.pop("n", None)
    assert s == {**SAMPLING, "seed_base": 20261117}
    assert "model" not in cb["sampling"]


def test_k11_suite_a_rules_should_use_the_named_keys(env):
    # K11 names the keys; K25 makes repeats a list of integers (round 1 accepted a string).
    cb = load_cb(env)
    rules = cb["suite_a_rules"]
    assert rules["dataset"] == "nebius/SWE-rebench-leaderboard"
    assert rules["min_split"] == "2026_09"
    assert rules["n"] == 60
    assert rules["repeats"] == [0, 1]
    assert rules["grader"] == "SWE-rebench/SWE-bench-fork"
    assert rules["selection_seed"] == 20261109


def test_k11_cut_order_should_be_the_three_cuts_of_section_10_5_in_order(env):
    cb = load_cb(env)
    cuts = cb["cut_order"]
    assert len(cuts) == 3
    assert "G" in str(cuts[0])
    assert "D" in str(cuts[1])
    assert "A" in str(cuts[2]) and "40" in str(cuts[2])


def test_k11_files_sha256_should_cover_arms_suite_b_suite_c_and_smoke(env):
    cb = load_cb(env)
    expected = {
        "arms_toml": ROOT / "config" / "arms.toml",
        "suite_b": ROOT / "suites" / "b_mini.toml",
        "suite_c": ROOT / "suites" / "c_redteam.toml",
        "suite_smoke": ROOT / "suites" / "smoke.toml",
    }
    for key, path in expected.items():
        assert cb["files_sha256"][key] == hashlib.sha256(path.read_bytes()).hexdigest(), key


def test_k11_bundle_manifest_should_be_recorded(env):
    cb = load_cb(env)
    assert cb["bundle"] == env["manifest"]


def test_k11_analysis_should_cover_h3_h4_and_the_holm_family_for_c(env):
    cb = load_cb(env)
    text = flat(cb["analysis"])
    for needle in ("H3", "H4", "Holm", "hone-guarded", "pi-guarded", "hermes-guarded"):
        assert needle in text, needle
    assert "escape" in text.lower()
    assert "8/10" in text


# ---- K12 placeholders -------------------------------------------------------------------


@pytest.mark.parametrize("stage", ["cb", "a"])
def test_k12_placeholder_should_be_refused_without_draft(env, stage):
    sel = env["tmp"] / "selection.json"
    sel.write_text("{}")
    args = ["--stage", stage, "--out", str(env["tmp"] / "p.toml"), "--env-file", str(EXAMPLE)]
    if stage == "a":
        args += ["--selection", str(sel)]
    with pytest.raises(SystemExit) as exc:
        prereg.main(args)
    assert "HC_MODEL_REVISION" in str(exc.value)


@pytest.mark.parametrize("stage", ["cb", "a"])
def test_k12_draft_should_allow_placeholders_and_say_so(env, stage):
    sel = env["tmp"] / "selection.json"
    sel.write_text("{}")
    out = env["tmp"] / "p.toml"
    args = ["--stage", stage, "--out", str(out), "--env-file", str(EXAMPLE), "--draft"]
    if stage == "a":
        args += ["--selection", str(sel)]
    assert prereg.main(args) == 0
    assert tomllib.loads(out.read_text())["draft"] is True


def test_k12_pinned_values_should_not_be_a_draft(env):
    cb = load_cb(env)
    assert cb["draft"] is False


# ---- K13 title / K14 no stale text ------------------------------------------------------


def test_k13_title(env):
    cb = load_cb(env)
    assert cb["title"] == "Hone vs Pi vs Hermes Agent, harness-controlled, one local model"


def test_k14_stage_cb_analysis_should_not_mention_n40_or_swe_bench(env):
    cb = load_cb(env)
    text = flat({"analysis": cb["analysis"], "title": cb["title"]}).lower()
    assert "n=40" not in text
    assert "swe-bench" not in text
    assert "swebench" not in text


def test_k14_stage_cb_file_should_not_say_n_equals_40_anywhere(env):
    pin(env)
    _, out = run_cb(env)
    assert "n=40" not in out.read_text()


# ---- K15 stage a unchanged ---------------------------------------------------------------


@pytest.fixture
def stage_a(env):
    pin(env)
    sel = env["tmp"] / "selection.json"
    sel.write_text(json.dumps({"dataset": "d", "split": "s", "n": 3, "seed": 9, "digest": "abc"}))
    (env["tmp"] / "tasks.txt").write_text("a\nb\nc\n")
    out = env["tmp"] / "pa.toml"
    return prereg.main, sel, out


@pytest.mark.parametrize("flag", [[], ["--stage", "a"]])
def test_k15_stage_a_should_write_what_it_wrote_before_plus_stage(env, stage_a, flag):
    main, sel, out = stage_a
    assert (
        main([*flag, "--selection", str(sel), "--out", str(out), "--env-file", str(EXAMPLE)]) == 0
    )
    data = tomllib.loads(out.read_text())
    assert data["stage"] == "a"
    assert data["tasks"]["dataset"] == "d"
    assert data["tasks"]["n"] == 3
    assert data["tasks"]["digest"] == "abc"
    assert data["files_sha256"]["tasks_txt"] == hashlib.sha256(b"a\nb\nc\n").hexdigest()
    assert "prompt_swebench" in data["files_sha256"]
    assert data["analysis"]["primary_metric"].startswith("pass@1 on suite A")
    for key in ("suites_in_scope", "suite_a_rules", "cut_order"):
        assert key not in data
