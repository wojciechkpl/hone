"""Spec section 11.2, K4: scripts/02_serve_model.sh passes the campaign 1 vLLM flags."""

import json

from fakebin import SHA40, VLLM_PINNED, docker_calls, make_fake_bin, make_rig, run_script

SPEC = '{"method":"mtp","num_speculative_tokens":3}'
BASE = {
    "HC_SERVER": "vllm",
    "HC_VLLM_IMAGE": VLLM_PINNED,
    "HC_MODEL_HF": "org/model",
    # Pinned so the script never expands an empty array: macOS bash 3.2 + `set -u` dies on that.
    "HC_MODEL_REVISION": SHA40,
    "HC_ENFORCE_EAGER": "0",
    "HC_LANGUAGE_MODEL_ONLY": "0",
    "HC_REASONING_PARSER": "",
    "HC_SPECULATIVE_CONFIG": "",
    "HC_ENABLE_THINKING": "true",
}


def serve(tmp_path, **env):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(rig, "02_serve_model.sh", ["up"], bindir, {**BASE, **env})
    runs = [c for c in docker_calls(log) if c[:1] == ["run"] and "hc-llm" in c]
    return r, runs


def run_argv(tmp_path, **env):
    r, runs = serve(tmp_path, **env)
    assert r.returncode == 0, r.stderr
    assert len(runs) == 1, runs
    argv = runs[0]
    return argv[argv.index(VLLM_PINNED) + 1 :]  # arguments handed to vLLM


def test_k4_flags_should_be_absent_when_switched_off(tmp_path):
    args = run_argv(tmp_path)
    for flag in (
        "--enforce-eager",
        "--language-model-only",
        "--reasoning-parser",
        "--speculative-config",
        "--default-chat-template-kwargs",
    ):
        assert flag not in args


def test_k4_enforce_eager_when_set_should_add_the_flag(tmp_path):
    assert "--enforce-eager" in run_argv(tmp_path, HC_ENFORCE_EAGER="1")


def test_k4_language_model_only_when_set_should_add_the_flag(tmp_path):
    assert "--language-model-only" in run_argv(tmp_path, HC_LANGUAGE_MODEL_ONLY="1")


def test_k4_reasoning_parser_when_set_should_pass_its_value(tmp_path):
    args = run_argv(tmp_path, HC_REASONING_PARSER="qwen3")
    assert args[args.index("--reasoning-parser") + 1] == "qwen3"


def test_k4_speculative_config_should_reach_vllm_as_one_json_argument(tmp_path):
    args = run_argv(tmp_path, HC_SPECULATIVE_CONFIG=SPEC)
    value = args[args.index("--speculative-config") + 1]
    assert json.loads(value) == {"method": "mtp", "num_speculative_tokens": 3}


def test_k4_chat_template_kwargs_when_thinking_false_should_be_one_json_argument(tmp_path):
    args = run_argv(tmp_path, HC_ENABLE_THINKING="false")
    value = args[args.index("--default-chat-template-kwargs") + 1]
    assert json.loads(value) == {"enable_thinking": False}


def test_k4_chat_template_kwargs_when_thinking_not_false_should_be_absent(tmp_path):
    assert "--default-chat-template-kwargs" not in run_argv(tmp_path, HC_ENABLE_THINKING="true")


def test_k4_every_existing_flag_should_stay(tmp_path):
    args = run_argv(tmp_path, HC_ENFORCE_EAGER="1", HC_REASONING_PARSER="qwen3")
    for flag in (
        "--model",
        "--served-model-name",
        "--max-model-len",
        "--gpu-memory-utilization",
        "--kv-cache-dtype",
        "--max-num-seqs",
        "--enable-auto-tool-choice",
        "--tool-call-parser",
        "--generation-config",
        "--seed",
    ):
        assert flag in args, flag
    assert args[args.index("--generation-config") + 1] == "vllm"


def test_k4_example_defaults_should_produce_the_campaign_1_command(tmp_path):
    # No overrides: the flags come straight from config/bench.env.example (K1 + K2).
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(
        rig,
        "02_serve_model.sh",
        ["up"],
        bindir,
        {"HC_VLLM_IMAGE": VLLM_PINNED, "HC_MODEL_REVISION": SHA40},
    )
    assert r.returncode == 0, r.stderr
    run = next(c for c in docker_calls(log) if c[:1] == ["run"] and "hc-llm" in c)
    args = run[run.index(VLLM_PINNED) + 1 :]
    assert args[args.index("--model") + 1] == "Inferact/Qwen3.8-27B-NVFP4"
    assert args[args.index("--served-model-name") + 1] == "qwen3.8-27b"
    assert args[args.index("--reasoning-parser") + 1] == "qwen3"
    assert "--enforce-eager" in args
    assert "--language-model-only" in args
    assert json.loads(args[args.index("--speculative-config") + 1]) == {
        "method": "mtp",
        "num_speculative_tokens": 3,
    }
    assert json.loads(args[args.index("--default-chat-template-kwargs") + 1]) == {
        "enable_thinking": False
    }


def test_k4_revision_flag_should_still_be_passed_when_pinned(tmp_path):
    args = run_argv(tmp_path, HC_ENFORCE_EAGER="1")
    assert args[args.index("--revision") + 1] == SHA40
