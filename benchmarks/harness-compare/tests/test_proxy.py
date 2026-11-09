"""Spec section 10: hcbench.proxy (metering reverse proxy)."""

import asyncio
import json

import pytest
from fakes import FakeUpstream, proxy_stack

import hcbench.proxy as proxy_mod

RUN = "c.pi.s0.rt01"
MSGS = [{"role": "system", "content": "sys"}, {"role": "user", "content": "hi"}]


def body(messages=None, **extra):
    return {"model": "m", "messages": list(messages or MSGS), **extra}


def data_payloads(text: str) -> list:
    """Payload strings of the SSE ``data:`` lines in order."""
    return [ln[len("data:") :].strip() for ln in text.splitlines() if ln.startswith("data:")]


# ---- fixtures for streaming -------------------------------------------

C_TEXT1 = {"choices": [{"index": 0, "delta": {"role": "assistant", "content": "Let me "}}]}
C_TEXT2 = {"choices": [{"index": 0, "delta": {"content": "look"}}]}
C_TC0_A = {
    "choices": [
        {
            "index": 0,
            "delta": {
                "tool_calls": [
                    {
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "bash", "arguments": '{"comm'},
                    }
                ]
            },
        }
    ]
}
C_TC1 = {
    "choices": [
        {
            "index": 0,
            "delta": {
                "tool_calls": [
                    {
                        "index": 1,
                        "id": "call_2",
                        "type": "function",
                        "function": {"name": "read", "arguments": "{}"},
                    }
                ]
            },
        }
    ]
}
C_TC0_B = {
    "choices": [
        {
            "index": 0,
            "delta": {"tool_calls": [{"index": 0, "function": {"arguments": 'and": "ls"}'}}]},
        }
    ]
}
C_FINISH = {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}
C_USAGE = {
    "choices": [],
    "usage": {"prompt_tokens": 900, "completion_tokens": 40, "total_tokens": 940},
}
STREAM_CHUNKS = [C_TEXT1, C_TEXT2, C_TC0_A, C_TC1, C_TC0_B, C_FINISH, C_USAGE]


def run(coro):
    return asyncio.run(coro)


# ---- admin / routing ---------------------------------------------------


def test_register_run_should_return_201_with_run_id(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            return await st.register(RUN)

    status, payload = run(go())
    assert status == 201
    assert payload == {"run_id": RUN}


def test_records_when_run_unknown_should_return_404(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            return await st.records("nope.x.s0.t")

    status, _ = run(go())
    assert status == 404


def test_records_when_run_registered_but_idle_should_be_empty_list(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            await st.register(RUN)
            return await st.records(RUN)

    assert run(go()) == (200, [])


def test_chat_when_run_unknown_should_return_404_unknown_run_without_upstream_call(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            return await st.chat("ghost.x.s0.t", body())

    status, text = run(go())
    assert status == 404
    assert json.loads(text)["error"]["type"] == "unknown_run"
    assert "message" in json.loads(text)["error"]
    assert up.calls == []


def test_models_should_be_passed_through_to_upstream(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            async with st.session.get(f"{st.base}/run/{RUN}/v1/models") as r:
                return r.status, await r.json()

    status, payload = run(go())
    assert status == 200
    assert payload["data"][0]["id"] == "fake-model"
    assert up.models_calls == 1


# ---- non-streaming -----------------------------------------------------


def test_chat_non_streaming_should_return_upstream_body_and_status_unchanged(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            return await st.chat(RUN, body())

    status, text = run(go())
    assert status == 200
    assert json.loads(text) == up.json_body
    assert len(up.calls) == 1


def test_chat_non_streaming_when_upstream_errors_should_pass_status_through_and_record_it(tmp_path):
    up = FakeUpstream(json_body={"error": {"message": "overloaded"}}, status=503)

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            status, text = await st.chat(RUN, body())
            return status, json.loads(text), (await st.records(RUN))[1]

    status, payload, records = run(go())
    assert status == 503
    assert payload == {"error": {"message": "overloaded"}}
    assert records[0]["http_status"] == 503


def test_chat_non_streaming_should_record_usage_text_and_tool_calls(tmp_path):
    up = FakeUpstream(
        json_body={
            "choices": [
                {
                    "index": 0,
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": "ok",
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {"name": "bash", "arguments": '{"command": "ls"}'},
                            }
                        ],
                    },
                }
            ],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
        }
    )

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            await st.chat(RUN, body())
            return (await st.records(RUN))[1]

    (r,) = run(go())
    assert r["run_id"] == RUN
    assert r["seq"] == 0
    assert r["http_status"] == 200
    assert r["stream"] is False
    assert r["prompt_tokens"] == 11
    assert r["completion_tokens"] == 7
    assert r["finish_reason"] == "tool_calls"
    assert r["tool_calls"] == [{"id": "call_1", "name": "bash", "arguments": '{"command": "ls"}'}]
    assert r["budget_exceeded"] is False
    assert r["ts_end"] >= r["ts_start"]


# ---- budget ------------------------------------------------------------


def test_budget_max_requests_exhausted_should_return_400_and_not_contact_upstream(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN, max_requests=1)
            first = await st.chat(RUN, body())
            second = await st.chat(RUN, body())
            third = await st.chat(RUN, body())
            return first, second, third, (await st.records(RUN))[1]

    first, second, third, records = run(go())
    assert first[0] == 200
    for status, text in (second, third):
        assert status == 400
        assert json.loads(text) == {
            "error": {"message": "hcbench budget exhausted", "type": "budget_exceeded"}
        }
    assert len(up.calls) == 1
    assert [r["budget_exceeded"] for r in records] == [False, True, True]
    assert records[1]["http_status"] == 400


def test_budget_max_total_tokens_exhausted_should_block_next_request(tmp_path):
    up = FakeUpstream()  # each call uses 10 + 5 = 15 tokens

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN, max_total_tokens=15)
            first = await st.chat(RUN, body())
            second = await st.chat(RUN, body())
            return first, second, (await st.records(RUN))[1]

    first, second, records = run(go())
    assert first[0] == 200
    assert second[0] == 400
    assert json.loads(second[1])["error"]["type"] == "budget_exceeded"
    assert len(up.calls) == 1
    assert records[1]["budget_exceeded"] is True


def test_budget_when_tokens_below_limit_should_still_forward(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN, max_total_tokens=16, max_requests=5)
            return [await st.chat(RUN, body()) for _ in range(2)]

    results = run(go())
    assert [s for s, _ in results] == [200, 200]
    assert len(up.calls) == 2


# ---- overrides and seed ------------------------------------------------


def test_overrides_should_shallow_update_request_body_sent_upstream(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up, overrides={"temperature": 0, "top_p": 0.9}) as st:
            await st.register(RUN)
            await st.chat(RUN, body(temperature=1.0, max_tokens=64))

    run(go())
    sent = up.calls[0]
    assert sent["temperature"] == 0
    assert sent["top_p"] == 0.9
    assert sent["max_tokens"] == 64
    assert sent["model"] == "m"
    assert sent["messages"] == MSGS


def test_registered_seed_should_take_precedence_over_override_seed_and_client_seed(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up, overrides={"seed": 99}) as st:
            await st.register(RUN, seed=7)
            await st.chat(RUN, body(seed=1))

    run(go())
    assert up.calls[0]["seed"] == 7


def test_override_seed_should_apply_when_run_has_no_registered_seed(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up, overrides={"seed": 99}) as st:
            await st.register(RUN)
            await st.chat(RUN, body())

    run(go())
    assert up.calls[0]["seed"] == 99


# ---- streaming ---------------------------------------------------------


def stream_up():
    return FakeUpstream(sse_chunks=list(STREAM_CHUNKS))


def test_streaming_when_client_did_not_ask_usage_should_force_it_upstream_and_strip_chunk(tmp_path):
    up = stream_up()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            return await st.chat(RUN, body(stream=True))

    status, text = run(go())
    assert status == 200
    assert up.calls[0]["stream_options"]["include_usage"] is True
    payloads = data_payloads(text)
    assert payloads[-1] == "[DONE]"
    assert [json.loads(p) for p in payloads[:-1]] == [
        C_TEXT1,
        C_TEXT2,
        C_TC0_A,
        C_TC1,
        C_TC0_B,
        C_FINISH,
    ]
    assert all(json.loads(p).get("choices") != [] for p in payloads[:-1])


def test_streaming_when_client_asked_usage_should_forward_usage_chunk(tmp_path):
    up = stream_up()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            return await st.chat(RUN, body(stream=True, stream_options={"include_usage": True}))

    _, text = run(go())
    assert up.calls[0]["stream_options"]["include_usage"] is True
    payloads = data_payloads(text)
    assert payloads[-1] == "[DONE]"
    assert [json.loads(p) for p in payloads[:-1]] == STREAM_CHUNKS


def test_streaming_should_record_usage_text_finish_and_reassembled_tool_calls(tmp_path):
    up = stream_up()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            await st.chat(RUN, body(stream=True))
            return (await st.records(RUN))[1]

    (r,) = run(go())
    assert r["stream"] is True
    assert r["http_status"] == 200
    assert r["prompt_tokens"] == 900
    assert r["completion_tokens"] == 40
    assert r["finish_reason"] == "tool_calls"
    assert r["assistant_text"] == "Let me look"
    assert r["tool_calls"] == [
        {"id": "call_1", "name": "bash", "arguments": '{"command": "ls"}'},
        {"id": "call_2", "name": "read", "arguments": "{}"},
    ]


def test_streaming_usage_should_count_toward_budget(tmp_path):
    up = stream_up()  # 940 tokens per call

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN, max_total_tokens=900)
            first = await st.chat(RUN, body(stream=True))
            second = await st.chat(RUN, body(stream=True))
            return first[0], second[0]

    assert run(go()) == (200, 400)
    assert len(up.calls) == 1


# ---- records: deltas, resets, persistence ------------------------------


def test_records_should_compute_messages_delta_and_reset_across_requests(tmp_path):
    up = FakeUpstream()
    turn1 = list(MSGS)
    turn2 = [
        *turn1,
        {"role": "assistant", "content": "calling"},
        {"role": "tool", "tool_call_id": "c1", "content": "out"},
    ]
    compacted = [{"role": "system", "content": "sys"}, {"role": "user", "content": "summary"}]

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            for msgs in (turn1, turn2, compacted):
                await st.chat(RUN, body(msgs))
            return (await st.records(RUN))[1]

    r0, r1, r2 = run(go())
    assert [r["seq"] for r in (r0, r1, r2)] == [0, 1, 2]
    assert r0["messages_delta"] == turn1
    assert r0["messages_reset"] is False
    assert r1["messages_delta"] == turn2[2:]
    assert r1["messages_reset"] is False
    assert r2["messages_delta"] == compacted
    assert r2["messages_reset"] is True


def test_records_when_messages_unchanged_between_requests_should_have_empty_delta(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            await st.register(RUN)
            await st.chat(RUN, body())
            await st.chat(RUN, body())
            return (await st.records(RUN))[1]

    _, r1 = run(go())
    assert r1["messages_delta"] == []
    assert r1["messages_reset"] is False


def test_records_should_be_written_to_jsonl_file_named_after_run(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            await st.register(RUN)
            await st.chat(RUN, body())
            await st.chat(RUN, body())
            return (await st.records(RUN))[1], st.log_dir

    records, log_dir = run(go())
    lines = (log_dir / f"{RUN}.jsonl").read_text().splitlines()
    assert [json.loads(line) for line in lines] == records
    assert len(lines) == 2


def test_budget_exceeded_record_should_also_be_persisted_to_jsonl(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            await st.register(RUN, max_requests=0)
            await st.chat(RUN, body())
            return st.log_dir

    log_dir = run(go())
    (line,) = (log_dir / f"{RUN}.jsonl").read_text().splitlines()
    assert json.loads(line)["budget_exceeded"] is True


def test_reregistering_run_should_clear_records_seq_and_budget_state(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN, max_requests=1)
            await st.chat(RUN, body())
            blocked = await st.chat(RUN, body())
            await st.register(RUN, max_requests=1)
            cleared = await st.records(RUN)
            again = await st.chat(RUN, body())
            return blocked[0], cleared, again[0], (await st.records(RUN))[1]

    blocked, cleared, again, records = run(go())
    assert blocked == 400
    assert cleared == (200, [])
    assert again == 200
    assert len(records) == 1
    assert records[0]["seq"] == 0
    assert records[0]["messages_reset"] is False
    assert records[0]["messages_delta"] == MSGS


def test_runs_should_be_isolated_from_each_other(tmp_path):
    other = "c.pi.s1.rt01"

    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            await st.register(RUN, max_requests=1)
            await st.register(other)
            await st.chat(RUN, body())
            await st.chat(other, body())
            await st.chat(other, body())
            return (await st.records(RUN))[1], (await st.records(other))[1]

    first, second = run(go())
    assert len(first) == 1
    assert [r["seq"] for r in second] == [0, 1]
    assert all(not r["budget_exceeded"] for r in second)


# ---- admin token -------------------------------------------------------

TOKEN = "s3cr3t-admin"


def test_admin_token_when_missing_or_wrong_should_return_401_unauthorized(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream(), admin_token=TOKEN) as st:
            out = []
            for headers in ({}, {"Authorization": "Bearer wrong"}, {"Authorization": TOKEN}):
                async with st.session.post(
                    f"{st.base}/admin/runs", json={"run_id": RUN}, headers=headers
                ) as r:
                    out.append((r.status, await r.json()))
                async with st.session.get(
                    f"{st.base}/admin/runs/{RUN}/records", headers=headers
                ) as r:
                    out.append((r.status, await r.json()))
            return out

    for status, payload in run(go()):
        assert status == 401
        assert payload["error"]["type"] == "unauthorized"
        assert "message" in payload["error"]


def test_admin_token_when_unauthorized_register_should_not_register_run(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream(), admin_token=TOKEN) as st:
            async with st.session.post(f"{st.base}/admin/runs", json={"run_id": RUN}) as r:
                assert r.status == 401
            return await st.chat(RUN, body())

    status, text = run(go())
    assert status == 404
    assert json.loads(text)["error"]["type"] == "unknown_run"


def test_admin_token_unauthorized_reregistration_should_leave_state_untouched(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up, admin_token=TOKEN) as st:
            await st.register(RUN, max_requests=1)
            await st.chat(RUN, body())
            # a hostile arm container tries to reset its own budget and records
            async with st.session.post(
                f"{st.base}/admin/runs", json={"run_id": RUN, "max_requests": 100}
            ) as r:
                denied = r.status
            blocked = await st.chat(RUN, body())
            return denied, blocked[0], (await st.records(RUN))[1]

    denied, blocked, records = run(go())
    assert denied == 401
    assert blocked == 400
    assert [r["budget_exceeded"] for r in records] == [False, True]


def test_admin_token_correct_token_should_allow_register_and_records(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream(), admin_token=TOKEN) as st:
            registered = await st.register(RUN)
            await st.chat(RUN, body())
            return registered, await st.records(RUN)

    registered, (status, records) = run(go())
    assert registered[0] == 201
    assert status == 200
    assert len(records) == 1


def test_admin_token_should_not_be_required_for_chat_or_models_routes(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream(), admin_token=TOKEN) as st:
            await st.register(RUN)
            chat = await st.chat(RUN, body())  # st.chat sends no Authorization header
            async with st.session.get(f"{st.base}/run/{RUN}/v1/models") as r:
                return chat[0], r.status

    assert run(go()) == (200, 200)


def test_admin_token_when_none_should_leave_admin_routes_open(tmp_path):
    async def go():
        async with (
            proxy_stack(tmp_path, FakeUpstream()) as st,
            st.session.post(
                f"{st.base}/admin/runs",
                json={"run_id": RUN},
                headers={"Authorization": "Bearer whatever"},
            ) as r,
        ):
            return r.status

    assert run(go()) == 201


# ---- sampling lockdown -------------------------------------------------


def test_dropped_sampling_keys_should_match_spec():
    assert (
        frozenset(
            {
                "top_k",
                "min_p",
                "repetition_penalty",
                "presence_penalty",
                "frequency_penalty",
                "logit_bias",
                "typical_p",
                "best_of",
                "use_beam_search",
                "length_penalty",
            }
        )
        == proxy_mod.DROPPED_SAMPLING_KEYS
    )


def test_sampling_lockdown_should_strip_every_dropped_key_from_upstream_body(tmp_path):
    up = FakeUpstream()
    extras = {k: 1 for k in sorted(proxy_mod.DROPPED_SAMPLING_KEYS)}

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            await st.chat(RUN, body(**extras))

    run(go())
    assert not (set(up.calls[0]) & proxy_mod.DROPPED_SAMPLING_KEYS)


def test_sampling_lockdown_should_keep_dropped_key_that_is_in_overrides(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up, overrides={"top_k": 20}) as st:
            await st.register(RUN)
            await st.chat(RUN, body(top_k=5, min_p=0.1, presence_penalty=1.0))

    run(go())
    sent = up.calls[0]
    assert sent["top_k"] == 20
    assert "min_p" not in sent
    assert "presence_penalty" not in sent


@pytest.mark.parametrize("client_n", [None, 1, 4])
def test_sampling_lockdown_should_force_n_to_one(tmp_path, client_n):
    up = FakeUpstream()
    extra = {} if client_n is None else {"n": client_n}

    async def go():
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            await st.chat(RUN, body(**extra))

    run(go())
    assert up.calls[0]["n"] == 1


def test_sampling_lockdown_should_drop_max_completion_tokens_only_when_max_tokens_overridden(
    tmp_path,
):
    up_a, up_b = FakeUpstream(), FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path / "a", up_a, overrides={"max_tokens": 256}) as st:
            await st.register(RUN)
            await st.chat(RUN, body(max_completion_tokens=999))
        async with proxy_stack(tmp_path / "b", up_b, overrides={"temperature": 0}) as st:
            await st.register(RUN)
            await st.chat(RUN, body(max_completion_tokens=999))

    run(go())
    assert up_a.calls[0]["max_tokens"] == 256
    assert "max_completion_tokens" not in up_a.calls[0]
    assert up_b.calls[0]["max_completion_tokens"] == 999


def test_sampling_lockdown_should_pass_other_keys_through_unchanged(tmp_path):
    up = FakeUpstream()
    tools = [{"type": "function", "function": {"name": "bash", "parameters": {"type": "object"}}}]
    extras = {
        "tools": tools,
        "tool_choice": "auto",
        "stop": ["END"],
        "chat_template_kwargs": {"enable_thinking": False},
    }

    async def go():
        async with proxy_stack(tmp_path, up, overrides={"temperature": 0}) as st:
            await st.register(RUN)
            await st.chat(RUN, body(**extras))

    run(go())
    sent = up.calls[0]
    for key, value in extras.items():
        assert sent[key] == value, key
    assert sent["messages"] == MSGS


def test_record_sampling_should_hold_exactly_present_keys_as_sent_upstream(tmp_path):
    up = FakeUpstream()

    async def go():
        async with proxy_stack(tmp_path, up, overrides={"temperature": 0}) as st:
            await st.register(RUN)
            await st.chat(RUN, body(top_p=0.8, max_tokens=100, n=3, top_k=5, stop=["x"]))
            return (await st.records(RUN))[1]

    (r,) = run(go())
    assert r["sampling"] == {
        "model": "m",
        "temperature": 0,
        "top_p": 0.8,
        "max_tokens": 100,
        "n": 1,
    }


def test_record_sampling_should_include_registered_seed(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            await st.register(RUN, seed=7)
            await st.chat(RUN, body(seed=1))
            return (await st.records(RUN))[1]

    (r,) = run(go())
    assert r["sampling"] == {"model": "m", "seed": 7, "n": 1}


def test_record_sampling_when_only_model_present_should_omit_absent_keys(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            await st.register(RUN)
            await st.chat(RUN, body())
            return (await st.records(RUN))[1]

    (r,) = run(go())
    assert r["sampling"] == {"model": "m", "n": 1}


def test_record_sampling_when_budget_blocked_should_be_null(tmp_path):
    async def go():
        async with proxy_stack(tmp_path, FakeUpstream()) as st:
            await st.register(RUN, max_requests=1)
            await st.chat(RUN, body(temperature=0.5))
            await st.chat(RUN, body(temperature=0.5))
            return (await st.records(RUN))[1]

    ok, blocked = run(go())
    assert ok["sampling"] == {"model": "m", "temperature": 0.5, "n": 1}
    assert blocked["budget_exceeded"] is True
    assert "sampling" in blocked
    assert blocked["sampling"] is None


# ---- client disconnect mid-stream -------------------------------------


def test_client_disconnect_mid_stream_should_persist_one_499_record_with_partial_data(tmp_path):
    first = {
        "choices": [
            {
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "content": "Let me ",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "bash", "arguments": '{"command": "ls"}'},
                        }
                    ],
                },
            }
        ]
    }
    later = [C_TEXT2, C_FINISH, C_USAGE]

    async def go():
        gate = asyncio.Event()
        up = FakeUpstream(sse_chunks=[first, *later], gate=gate, gate_after=1)
        async with proxy_stack(tmp_path, up) as st:
            await st.register(RUN)
            resp = await st.session.post(
                f"{st.base}/run/{RUN}/v1/chat/completions", json=body(stream=True)
            )
            await resp.content.readline()  # read the first SSE line, then walk away
            resp.close()
            await asyncio.sleep(0.1)
            gate.set()  # upstream keeps streaming; the proxy's write to the client fails
            records = []
            for _ in range(100):
                _, records = await st.records(RUN)
                if records:
                    break
                await asyncio.sleep(0.05)
            await asyncio.sleep(0.3)  # allow any (incorrect) duplicate record to appear
            _, records = await st.records(RUN)
            return records, st.log_dir

    records, log_dir = run(go())
    assert len(records) == 1
    (r,) = records
    assert r["http_status"] == 499
    assert r["error"] == "client disconnected"
    assert r["stream"] is True
    assert "Let me " in r["assistant_text"]
    assert r["tool_calls"] == [{"id": "call_1", "name": "bash", "arguments": '{"command": "ls"}'}]
    lines = (log_dir / f"{RUN}.jsonl").read_text().splitlines()
    assert len(lines) == 1
    assert json.loads(lines[0]) == r
