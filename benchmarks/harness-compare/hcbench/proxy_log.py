"""Per-request records written by the metering proxy, and their summary."""

from __future__ import annotations

from collections.abc import Iterable, Iterator, Mapping

from .tools import classify_tool

GUARD_MARKERS = {
    "doom_loop": "[SYSTEM: DOOM LOOP DETECTED]",
    "context_full": "[SYSTEM: CONTEXT NEARLY FULL]",
    "output_truncated": "[SYSTEM: OUTPUT TRUNCATED]",
}


def message_text(message: Mapping) -> str:
    content = message.get("content")
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(str(p["text"]) for p in content if isinstance(p, Mapping) and "text" in p)
    return str(content)


def iter_message_texts(record: Mapping) -> Iterator[str]:
    for message in record.get("messages_delta") or []:
        if isinstance(message, Mapping):
            yield message_text(message)


def summarize_records(records: Iterable[Mapping]) -> dict:
    counts = {"read": 0, "edit": 0, "exec": 0, "other": 0}
    guard = dict.fromkeys(GUARD_MARKERS, 0)
    requests = prompt = completion = length = errors = 0
    budget = False
    llm_seconds = 0.0
    seen: set[tuple[str, str]] = set()
    for rec in records:
        requests += 1
        prompt += rec.get("prompt_tokens") or 0
        completion += rec.get("completion_tokens") or 0
        if rec.get("finish_reason") == "length":
            length += 1
        if (rec.get("http_status") or 0) >= 400:
            errors += 1
        budget = budget or bool(rec.get("budget_exceeded"))
        start, end = rec.get("ts_start"), rec.get("ts_end")
        if start is not None and end is not None:
            llm_seconds += max(0.0, float(end) - float(start))
        for call in rec.get("tool_calls") or []:
            counts[classify_tool(call.get("name", ""))] += 1
        replay = bool(rec.get("messages_reset"))  # compacted history re-sent in full
        for text in iter_message_texts(rec):
            for key, marker in GUARD_MARKERS.items():
                if marker not in text:
                    continue
                pair = (key, text)
                if replay and pair in seen:
                    continue
                guard[key] += 1
                seen.add(pair)
    tool_calls = sum(counts.values())
    return {
        "requests": requests,
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": prompt + completion,
        "tool_calls": tool_calls,
        "read_calls": counts["read"],
        "edit_calls": counts["edit"],
        "exec_calls": counts["exec"],
        "other_calls": counts["other"],
        "read_edit_ratio": (counts["read"] / counts["edit"]) if counts["edit"] else None,
        "length_finishes": length,
        "http_errors": errors,
        "budget_exceeded": budget,
        "llm_seconds": llm_seconds,
        "completion_tokens_per_s": (completion / llm_seconds) if llm_seconds else None,
        "guard_events": guard,
    }
