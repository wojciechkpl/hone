"""Spec section 5: hcbench.proxy_log."""

import pytest

from hcbench.proxy_log import GUARD_MARKERS, iter_message_texts, message_text, summarize_records


def rec(**over):
    base = {
        "run_id": "c.pi.s0.rt01",
        "seq": 0,
        "ts_start": 1.0,
        "ts_end": 2.0,
        "http_status": 200,
        "stream": False,
        "prompt_tokens": 10,
        "completion_tokens": 5,
        "finish_reason": "stop",
        "tool_calls": [],
        "assistant_text": "",
        "messages_delta": [],
        "messages_reset": False,
        "budget_exceeded": False,
        "error": None,
    }
    base.update(over)
    return base


def call(name, i=1):
    return {"id": f"call_{i}", "name": name, "arguments": "{}"}


def test_guard_markers_should_match_spec():
    assert GUARD_MARKERS == {
        "doom_loop": "[SYSTEM: DOOM LOOP DETECTED]",
        "context_full": "[SYSTEM: CONTEXT NEARLY FULL]",
        "output_truncated": "[SYSTEM: OUTPUT TRUNCATED]",
    }


# ---- message_text ------------------------------------------------------


def test_message_text_when_string_should_return_as_is():
    assert message_text({"role": "user", "content": " hi\n"}) == " hi\n"


def test_message_text_when_list_of_parts_should_join_text_parts_with_newline():
    msg = {
        "role": "user",
        "content": [
            {"type": "text", "text": "one"},
            {"type": "image_url", "image_url": {"url": "x"}},
            {"type": "text", "text": "two"},
        ],
    }
    assert message_text(msg) == "one\ntwo"


@pytest.mark.parametrize("msg", [{"role": "assistant", "content": None}, {"role": "assistant"}])
def test_message_text_when_none_or_missing_should_return_empty(msg):
    assert message_text(msg) == ""


def test_iter_message_texts_should_yield_text_for_each_delta_message_in_order():
    r = rec(
        messages_delta=[
            {"role": "user", "content": "a"},
            {"role": "assistant", "content": None},
            {"role": "tool", "tool_call_id": "c", "content": [{"type": "text", "text": "b"}]},
        ]
    )
    assert list(iter_message_texts(r)) == ["a", "", "b"]


# ---- summarize_records -------------------------------------------------


def test_summarize_records_when_empty_should_give_zeros_and_none_ratios():
    s = summarize_records([])
    for key in (
        "requests",
        "prompt_tokens",
        "completion_tokens",
        "total_tokens",
        "tool_calls",
        "read_calls",
        "edit_calls",
        "exec_calls",
        "other_calls",
        "length_finishes",
        "http_errors",
    ):
        assert s[key] == 0, key
    assert s["read_edit_ratio"] is None
    assert s["completion_tokens_per_s"] is None
    assert s["budget_exceeded"] is False
    assert s["llm_seconds"] == 0
    assert s["guard_events"] == {"doom_loop": 0, "context_full": 0, "output_truncated": 0}


def test_summarize_records_should_sum_tokens_and_treat_none_as_zero():
    s = summarize_records(
        [
            rec(prompt_tokens=100, completion_tokens=10),
            rec(prompt_tokens=None, completion_tokens=None),
            rec(prompt_tokens=50, completion_tokens=None),
        ]
    )
    assert s["requests"] == 3
    assert s["prompt_tokens"] == 150
    assert s["completion_tokens"] == 10
    assert s["total_tokens"] == 160


def test_summarize_records_should_classify_tool_calls_and_compute_read_edit_ratio():
    s = summarize_records(
        [
            rec(tool_calls=[call("read"), call("grep"), call("bash")]),
            rec(tool_calls=[call("Read"), call("edit"), call("write"), call("mystery")]),
        ]
    )
    assert s["tool_calls"] == 7
    assert s["read_calls"] == 3
    assert s["edit_calls"] == 2
    assert s["exec_calls"] == 1
    assert s["other_calls"] == 1
    assert s["read_edit_ratio"] == pytest.approx(1.5)


def test_summarize_records_when_no_edits_should_have_none_ratio():
    s = summarize_records([rec(tool_calls=[call("read")])])
    assert s["read_calls"] == 1
    assert s["read_edit_ratio"] is None


def test_summarize_records_should_count_length_finishes_http_errors_and_budget():
    s = summarize_records(
        [
            rec(finish_reason="length"),
            rec(finish_reason="length", http_status=500),
            rec(finish_reason="stop", http_status=400, budget_exceeded=True),
            rec(finish_reason=None, http_status=399),
        ]
    )
    assert s["length_finishes"] == 2
    assert s["http_errors"] == 2
    assert s["budget_exceeded"] is True


def test_summarize_records_should_compute_llm_seconds_and_throughput():
    s = summarize_records(
        [
            rec(ts_start=1.0, ts_end=2.5, completion_tokens=40),
            rec(ts_start=3.0, ts_end=4.0, completion_tokens=60),
        ]
    )
    assert s["llm_seconds"] == pytest.approx(2.5)
    assert s["completion_tokens_per_s"] == pytest.approx(40.0)


def test_summarize_records_when_zero_duration_should_have_none_throughput():
    s = summarize_records([rec(ts_start=5.0, ts_end=5.0, completion_tokens=10)])
    assert s["llm_seconds"] == 0
    assert s["completion_tokens_per_s"] is None


def test_summarize_records_should_count_guard_markers_in_any_role_and_content_shape():
    d, c, o = (GUARD_MARKERS[k] for k in ("doom_loop", "context_full", "output_truncated"))
    s = summarize_records(
        [
            rec(
                messages_delta=[
                    {"role": "tool", "tool_call_id": "x", "content": f"result {d}"},
                    {"role": "user", "content": [{"type": "text", "text": f"hint {c}"}]},
                ]
            ),
            rec(
                messages_delta=[
                    {
                        "role": "assistant",
                        "content": f"{d} and again {d}",
                    },  # one message -> one event
                    {"role": "user", "content": f"{o}"},
                    {"role": "user", "content": None},
                    {"role": "user", "content": "nothing special"},
                ]
            ),
        ]
    )
    assert s["guard_events"] == {"doom_loop": 2, "context_full": 1, "output_truncated": 1}


def test_summarize_records_should_accept_a_generator():
    s = summarize_records(rec(seq=i) for i in range(3))
    assert s["requests"] == 3


# ---- guard-marker dedup on reset records -------------------------------

D = GUARD_MARKERS["doom_loop"]
C = GUARD_MARKERS["context_full"]


def msg(text, role="user"):
    return {"role": role, "content": text}


def test_summarize_records_guard_events_when_reset_false_should_count_repeated_messages():
    m = msg(f"{D} again")
    s = summarize_records([rec(messages_delta=[m]), rec(messages_delta=[m])])
    assert s["guard_events"]["doom_loop"] == 2


def test_summarize_records_guard_events_when_reset_replays_counted_message_should_not_recount():
    old = msg(f"{D} first")
    new = msg(f"{D} second")
    s = summarize_records(
        [
            rec(messages_delta=[old]),
            rec(messages_delta=[old, new], messages_reset=True),
        ]
    )
    assert s["guard_events"]["doom_loop"] == 2  # old once, new once


def test_summarize_records_guard_events_when_reset_has_unseen_message_should_count_it():
    s = summarize_records([rec(messages_delta=[msg(f"{C} x")], messages_reset=True)])
    assert s["guard_events"]["context_full"] == 1


def test_summarize_records_guard_events_when_replayed_across_several_resets_should_count_once():
    m = msg(f"{D} loop")
    s = summarize_records(
        [
            rec(messages_delta=[m], messages_reset=True),
            rec(messages_delta=[m], messages_reset=True),
            rec(messages_delta=[m], messages_reset=True),
        ]
    )
    assert s["guard_events"]["doom_loop"] == 1


def test_summarize_records_guard_events_dedup_should_be_per_marker_and_text_pair():
    both = msg(f"{D} and {C}")
    s = summarize_records(
        [
            rec(messages_delta=[msg(f"{D} only")]),
            rec(messages_delta=[both], messages_reset=True),  # D with new text, C unseen
            rec(messages_delta=[both], messages_reset=True),  # both pairs already counted
        ]
    )
    assert s["guard_events"] == {"doom_loop": 2, "context_full": 1, "output_truncated": 0}


def test_summarize_records_guard_events_non_reset_record_after_reset_should_count_again():
    m = msg(f"{D} loop")
    s = summarize_records(
        [
            rec(messages_delta=[m], messages_reset=True),
            rec(messages_delta=[m], messages_reset=False),
        ]
    )
    assert s["guard_events"]["doom_loop"] == 2
