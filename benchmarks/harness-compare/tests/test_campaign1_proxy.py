"""Spec section 11.2, K6-K8: the proxy replaces sampling, logs it, and the seed is base + repeat."""

import asyncio
import json

import pytest
from fakes import FakeUpstream, proxy_stack

RUN = "c.pi.s0.rt01"
MSGS = [{"role": "user", "content": "hi"}]

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
LOGGED_KEYS = {
    "model", "temperature", "top_p", "top_k", "min_p", "presence_penalty",
    "repetition_penalty", "max_tokens", "seed", "n", "chat_template_kwargs",
}  # fmt: skip
HARNESS_SAMPLING = {
    "model": "whatever",
    "temperature": 0,
    "top_p": 1,
    "top_k": 5,
    "min_p": 0.3,
    "presence_penalty": 0,
    "repetition_penalty": 1.3,
    "max_tokens": 99,
}


def run(coro):
    return asyncio.run(coro)


async def one_call(tmp_path, body, overrides, seed=None):
    up = FakeUpstream()
    async with proxy_stack(tmp_path, up, overrides) as st:
        await st.register(RUN, **({"seed": seed} if seed is not None else {}))
        status, _ = await st.chat(RUN, {"messages": MSGS, **body})
        assert status == 200
        _, records = await st.records(RUN)
    return up.calls[0], records[0]


# ---- K6 replace, never merge ------------------------------------------------------------


def test_k6_every_overridden_key_should_carry_the_override_whatever_the_harness_sent(tmp_path):
    sent, _ = run(one_call(tmp_path, dict(HARNESS_SAMPLING), FORCED))
    for key, value in FORCED.items():
        assert sent[key] == value, key


def test_k6_harness_chat_template_kwargs_should_be_replaced_whole_not_merged(tmp_path):
    body = {
        "chat_template_kwargs": {"enable_thinking": True, "preserve_thinking": True, "x": 1},
        "enable_thinking": True,
    }
    sent, _ = run(one_call(tmp_path, body, FORCED))
    assert sent["chat_template_kwargs"] == {"enable_thinking": False}
    assert "enable_thinking" not in sent


@pytest.mark.parametrize("key", ["reasoning_effort", "reasoning", "thinking", "enable_thinking"])
@pytest.mark.parametrize("overrides", [FORCED, {}])
def test_k6_thinking_knobs_should_be_removed_unless_overridden(tmp_path, key, overrides):
    value = {"effort": "high"} if key in ("reasoning", "thinking") else "high"
    sent, _ = run(one_call(tmp_path, {key: value}, overrides))
    assert key not in sent


@pytest.mark.parametrize(
    "key",
    [
        "top_k", "min_p", "repetition_penalty", "presence_penalty", "frequency_penalty",
        "logit_bias", "typical_p", "best_of", "use_beam_search", "length_penalty",
    ],
)  # fmt: skip
def test_k6_todays_stripped_keys_should_stay_stripped_when_not_overridden(tmp_path, key):
    sent, _ = run(one_call(tmp_path, {key: 3}, {"temperature": 0.7}))
    assert key not in sent


def test_k6_a_key_in_both_the_strip_list_and_the_overrides_should_carry_the_override(tmp_path):
    sent, _ = run(one_call(tmp_path, {"presence_penalty": 0}, {"presence_penalty": 1.5}))
    assert sent["presence_penalty"] == 1.5


def test_k6_n_should_be_one(tmp_path):
    sent, _ = run(one_call(tmp_path, {"n": 4, "reasoning_effort": "low"}, FORCED))
    assert sent["n"] == 1
    assert "reasoning_effort" not in sent


def test_k6_max_completion_tokens_should_not_survive_a_max_tokens_override(tmp_path):
    sent, _ = run(one_call(tmp_path, {"max_completion_tokens": 50, "thinking": {}}, FORCED))
    assert sent["max_tokens"] == 6144
    assert "max_completion_tokens" not in sent


# ---- K7 logged --------------------------------------------------------------------------


def test_k7_record_sampling_should_hold_all_eleven_effective_values(tmp_path):
    sent, rec = run(one_call(tmp_path, dict(HARNESS_SAMPLING), FORCED, seed=20261119))
    assert set(rec["sampling"]) == LOGGED_KEYS
    assert rec["sampling"] == {**FORCED, "seed": 20261119, "n": 1}
    for key, value in rec["sampling"].items():
        assert sent[key] == value


def test_k7_record_sampling_should_list_only_keys_the_forwarded_body_contains(tmp_path):
    sent, rec = run(
        one_call(tmp_path, {"top_k": 5, "reasoning_effort": "high"}, {"temperature": 0.7})
    )
    assert set(rec["sampling"]) == (set(sent) & LOGGED_KEYS)
    assert rec["sampling"] == {"temperature": 0.7, "n": 1}


def test_k7_record_sampling_should_log_streaming_requests_too(tmp_path):
    async def go():
        up = FakeUpstream(sse_chunks=[{"choices": [{"index": 0, "delta": {"content": "x"}}]}])
        async with proxy_stack(tmp_path, up, FORCED) as st:
            await st.register(RUN, seed=5)
            await st.chat(RUN, {"messages": MSGS, "stream": True, "top_k": 1})
            return (await st.records(RUN))[1][0], up.calls[0]

    rec, sent = run(go())
    assert rec["sampling"] == {**FORCED, "seed": 5, "n": 1}
    assert sent["top_k"] == 20


# ---- K8 seed (proxy side) ---------------------------------------------------------------


def test_k8_registered_seed_should_be_sent_upstream_and_logged(tmp_path):
    sent, rec = run(one_call(tmp_path, {"seed": 1}, FORCED, seed=20261118))
    assert sent["seed"] == 20261118
    assert rec["sampling"]["seed"] == 20261118


def test_k8_forced_values_should_be_json_serialisable_in_the_record_file(tmp_path):
    async def go():
        up = FakeUpstream()
        async with proxy_stack(tmp_path, up, FORCED) as st:
            await st.register(RUN, seed=3)
            await st.chat(RUN, {"messages": MSGS})
        return (tmp_path / "logs" / f"{RUN}.jsonl").read_text()

    line = run(go()).splitlines()[0]
    assert json.loads(line)["sampling"]["chat_template_kwargs"] == {"enable_thinking": False}
