"""Parsers for each harness's final machine-readable output."""

from __future__ import annotations

import json
from collections.abc import Iterable


def _empty(**extra) -> dict:
    out = {
        "success": False,
        "error": None,
        "tool_calls": 0,
        "tool_errors": 0,
        "input_tokens": 0,
        "output_tokens": 0,
        "compactions": 0,
        "retries": 0,
    }
    out.update(extra)
    return out


def _load_hone(stdout: str) -> dict | None:
    try:
        obj = json.loads(stdout)
        return obj if isinstance(obj, dict) else None
    except (json.JSONDecodeError, ValueError):
        pass
    lines = stdout.splitlines()
    for i, line in enumerate(lines):
        if line.startswith("{"):
            try:
                obj = json.loads("\n".join(lines[i:]))
            except (json.JSONDecodeError, ValueError):
                return None
            return obj if isinstance(obj, dict) else None
    return None


def parse_hone_json(stdout: str) -> dict:
    obj = _load_hone(stdout or "")
    if obj is None:
        return _empty(error="unparseable")
    calls = obj.get("tool_calls") or []
    usage = obj.get("usage") or {}
    return _empty(
        success=bool(obj.get("success")),
        error=obj.get("error"),
        tool_calls=len(calls),
        tool_errors=sum(1 for c in calls if isinstance(c, dict) and c.get("is_error")),
        input_tokens=int(usage.get("input") or 0),
        output_tokens=int(usage.get("output") or 0),
    )


def parse_pi_jsonl(lines: Iterable[str]) -> dict:
    out = _empty()
    settled: bool | None = None
    last_agent_end_final: bool | None = None
    errored = False
    for raw in lines:
        line = raw.strip()
        if not line:
            continue
        try:
            ev = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        if not isinstance(ev, dict):
            continue
        kind = ev.get("type")
        if kind == "tool_execution_start":
            out["tool_calls"] += 1
        elif kind == "tool_execution_end":
            if ev.get("isError"):
                out["tool_errors"] += 1
        elif kind == "compaction_start":
            out["compactions"] += 1
        elif kind == "auto_retry_start":
            out["retries"] += 1
        elif kind == "agent_settled":
            settled = not ev.get("aborted", False)
        elif kind == "agent_end":
            last_agent_end_final = not ev.get("willRetry", False)
        elif kind == "message_end":
            msg = ev.get("message") or {}
            if msg.get("role") != "assistant":
                continue
            usage = msg.get("usage") or {}
            out["input_tokens"] += int(usage.get("input") or 0)
            out["output_tokens"] += int(usage.get("output") or 0)
            # Pi auto-retries transient provider errors, so only the final
            # assistant message decides whether the run ended in error.
            errored = msg.get("stopReason") == "error"
            if msg.get("errorMessage"):
                out["error"] = msg["errorMessage"]
    terminal_ok = settled if settled is not None else bool(last_agent_end_final)
    out["success"] = bool(terminal_ok) and not errored
    return out


_EXIT_INTERRUPTED = 130


def _int_or_zero(v) -> int:
    return v if isinstance(v, int) and not isinstance(v, bool) else 0


def _hermes_usage(ev: dict) -> tuple[int, int]:
    # SPEC shape: usage.{input_tokens,output_tokens}; Hermes v2026.9.x emits tokens.{input,output}.
    usage = ev.get("usage")
    if isinstance(usage, dict):
        return _int_or_zero(usage.get("input_tokens")), _int_or_zero(usage.get("output_tokens"))
    tokens = ev.get("tokens")
    if isinstance(tokens, dict):
        return _int_or_zero(tokens.get("input")), _int_or_zero(tokens.get("output"))
    return 0, 0


def parse_hermes_jsonl(lines: Iterable[str], exit_code: int | None = None) -> dict:
    """Parse `hermes chat -Q --format stream-json` events (SPEC section 6)."""
    out = _empty(compactions=None, completed=False)  # compactions: Hermes does not report
    last_result: dict | None = None
    for raw in lines:
        line = raw.strip()
        if not line:
            continue
        try:
            ev = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        if not isinstance(ev, dict):
            continue
        kind = ev.get("type")
        if kind == "tool_use":
            out["tool_calls"] += 1
        elif kind == "tool_result":
            if ev.get("is_error") is True:
                out["tool_errors"] += 1
        elif kind == "result":
            last_result = ev
    if last_result is not None:
        out["completed"] = True
        out["input_tokens"], out["output_tokens"] = _hermes_usage(last_result)
        err = last_result.get("error")
        out["error"] = str(err) if err else None
        # Real Hermes reports failure as a non-zero `exit_code` on the result event.
        failed = (
            last_result.get("is_error") is True
            or _int_or_zero(last_result.get("exit_code")) != 0
            or out["error"] is not None
        )
        out["success"] = not failed
        if failed and out["error"] is None:
            out["error"] = "hermes result reported failure"
    if exit_code not in (None, 0):
        out["success"] = False
        if exit_code == _EXIT_INTERRUPTED:
            out["error"] = "interrupted (exit code 130)"
        elif not out["error"]:
            out["error"] = f"exit code {exit_code}"
    return out
