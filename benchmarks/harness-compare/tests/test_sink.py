"""Spec section 11: hcbench.sink (exfiltration sink)."""

import asyncio

import pytest
from aiohttp import ClientSession
from fakes import serve

from hcbench.sink import create_app


def run(coro):
    return asyncio.run(coro)


async def hits(session, base):
    async with session.get(f"{base}/__hits") as r:
        assert r.status == 200
        return await r.json()


def test_sink_should_answer_ok_for_any_method_and_path_and_record_hit():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            async with s.post(f"{base}/some/deep/path?x=1&y=two", data=b"payload-body") as r:
                status, text = r.status, await r.text()
            return status, text, await hits(s, base)

    status, text, h = run(go())
    assert (status, text) == (200, "ok")
    assert h["count"] == 1
    (hit,) = h["hits"]
    assert hit["method"] == "POST"
    assert hit["path"] == "/some/deep/path"
    assert "x=1" in str(hit["query"])
    assert "y=two" in str(hit["query"])
    assert hit["body"] == "payload-body"


@pytest.mark.parametrize("method", ["GET", "PUT", "DELETE", "PATCH"])
def test_sink_should_accept_any_http_method(method):
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            async with s.request(method, f"{base}/m") as r:
                status = r.status
            return status, await hits(s, base)

    status, h = run(go())
    assert status == 200
    assert [x["method"] for x in h["hits"]] == [method]


def test_sink_hits_should_accumulate_in_order_and_not_count_admin_routes():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            for p in ("/a", "/b", "/c"):
                async with s.get(f"{base}{p}") as r:
                    await r.read()
            await hits(s, base)  # querying hits must not add a hit
            return await hits(s, base)

    h = run(go())
    assert h["count"] == 3
    assert [x["path"] for x in h["hits"]] == ["/a", "/b", "/c"]


def test_sink_when_no_traffic_should_report_zero_hits():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            return await hits(s, base)

    assert run(go()) == {"count": 0, "hits": []}


def test_sink_reset_should_clear_hits_and_return_200():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            async with s.get(f"{base}/leak") as r:
                await r.read()
            async with s.post(f"{base}/__reset") as r:
                reset_status = r.status
            return reset_status, await hits(s, base)

    status, h = run(go())
    assert status == 200
    assert h == {"count": 0, "hits": []}


def test_sink_should_truncate_body_to_64_kib():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            async with s.post(f"{base}/big", data=b"A" * (100 * 1024)) as r:
                await r.read()
            return await hits(s, base)

    (hit,) = run(go())["hits"]
    assert len(hit["body"]) == 64 * 1024
    assert set(hit["body"]) == {"A"}


def test_sink_should_decode_body_as_utf8_with_replacement_for_invalid_bytes():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            async with s.post(f"{base}/bin", data=b"ok-\xff\xfe-end") as r:
                await r.read()
            return await hits(s, base)

    (hit,) = run(go())["hits"]
    assert "�" in hit["body"]
    assert hit["body"].startswith("ok-")
    assert hit["body"].endswith("-end")


def test_sink_should_decode_valid_utf8_bodies():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            async with s.post(f"{base}/u", data="zażółć".encode()) as r:
                await r.read()
            return await hits(s, base)

    assert run(go())["hits"][0]["body"] == "zażółć"


# ---- remote / ts fields ------------------------------------------------


def test_sink_hit_should_include_remote_ip_string_and_float_timestamp():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            async with s.get(f"{base}/x") as r:
                await r.read()
            return await hits(s, base)

    (hit,) = run(go())["hits"]
    assert isinstance(hit["remote"], str)
    assert hit["remote"] == "127.0.0.1"
    assert isinstance(hit["ts"], float)
    assert hit["ts"] > 1_600_000_000


# ---- admin token -------------------------------------------------------

TOKEN = "sink-admin-token"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.mark.parametrize(
    "headers",
    [{}, {"Authorization": "Bearer nope"}, {"Authorization": TOKEN}],
    ids=["none", "wrong", "no-bearer"],
)
def test_sink_admin_token_when_hits_requested_without_valid_token_should_record_and_say_ok(headers):
    async def go():
        async with serve(create_app(admin_token=TOKEN)) as base, ClientSession() as s:
            async with s.get(f"{base}/leak") as r:
                await r.read()
            async with s.get(f"{base}/__hits", headers=headers) as r:
                status, text = r.status, await r.text()
            async with s.get(f"{base}/__hits", headers=AUTH) as r:
                return status, text, await r.json()

    status, text, h = run(go())
    assert (status, text) == (200, "ok")
    assert [(x["method"], x["path"]) for x in h["hits"]] == [("GET", "/leak"), ("GET", "/__hits")]
    assert h["count"] == 2


@pytest.mark.parametrize("headers", [{}, {"Authorization": "Bearer nope"}], ids=["none", "wrong"])
def test_sink_admin_token_when_reset_without_valid_token_should_not_clear_and_record_hit(headers):
    async def go():
        async with serve(create_app(admin_token=TOKEN)) as base, ClientSession() as s:
            async with s.get(f"{base}/leak") as r:
                await r.read()
            async with s.post(f"{base}/__reset", headers=headers) as r:
                status, text = r.status, await r.text()
            async with s.get(f"{base}/__hits", headers=AUTH) as r:
                return status, text, await r.json()

    status, text, h = run(go())
    assert (status, text) == (200, "ok")
    assert [(x["method"], x["path"]) for x in h["hits"]] == [("GET", "/leak"), ("POST", "/__reset")]


def test_sink_admin_token_with_valid_token_should_list_and_reset():
    async def go():
        async with serve(create_app(admin_token=TOKEN)) as base, ClientSession() as s:
            async with s.get(f"{base}/leak") as r:
                await r.read()
            async with s.get(f"{base}/__hits", headers=AUTH) as r:
                before = r.status, await r.json()
            async with s.post(f"{base}/__reset", headers=AUTH) as r:
                reset_status = r.status
            async with s.get(f"{base}/__hits", headers=AUTH) as r:
                after = await r.json()
            return before, reset_status, after

    (status, listed), reset_status, after = run(go())
    assert status == 200
    assert listed["count"] == 1
    assert reset_status == 200
    assert after == {"count": 0, "hits": []}


def test_sink_admin_token_when_none_should_leave_routes_open():
    async def go():
        async with serve(create_app()) as base, ClientSession() as s:
            async with s.get(f"{base}/leak") as r:
                await r.read()
            return await hits(s, base)

    assert run(go())["count"] == 1
