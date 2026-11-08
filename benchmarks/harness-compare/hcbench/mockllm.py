"""Minimal OpenAI-compatible server for the no-GPU selftest (scripts/selftest.sh)."""

from __future__ import annotations

import argparse
import json

from aiohttp import web

USAGE = {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15}


async def chat(request: web.Request) -> web.StreamResponse:
    body = await request.json()
    model = body.get("model", "mock")
    if not body.get("stream"):
        return web.json_response(
            {
                "id": "mock-1",
                "object": "chat.completion",
                "model": model,
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "selftest ok"},
                        "finish_reason": "stop",
                    }
                ],
                "usage": USAGE,
            }
        )
    resp = web.StreamResponse(headers={"Content-Type": "text/event-stream"})
    await resp.prepare(request)
    chunks = [
        {"choices": [{"index": 0, "delta": {"role": "assistant", "content": "selftest ok"}}]},
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
    ]
    if (body.get("stream_options") or {}).get("include_usage"):
        chunks.append({"choices": [], "usage": USAGE})
    for c in chunks:
        await resp.write(f"data: {json.dumps(c)}\n\n".encode())
    await resp.write(b"data: [DONE]\n\n")
    await resp.write_eof()
    return resp


async def models(_: web.Request) -> web.Response:
    return web.json_response({"object": "list", "data": [{"id": "mock"}]})


def create_app() -> web.Application:
    app = web.Application()
    app.router.add_post("/v1/chat/completions", chat)
    app.router.add_get("/v1/models", models)
    return app


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--host", default="0.0.0.0")
    ap.add_argument("--port", type=int, default=8000)
    args = ap.parse_args()
    web.run_app(create_app(), host=args.host, port=args.port)


if __name__ == "__main__":
    main()
