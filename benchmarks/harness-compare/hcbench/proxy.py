"""Metering reverse proxy for OpenAI-compatible chat completions.

Every harness arm talks to the model through ``/run/<run_id>/v1``. The proxy:

* enforces identical sampling parameters for all arms (``overrides`` + per-run seed),
* enforces per-run caps (requests, total tokens) that neither harness exposes,
* records each request/response as a harness-neutral trajectory record
  (schema: SPEC.md §5), which is what tool-call counts, guard events and
  red-team classification are computed from.
"""

from __future__ import annotations

import argparse
import asyncio
import hmac
import json
import os
import time
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import aiohttp
from aiohttp import web

CHAT_PATH = "/v1/chat/completions"
DROPPED_SAMPLING_KEYS = frozenset(
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
SAMPLING_FIELDS = ("model", "temperature", "top_p", "max_tokens", "seed", "n")


class _ClientGone(Exception):
    """The harness closed its connection (e.g. killed by the wall-clock timeout)."""


@dataclass
class RunState:
    run_id: str
    max_requests: int | None = None
    max_total_tokens: int | None = None
    seed: int | None = None
    records: list[dict] = field(default_factory=list)
    seq: int = 0
    forwarded: int = 0
    tokens: int = 0
    prev_messages: list | None = None
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)

    def exhausted(self) -> bool:
        if self.max_requests is not None and self.forwarded >= self.max_requests:
            return True
        return self.max_total_tokens is not None and self.tokens >= self.max_total_tokens


RUNS_KEY = web.AppKey("runs", dict)
CFG_KEY = web.AppKey("cfg", dict)
SESSION_KEY = web.AppKey("session", aiohttp.ClientSession)


def _delta(prev: list | None, messages: list) -> tuple[list, bool]:
    if prev is None:
        return list(messages), False
    n = len(prev)
    if len(messages) >= n and messages[:n] == prev:
        return list(messages[n:]), False
    return list(messages), True


def _error(status: int, message: str, kind: str) -> web.Response:
    return web.json_response({"error": {"message": message, "type": kind}}, status=status)


def _authorized(request: web.Request) -> bool:
    token = request.app[CFG_KEY].get("admin_token")
    if not token:
        return True
    got = request.headers.get("Authorization", "")
    return hmac.compare_digest(got.encode(), f"Bearer {token}".encode())


def _unauthorized() -> web.Response:
    return _error(401, "admin token required", "unauthorized")


class _StreamAccumulator:
    def __init__(self) -> None:
        self.text: list[str] = []
        self.calls: dict[int, dict] = {}
        self.finish_reason: str | None = None
        self.prompt_tokens: int | None = None
        self.completion_tokens: int | None = None

    def feed(self, obj: Mapping) -> None:
        usage = obj.get("usage")
        if isinstance(usage, Mapping):
            self.prompt_tokens = usage.get("prompt_tokens", self.prompt_tokens)
            self.completion_tokens = usage.get("completion_tokens", self.completion_tokens)
        for choice in obj.get("choices") or []:
            if choice.get("index", 0) != 0:
                continue
            delta = choice.get("delta") or {}
            if delta.get("content"):
                self.text.append(delta["content"])
            for tc in delta.get("tool_calls") or []:
                slot = self.calls.setdefault(
                    tc.get("index", 0), {"id": "", "name": "", "arguments": ""}
                )
                if tc.get("id") and not slot["id"]:
                    slot["id"] = tc["id"]
                fn = tc.get("function") or {}
                if fn.get("name") and not slot["name"]:
                    slot["name"] = fn["name"]
                if fn.get("arguments"):
                    slot["arguments"] += fn["arguments"]
            if choice.get("finish_reason"):
                self.finish_reason = choice["finish_reason"]

    def tool_calls(self) -> list[dict]:
        return [self.calls[i] for i in sorted(self.calls)]


def _parse_completion(payload: Any) -> dict:
    out = {
        "prompt_tokens": None,
        "completion_tokens": None,
        "finish_reason": None,
        "tool_calls": [],
        "assistant_text": "",
    }
    if not isinstance(payload, Mapping):
        return out
    usage = payload.get("usage") or {}
    out["prompt_tokens"] = usage.get("prompt_tokens")
    out["completion_tokens"] = usage.get("completion_tokens")
    choices = payload.get("choices") or []
    if choices:
        choice = choices[0]
        msg = choice.get("message") or {}
        out["finish_reason"] = choice.get("finish_reason")
        out["assistant_text"] = msg.get("content") or ""
        out["tool_calls"] = [
            {
                "id": tc.get("id", ""),
                "name": (tc.get("function") or {}).get("name", ""),
                "arguments": (tc.get("function") or {}).get("arguments", ""),
            }
            for tc in msg.get("tool_calls") or []
        ]
    return out


def _persist(app: web.Application, st: RunState, record: dict) -> None:
    st.records.append(record)
    st.tokens += (record.get("prompt_tokens") or 0) + (record.get("completion_tokens") or 0)
    path = app[CFG_KEY]["log_dir"] / f"{st.run_id}.jsonl"
    with path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(record, ensure_ascii=False) + "\n")


async def _register(request: web.Request) -> web.Response:
    if not _authorized(request):
        return _unauthorized()
    payload = await request.json()
    run_id = payload.get("run_id")
    if not run_id or not isinstance(run_id, str):
        return _error(400, "run_id is required", "invalid_request")
    st = RunState(
        run_id=run_id,
        max_requests=payload.get("max_requests"),
        max_total_tokens=payload.get("max_total_tokens"),
        seed=payload.get("seed"),
    )
    request.app[RUNS_KEY][run_id] = st
    (request.app[CFG_KEY]["log_dir"] / f"{run_id}.jsonl").write_text("")
    return web.json_response({"run_id": run_id}, status=201)


async def _records(request: web.Request) -> web.Response:
    if not _authorized(request):
        return _unauthorized()
    st = request.app[RUNS_KEY].get(request.match_info["run_id"])
    if st is None:
        return _error(404, "unknown run", "unknown_run")
    return web.json_response(st.records)


async def _models(request: web.Request) -> web.Response:
    upstream = request.app[CFG_KEY]["upstream"]
    async with request.app[SESSION_KEY].get(f"{upstream}/v1/models") as resp:
        body = await resp.read()
        return web.Response(body=body, status=resp.status, content_type="application/json")


async def _chat(request: web.Request) -> web.StreamResponse:
    app = request.app
    run_id = request.match_info["run_id"]
    st: RunState | None = app[RUNS_KEY].get(run_id)
    if st is None:
        return _error(404, f"unknown run {run_id!r}; register it via /admin/runs", "unknown_run")
    body = await request.json()

    async with st.lock:
        messages = body.get("messages") or []
        delta, reset = _delta(st.prev_messages, messages)
        st.prev_messages = list(messages)
        stream = bool(body.get("stream"))
        record: dict[str, Any] = {
            "run_id": run_id,
            "seq": st.seq,
            "ts_start": time.time(),
            "ts_end": None,
            "http_status": None,
            "stream": stream,
            "prompt_tokens": None,
            "completion_tokens": None,
            "finish_reason": None,
            "tool_calls": [],
            "assistant_text": "",
            "messages_delta": delta,
            "messages_reset": reset,
            "budget_exceeded": False,
            "error": None,
            "sampling": None,
        }
        st.seq += 1

        if st.exhausted():
            record.update(ts_end=time.time(), http_status=400, budget_exceeded=True, sampling=None)
            _persist(app, st, record)
            return _error(400, "hcbench budget exhausted", "budget_exceeded")

        overrides = app[CFG_KEY]["overrides"]
        body.update(overrides)
        if st.seed is not None:
            body["seed"] = st.seed
        for key in DROPPED_SAMPLING_KEYS - set(overrides):
            body.pop(key, None)
        body["n"] = 1
        if "max_tokens" in overrides:
            body.pop("max_completion_tokens", None)
        record["sampling"] = {k: body[k] for k in SAMPLING_FIELDS if k in body}
        client_wants_usage = bool((body.get("stream_options") or {}).get("include_usage"))
        if stream:
            body["stream_options"] = {**(body.get("stream_options") or {}), "include_usage": True}

        st.forwarded += 1
        url = f"{app[CFG_KEY]['upstream']}{CHAT_PATH}"
        acc = _StreamAccumulator()
        persisted = False

        def finish(from_stream: bool, **fields: Any) -> None:
            """Persist the record exactly once; stream fields come from the accumulator."""
            nonlocal persisted
            if persisted:
                return
            if from_stream:
                record.update(
                    prompt_tokens=acc.prompt_tokens,
                    completion_tokens=acc.completion_tokens,
                    finish_reason=acc.finish_reason,
                    tool_calls=acc.tool_calls(),
                    assistant_text="".join(acc.text),
                )
            record.update(ts_end=time.time(), **fields)
            _persist(app, st, record)
            persisted = True

        try:
            async with app[SESSION_KEY].post(url, json=body) as resp:
                is_sse = resp.content_type == "text/event-stream"
                if not (stream and resp.status == 200 and is_sse):
                    raw = await resp.read()
                    try:
                        parsed = _parse_completion(json.loads(raw))
                    except (json.JSONDecodeError, ValueError):
                        parsed = _parse_completion(None)
                    record.update(parsed)
                    error = None
                    if resp.status >= 400:
                        error = raw[:2000].decode("utf-8", errors="replace")
                    finish(False, http_status=resp.status, error=error)
                    return web.Response(
                        body=raw,
                        status=resp.status,
                        content_type=resp.content_type or "application/json",
                    )

                out = web.StreamResponse(
                    status=200,
                    headers={"Content-Type": "text/event-stream", "Cache-Control": "no-cache"},
                )
                try:
                    await out.prepare(request)
                except ConnectionResetError as e:
                    raise _ClientGone from e
                buf = b""
                async for chunk in resp.content.iter_any():
                    buf += chunk
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        await _forward_line(out, line.rstrip(b"\r"), acc, client_wants_usage)
                if buf.strip():
                    await _forward_line(out, buf.rstrip(b"\r"), acc, client_wants_usage)
                try:
                    await out.write_eof()
                except ConnectionResetError as e:
                    raise _ClientGone from e
                finish(True, http_status=200)
                return out
        except _ClientGone:
            finish(stream, http_status=499, error="client disconnected")
            return web.Response(status=499)
        except asyncio.CancelledError:
            finish(stream, http_status=499, error="client disconnected")
            raise
        except aiohttp.ClientError as e:
            finish(stream, http_status=502, error=f"upstream error: {e}")
            return _error(502, f"upstream error: {e}", "upstream_error")
        finally:
            finish(stream, http_status=500, error="proxy error")


async def _forward_line(
    out: web.StreamResponse, line: bytes, acc: _StreamAccumulator, client_wants_usage: bool
) -> None:
    if not line.strip():
        return  # event separators are re-emitted below
    if line.startswith(b"data:"):
        payload = line[len(b"data:") :].strip()
        if payload != b"[DONE]":
            try:
                obj = json.loads(payload)
            except (json.JSONDecodeError, ValueError):
                obj = None
            if isinstance(obj, Mapping):
                acc.feed(obj)
                usage_only = obj.get("choices") == [] and obj.get("usage") is not None
                if usage_only and not client_wants_usage:
                    return
        data = line + b"\n\n"
    else:
        data = line + b"\n"
    try:
        await out.write(data)
    except ConnectionResetError as e:  # includes aiohttp's ClientConnectionResetError
        raise _ClientGone from e


def create_app(
    upstream: str,
    log_dir: Path,
    overrides: Mapping[str, Any] | None = None,
    admin_token: str | None = None,
) -> web.Application:
    log_dir = Path(log_dir)
    log_dir.mkdir(parents=True, exist_ok=True)
    app = web.Application(client_max_size=64 * 1024 * 1024)
    app[RUNS_KEY] = {}
    app[CFG_KEY] = {
        "upstream": upstream.rstrip("/"),
        "log_dir": log_dir,
        "overrides": dict(overrides or {}),
        "admin_token": admin_token or None,
    }

    async def session_ctx(app: web.Application):
        timeout = aiohttp.ClientTimeout(total=None, sock_connect=30, sock_read=900)
        async with aiohttp.ClientSession(timeout=timeout) as session:
            app[SESSION_KEY] = session
            yield

    app.cleanup_ctx.append(session_ctx)
    app.router.add_post("/admin/runs", _register)
    app.router.add_get("/admin/runs/{run_id}/records", _records)
    app.router.add_post("/run/{run_id}/v1/chat/completions", _chat)
    app.router.add_get("/run/{run_id}/v1/models", _models)
    return app


def _parse_override(item: str) -> tuple[str, Any]:
    key, _, value = item.partition("=")
    if not key or not _:
        raise argparse.ArgumentTypeError(f"override must be KEY=VALUE, got {item!r}")
    try:
        return key, json.loads(value)
    except json.JSONDecodeError:
        return key, value


def main() -> None:
    ap = argparse.ArgumentParser(description="hcbench metering proxy")
    ap.add_argument("--upstream", required=True, help="e.g. http://hc-llm:8000")
    ap.add_argument("--host", default="0.0.0.0")
    ap.add_argument("--port", type=int, default=8100)
    ap.add_argument("--log-dir", type=Path, required=True)
    ap.add_argument(
        "--override",
        action="append",
        default=[],
        type=_parse_override,
        help="request field forced on every call, e.g. temperature=0 (repeatable)",
    )
    ap.add_argument(
        "--admin-token-env",
        default="HC_ADMIN_TOKEN",
        help="environment variable holding the admin bearer token (unset = admin routes open)",
    )
    args = ap.parse_args()
    token = os.environ.get(args.admin_token_env) or None
    app = create_app(args.upstream, args.log_dir, dict(args.override), admin_token=token)
    web.run_app(app, host=args.host, port=args.port)


if __name__ == "__main__":
    main()
