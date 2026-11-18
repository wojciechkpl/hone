"""Spec section 11.4, K23 (proxy) and K30 (thinking key overridden)."""

import asyncio

import pytest
from fakes import FakeUpstream, proxy_stack

RUN = "c.pi.s0.rt01"
MSGS = [{"role": "user", "content": "hi"}]
THINKING_KEYS = ["reasoning_effort", "reasoning", "thinking", "enable_thinking"]


async def one_call(tmp_path, body, overrides):
    up = FakeUpstream()
    async with proxy_stack(tmp_path, up, overrides) as st:
        await st.register(RUN, seed=1)
        status, _ = await st.chat(RUN, {"messages": MSGS, **body})
        assert status == 200
        _, records = await st.records(RUN)
    return up.calls[0], records[0]


def run(coro):
    return asyncio.run(coro)


# ---- K23 chat_template_kwargs removed unless overridden ---------------------------------


def test_k23_chat_template_kwargs_without_an_override_should_be_removed(tmp_path):
    sent, rec = run(
        one_call(
            tmp_path, {"chat_template_kwargs": {"enable_thinking": True}}, {"temperature": 0.7}
        )
    )
    assert "chat_template_kwargs" not in sent
    assert "chat_template_kwargs" not in rec["sampling"]


def test_k23_chat_template_kwargs_with_an_override_should_carry_the_override(tmp_path):
    sent, _ = run(
        one_call(
            tmp_path,
            {"chat_template_kwargs": {"enable_thinking": True, "x": 1}},
            {"chat_template_kwargs": {"enable_thinking": False}},
        )
    )
    assert sent["chat_template_kwargs"] == {"enable_thinking": False}


# ---- K23 extra_keys ---------------------------------------------------------------------


def test_k23_record_should_list_unknown_top_level_keys_sorted(tmp_path):
    _, rec = run(one_call(tmp_path, {"zeta_knob": 1, "alpha_knob": {"a": 1}}, {}))
    assert rec["extra_keys"] == ["alpha_knob", "zeta_knob"]


def test_k23_unknown_keys_should_still_be_forwarded(tmp_path):
    sent, _ = run(one_call(tmp_path, {"zeta_knob": 1}, {}))
    assert sent["zeta_knob"] == 1


def test_k23_ordinary_openai_keys_should_not_count_as_extra(tmp_path):
    body = {
        "stream": False,
        "tools": [],
        "tool_choice": "auto",
        "temperature": 0.2,
        "top_p": 1,
        "max_tokens": 5,
        "stop": ["x"],
        "n": 1,
        "seed": 3,
        "model": "m",
    }
    _, rec = run(one_call(tmp_path, body, {}))
    assert rec["extra_keys"] == []


def test_k23_record_without_unknown_keys_should_have_an_empty_extra_keys_list(tmp_path):
    _, rec = run(one_call(tmp_path, {}, {"temperature": 0.7}))
    assert rec["extra_keys"] == []


# ---- K30: a thinking key that IS overridden carries its override ------------------------


@pytest.mark.parametrize("key", THINKING_KEYS)
def test_k30_k6_overridden_thinking_key_should_be_kept_with_the_override_value(tmp_path, key):
    sent, _ = run(one_call(tmp_path, {key: "harness-value"}, {key: "forced"}))
    assert sent[key] == "forced"


@pytest.mark.parametrize("key", THINKING_KEYS)
def test_k30_k6_other_thinking_keys_should_still_go_when_one_is_overridden(tmp_path, key):
    others = {k: "x" for k in THINKING_KEYS if k != key}
    sent, _ = run(one_call(tmp_path, others, {key: "forced"}))
    assert sent[key] == "forced"
    assert not set(others) & set(sent)
