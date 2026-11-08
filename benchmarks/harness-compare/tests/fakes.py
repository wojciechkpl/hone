"""Test helpers: a fake OpenAI-compatible upstream and a running proxy stack.

Not a test module (no ``test_`` prefix); imported by test_proxy.py.
"""

from __future__ import annotations

import asyncio
import json
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from aiohttp import ClientSession, web
from aiohttp.test_utils import TestServer

from hcbench.proxy import create_app

DEFAULT_JSON = {
    "id": "chatcmpl-1",
    "object": "chat.completion",
    "choices": [
        {"index": 0, "message": {"role": "assistant", "content": "hello"}, "finish_reason": "stop"}
    ],
    "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
}


def sse_data(payload: Any) -> str:
    text = payload if isinstance(payload, str) else json.dumps(payload)
    return f"data: {text}\n\n"


@dataclass
class FakeUpstream:
    """Returns ``json_body`` for non-stream requests and ``sse_chunks`` + [DONE] for streams."""

    json_body: dict = field(default_factory=lambda: dict(DEFAULT_JSON))
    sse_chunks: list = field(default_factory=list)
    status: int = 200
    calls: list = field(default_factory=list)  # parsed JSON bodies of chat requests
    models_calls: int = 0
    # Streaming gate: the first ``gate_after`` chunks are sent, then the handler waits for
    # ``gate`` before sending the rest. Used to hold a stream open for disconnect tests.
    gate: asyncio.Event | None = None
    gate_after: int = 0

    async def _chat(self, request: web.Request) -> web.StreamResponse:
        body = await request.json()
        self.calls.append(body)
        if not body.get("stream"):
            return web.json_response(self.json_body, status=self.status)
        resp = web.StreamResponse(headers={"Content-Type": "text/event-stream"})
        await resp.prepare(request)
        try:
            for i, chunk in enumerate(self.sse_chunks):
                if self.gate is not None and i == self.gate_after:
                    await asyncio.wait_for(self.gate.wait(), timeout=10)
                await resp.write(sse_data(chunk).encode())
            await resp.write(sse_data("[DONE]").encode())
            await resp.write_eof()
        except (ConnectionError, TimeoutError):
            pass
        return resp

    async def _models(self, request: web.Request) -> web.Response:
        self.models_calls += 1
        return web.json_response({"object": "list", "data": [{"id": "fake-model"}]})

    def app(self) -> web.Application:
        app = web.Application()
        app.router.add_post("/v1/chat/completions", self._chat)
        app.router.add_get("/v1/models", self._models)
        return app


@asynccontextmanager
async def serve(app: web.Application):
    server = TestServer(app)
    await server.start_server()
    try:
        yield f"http://{server.host}:{server.port}"
    finally:
        await server.close()


@dataclass
class Stack:
    base: str
    session: ClientSession
    upstream: FakeUpstream
    log_dir: Path
    headers: dict = field(default_factory=dict)  # sent on admin calls

    async def register(self, run_id: str, **kw):
        async with self.session.post(
            f"{self.base}/admin/runs", json={"run_id": run_id, **kw}, headers=self.headers
        ) as r:
            return r.status, await r.json()

    async def chat(self, run_id: str, body: dict):
        """POST a chat completion; returns (status, raw_text)."""
        async with self.session.post(
            f"{self.base}/run/{run_id}/v1/chat/completions", json=body
        ) as r:
            return r.status, await r.text()

    async def records(self, run_id: str):
        async with self.session.get(
            f"{self.base}/admin/runs/{run_id}/records", headers=self.headers
        ) as r:
            return r.status, await r.json()


@asynccontextmanager
async def proxy_stack(
    tmp_path: Path,
    upstream: FakeUpstream,
    overrides: dict | None = None,
    admin_token: str | None = None,
):
    """Run proxy + upstream. When ``admin_token`` is set, ``Stack.headers`` carries it."""
    log_dir = tmp_path / "logs"
    headers = {"Authorization": f"Bearer {admin_token}"} if admin_token else {}
    async with serve(upstream.app()) as up_url:
        # only pass admin_token when set, so unrelated tests do not depend on the new kwarg
        extra = {"admin_token": admin_token} if admin_token else {}
        app = create_app(up_url, log_dir, overrides, **extra)
        async with serve(app) as proxy_url, ClientSession() as session:
            yield Stack(proxy_url, session, upstream, log_dir, headers)
