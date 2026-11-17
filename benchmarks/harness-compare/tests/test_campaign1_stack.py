"""Spec section 11.2, K5: scripts/03_stack.sh starts the proxy with every forced value."""

import json

from fakebin import docker_calls, make_fake_bin, make_rig, run_script

FORCED = {
    "model": "qwen3.8-27b",
    "temperature": 0.7,
    "top_p": 0.8,
    "top_k": 20,
    "min_p": 0,
    "presence_penalty": 1.5,
    "repetition_penalty": 1.0,
    "max_tokens": 6144,
    "chat_template_kwargs": {"enable_thinking": False},
}


def stack_overrides(tmp_path, **env):
    """Run `03_stack.sh up` against fakes; return the proxy's --override pairs, parsed like
    hcbench.proxy does (key=JSON)."""
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(rig, "03_stack.sh", ["up"], bindir, env)
    assert r.returncode == 0, r.stderr
    run = next(c for c in docker_calls(log) if "hcbench.proxy" in c)
    pairs = [run[i + 1] for i, a in enumerate(run) if a == "--override"]
    out = {}
    for item in pairs:
        key, _, value = item.partition("=")
        try:
            out[key] = json.loads(value)
        except json.JSONDecodeError:
            out[key] = value
    return out


def test_k5_stack_should_force_every_campaign_1_value_on_the_proxy(tmp_path):
    assert stack_overrides(tmp_path) == FORCED


def test_k5_overrides_should_come_from_config_not_be_hardcoded(tmp_path):
    got = stack_overrides(
        tmp_path,
        HC_TEMPERATURE="0.3",
        HC_TOP_K="7",
        HC_MIN_P="0.05",
        HC_PRESENCE_PENALTY="0.5",
        HC_REPETITION_PENALTY="1.1",
        HC_MAX_TOKENS="1000",
    )
    assert got["temperature"] == 0.3
    assert got["top_k"] == 7
    assert got["min_p"] == 0.05
    assert got["presence_penalty"] == 0.5
    assert got["repetition_penalty"] == 1.1
    assert got["max_tokens"] == 1000


def test_k5_chat_template_kwargs_should_be_one_json_object_argument(tmp_path):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(rig, "03_stack.sh", ["up"], bindir, {})
    assert r.returncode == 0, r.stderr
    run = next(c for c in docker_calls(log) if "hcbench.proxy" in c)
    items = [a for a in run if a.startswith("chat_template_kwargs=")]
    assert len(items) == 1
    assert json.loads(items[0].partition("=")[2]) == {"enable_thinking": False}
