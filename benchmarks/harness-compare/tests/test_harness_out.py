"""Spec section 6: hcbench.harness_out."""

import json

import pytest

from hcbench.harness_out import parse_hone_json, parse_pi_jsonl

BASE_KEYS = {"success", "error", "tool_calls", "tool_errors", "input_tokens", "output_tokens"}


def hone_payload(**over):
    payload = {
        "response": "done",
        "tool_calls": [
            {"name": "bash", "result": "ok", "is_error": False},
            {"name": "read", "result": "boom", "is_error": True},
            {"name": "edit", "result": "boom", "is_error": True},
        ],
        "usage": {"input": 1200, "output": 300, "cache_read": 50},
        "cost_usd": 0.0,
        "model": "m",
        "success": True,
        "error": None,
    }
    payload.update(over)
    return payload


# ---- hone --------------------------------------------------------------


def test_parse_hone_json_when_clean_pretty_json_should_extract_fields():
    out = parse_hone_json(json.dumps(hone_payload(), indent=2))
    assert set(out) >= BASE_KEYS
    assert out["success"] is True
    assert out["error"] is None
    assert out["tool_calls"] == 3
    assert out["tool_errors"] == 2
    assert out["input_tokens"] == 1200
    assert out["output_tokens"] == 300
    assert out["compactions"] == 0
    assert out["retries"] == 0


def test_parse_hone_json_when_leading_log_lines_should_parse_from_first_brace_line():
    text = "INFO starting up\nWARN [x] something {odd}\n" + json.dumps(hone_payload(), indent=2)
    out = parse_hone_json(text)
    assert out["tool_calls"] == 3
    assert out["success"] is True


def test_parse_hone_json_when_failure_should_surface_error():
    out = parse_hone_json(json.dumps(hone_payload(success=False, error="max turns"), indent=2))
    assert out["success"] is False
    assert out["error"] == "max turns"


def test_parse_hone_json_when_usage_null_should_report_zero_tokens():
    out = parse_hone_json(json.dumps(hone_payload(usage=None), indent=2))
    assert out["input_tokens"] == 0
    assert out["output_tokens"] == 0


@pytest.mark.parametrize("text", ["", "not json at all", "log line\nanother log line", "{broken"])
def test_parse_hone_json_when_unparseable_should_report_failure_with_zero_counts(text):
    out = parse_hone_json(text)
    assert out["success"] is False
    assert out["error"] == "unparseable"
    assert out["tool_calls"] == 0
    assert out["tool_errors"] == 0
    assert out["input_tokens"] == 0
    assert out["output_tokens"] == 0


# ---- pi ----------------------------------------------------------------


def ev(**kw):
    return json.dumps(kw)


def msg_end(role="assistant", inp=None, out=None, **extra):
    message = {"role": role, **extra}
    if inp is not None or out is not None:
        message["usage"] = {"input": inp or 0, "output": out or 0}
    return ev(type="message_end", message=message)


def test_parse_pi_jsonl_when_settled_cleanly_should_succeed_and_sum_assistant_usage():
    lines = [
        ev(type="agent_start"),
        msg_end("user", 999, 999),
        ev(type="tool_execution_start", toolName="bash"),
        ev(type="tool_execution_end", isError=False),
        ev(type="tool_execution_start", toolName="read"),
        ev(type="tool_execution_end", isError=True),
        msg_end("assistant", 100, 10, stopReason="toolUse"),
        msg_end("assistant", 200, 20, stopReason="stop"),
        ev(type="agent_end", willRetry=False),
        ev(type="agent_settled", aborted=False),
    ]
    out = parse_pi_jsonl(lines)
    assert set(out) >= BASE_KEYS | {"compactions", "retries"}
    assert out["success"] is True
    assert out["error"] is None
    assert out["tool_calls"] == 2
    assert out["tool_errors"] == 1
    assert out["input_tokens"] == 300
    assert out["output_tokens"] == 30
    assert out["compactions"] == 0
    assert out["retries"] == 0


def test_parse_pi_jsonl_when_aborted_should_not_succeed():
    out = parse_pi_jsonl([msg_end("assistant", 1, 1), ev(type="agent_settled", aborted=True)])
    assert out["success"] is False


def test_parse_pi_jsonl_when_no_settled_and_last_agent_end_final_should_succeed():
    lines = [ev(type="agent_end", willRetry=True), ev(type="agent_end", willRetry=False)]
    out = parse_pi_jsonl(lines)
    assert out["success"] is True


def test_parse_pi_jsonl_when_no_settled_and_last_agent_end_will_retry_should_not_succeed():
    lines = [ev(type="agent_end", willRetry=False), ev(type="agent_end", willRetry=True)]
    out = parse_pi_jsonl(lines)
    assert out["success"] is False


def test_parse_pi_jsonl_when_no_terminal_events_should_not_succeed():
    assert parse_pi_jsonl([ev(type="agent_start")])["success"] is False
    assert parse_pi_jsonl([])["success"] is False


def test_parse_pi_jsonl_when_assistant_stop_reason_error_should_fail_even_if_settled():
    lines = [
        msg_end("assistant", 5, 1, stopReason="error", errorMessage="429 rate limited"),
        ev(type="agent_settled", aborted=False),
    ]
    out = parse_pi_jsonl(lines)
    assert out["success"] is False
    assert out["error"] == "429 rate limited"


def test_parse_pi_jsonl_error_should_be_last_assistant_error_message():
    lines = [
        msg_end("assistant", stopReason="error", errorMessage="first"),
        msg_end("assistant", stopReason="error", errorMessage="second"),
        msg_end("assistant", stopReason="stop"),
        ev(type="agent_settled", aborted=False),
    ]
    assert parse_pi_jsonl(lines)["error"] == "second"


def test_parse_pi_jsonl_should_ignore_error_message_on_non_assistant_roles():
    lines = [msg_end("user", errorMessage="not mine"), ev(type="agent_settled", aborted=False)]
    out = parse_pi_jsonl(lines)
    assert out["error"] is None
    assert out["success"] is True


def test_parse_pi_jsonl_should_count_compactions_and_retries():
    lines = [
        ev(type="compaction_start"),
        ev(type="compaction_end"),
        ev(type="compaction_start"),
        ev(type="auto_retry_start"),
        ev(type="auto_retry_end"),
        ev(type="agent_settled", aborted=False),
    ]
    out = parse_pi_jsonl(lines)
    assert out["compactions"] == 2
    assert out["retries"] == 1


def test_parse_pi_jsonl_when_usage_missing_should_count_zero():
    out = parse_pi_jsonl([msg_end("assistant"), msg_end("assistant", 7, 3)])
    assert out["input_tokens"] == 7
    assert out["output_tokens"] == 3


def test_parse_pi_jsonl_should_skip_garbage_and_blank_lines():
    lines = [
        "pi v1.2.3 starting",
        "",
        ev(type="tool_execution_start"),
        "{not json",
        "   ",
        ev(type="tool_execution_end", isError=True),
        "WARN something",
        ev(type="agent_settled", aborted=False),
    ]
    out = parse_pi_jsonl(lines)
    assert out["tool_calls"] == 1
    assert out["tool_errors"] == 1
    assert out["success"] is True


def test_parse_pi_jsonl_should_accept_a_generator_and_newline_terminated_lines():
    events = [ev(type="tool_execution_start"), ev(type="agent_settled", aborted=False)]
    gen = (line + "\n" for line in events)
    out = parse_pi_jsonl(gen)
    assert out["tool_calls"] == 1
    assert out["success"] is True
