"""Spec section 6 (Hermes): hcbench.harness_out.parse_hermes_jsonl.

Real `hermes chat -Q --format stream-json` shape (hermes_cli/stream_json.py at v2026.9.24):

  {"type": "system", ...}
  {"type": "text", "text": "..."}
  {"type": "tool_use", "name": "terminal", "input": {...}}        # no tool_call_id at this SHA
  {"type": "tool_result", "content": "...", "is_error": false}     # is_error: stream_json.py
  {"type": "result", "exit_code": 0, "text": "final", "tokens": {"input": N, "output": M}}
                                                       # optional "error": "..." on failure

`tool_result.is_error` is real (stream_json.py, tool_executor.py:1121): tool_errors counts it.
A tolerance test keeps the earlier assumed shape (`is_error` / `usage.input_tokens`) working.
"""

import json

import pytest

from hcbench.harness_out import parse_hermes_jsonl

BASE_KEYS = {
    "success", "error", "tool_calls", "tool_errors", "input_tokens", "output_tokens",
    "compactions", "retries",
}  # fmt: skip


def ev(**kw):
    return json.dumps(kw)


def tool_use(i=1, name="terminal"):
    return ev(type="tool_use", name=name, input={"command": "ls"})


def tool_result(i=1, is_error=False):
    return ev(type="tool_result", content="out", is_error=is_error)


def result(exit_code=0, inp=None, out=None, **extra):
    d = {"type": "result", "exit_code": exit_code, "text": "done", **extra}
    if inp is not None or out is not None:
        d["tokens"] = {"input": inp or 0, "output": out or 0}
    return json.dumps(d)


def good_run():
    return [
        ev(type="system", subtype="init", model="m"),
        ev(type="text", text="looking"),
        tool_use(1),
        tool_result(1),
        tool_use(2, "read_file"),
        tool_result(2, is_error=True),
        ev(type="text", text="all done"),
        result(0, 1200, 300),
    ]


def test_parse_hermes_jsonl_when_result_event_clean_should_succeed_and_count_tools():
    out = parse_hermes_jsonl(good_run())
    assert set(out) >= BASE_KEYS
    assert out["success"] is True
    assert out["error"] is None
    assert out["tool_calls"] == 2
    assert out["tool_errors"] == 1
    assert out["input_tokens"] == 1200
    assert out["output_tokens"] == 300
    assert out["compactions"] is None  # Hermes compacts but does not report it: unknown, not 0
    assert out["retries"] == 0
    assert out["completed"] is True


def test_parse_hermes_jsonl_when_no_result_event_should_not_succeed():
    out = parse_hermes_jsonl(good_run()[:-1])
    assert out["success"] is False
    assert out["tool_calls"] == 2


def test_parse_hermes_jsonl_when_empty_should_fail_with_zero_counts():
    out = parse_hermes_jsonl([])
    assert out["success"] is False
    assert out["tool_calls"] == out["tool_errors"] == 0
    assert out["input_tokens"] == out["output_tokens"] == 0


def test_parse_hermes_jsonl_when_result_has_error_should_fail_and_surface_error():
    lines = [tool_use(1), result(1, error="max turns exceeded")]
    out = parse_hermes_jsonl(lines)
    assert out["success"] is False
    assert out["error"] == "max turns exceeded"


def test_parse_hermes_jsonl_when_result_exit_code_nonzero_without_error_should_fail():
    out = parse_hermes_jsonl([result(1)])
    assert out["success"] is False
    assert out["error"]


def test_parse_hermes_jsonl_when_result_has_error_but_exit_code_zero_should_still_fail():
    out = parse_hermes_jsonl([result(0, error="partial")])
    assert out["success"] is False
    assert out["error"] == "partial"


def test_parse_hermes_jsonl_should_accept_earlier_assumed_shape_as_tolerance():
    line = json.dumps(
        {"type": "result", "is_error": False, "usage": {"input_tokens": 9, "output_tokens": 4}}
    )
    out = parse_hermes_jsonl([line])
    assert out["success"] is True
    assert (out["input_tokens"], out["output_tokens"]) == (9, 4)


def test_parse_hermes_jsonl_should_use_the_last_result_event():
    out = parse_hermes_jsonl([result(1, error="first"), result(0, 5, 2)])
    assert out["success"] is True
    assert out["error"] is None
    assert out["input_tokens"] == 5


def test_parse_hermes_jsonl_when_usage_missing_should_report_zero_tokens():
    out = parse_hermes_jsonl([result(0)])
    assert out["success"] is True
    assert out["input_tokens"] == 0
    assert out["output_tokens"] == 0


def test_parse_hermes_jsonl_should_count_tool_use_not_tool_result():
    out = parse_hermes_jsonl([tool_result(1), tool_result(2, True), result(0)])
    assert out["tool_calls"] == 0
    assert out["tool_errors"] == 1  # errors are counted from tool_result.is_error


def test_parse_hermes_jsonl_should_skip_garbage_blank_and_non_object_lines():
    lines = [
        "hermes v9.9 starting",
        "",
        "   ",
        tool_use(1),
        "{not json",
        "[1, 2, 3]",
        '"just a string"',
        "null",
        ev(no_type_key=True),
        tool_result(1, True),
        "WARN something",
        result(0, 3, 4),
    ]
    out = parse_hermes_jsonl(lines)
    assert out["tool_calls"] == 1
    assert out["tool_errors"] == 1
    assert out["success"] is True
    assert out["output_tokens"] == 4


def test_parse_hermes_jsonl_when_output_truncated_mid_line_should_not_succeed():
    lines = [tool_use(1), tool_result(1), result(0, 1, 1)[:-6]]
    out = parse_hermes_jsonl(lines)
    assert out["success"] is False
    assert out["tool_calls"] == 1


def test_parse_hermes_jsonl_should_accept_generator_and_newline_terminated_lines():
    gen = (line + "\n" for line in good_run())
    out = parse_hermes_jsonl(gen)
    assert out["tool_calls"] == 2
    assert out["success"] is True


# ---- exit codes: 0 completed; 1 failed/partial/budget; 130 interrupted ----


def test_parse_hermes_jsonl_when_exit_code_zero_should_not_change_result():
    assert parse_hermes_jsonl(good_run(), exit_code=0)["success"] is True


def test_parse_hermes_jsonl_when_exit_code_one_should_fail_even_with_clean_result():
    out = parse_hermes_jsonl(good_run(), exit_code=1)
    assert out["success"] is False
    assert out["error"]  # some non-empty reason (e.g. "exit code 1")
    assert out["tool_calls"] == 2  # counts are still reported


def test_parse_hermes_jsonl_when_exit_code_130_should_report_interrupted():
    out = parse_hermes_jsonl(good_run()[:-1], exit_code=130)
    assert out["success"] is False
    assert "interrupt" in str(out["error"]).lower()


def test_parse_hermes_jsonl_when_exit_code_zero_but_no_result_should_not_succeed():
    assert parse_hermes_jsonl(good_run()[:-1], exit_code=0)["success"] is False


def test_parse_hermes_jsonl_when_exit_code_omitted_should_rely_on_events_only():
    assert parse_hermes_jsonl(good_run())["success"] is True


@pytest.mark.parametrize("code", [2, 137, 255])
def test_parse_hermes_jsonl_when_other_nonzero_exit_should_fail(code):
    assert parse_hermes_jsonl(good_run(), exit_code=code)["success"] is False


# ---- runner dispatch -----------------------------------------------------


def test_harness_summary_when_kind_hermes_should_parse_stdout_as_stream_json():
    from hcbench.runner import harness_summary

    out = harness_summary("hermes", "\n".join(good_run()) + "\n")
    assert out["success"] is True
    assert out["tool_calls"] == 2
    assert out["input_tokens"] == 1200


# ---- round 2: unknown compactions; `completed` = a result event was seen ----


def test_parse_hermes_jsonl_compactions_should_be_none_even_without_events():
    assert parse_hermes_jsonl([])["compactions"] is None
    assert parse_hermes_jsonl([result(1, error="x")])["compactions"] is None


def test_parse_hermes_jsonl_completed_should_report_whether_a_result_event_was_seen():
    assert parse_hermes_jsonl(good_run())["completed"] is True
    assert parse_hermes_jsonl(good_run()[:-1])["completed"] is False
    # seen, even if failed
    assert parse_hermes_jsonl([result(1, error="boom")])["completed"] is True


def test_harness_summary_should_pass_process_exit_code_to_the_hermes_parser():
    from hcbench.runner import harness_summary

    out = harness_summary("hermes", "\n".join(good_run()), 130)
    assert out["success"] is False
    assert "interrupt" in str(out["error"]).lower()
