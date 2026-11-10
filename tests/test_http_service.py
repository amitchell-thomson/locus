"""Slice 0.6: the HTTP service refuses any request without the bearer token."""

import asyncio

import httpx

from locus.mcp_server import _Bearer

TOKEN = "t" * 40


async def _ok(scope, receive, send):
    await send({"type": "http.response.start", "status": 200, "headers": []})
    await send({"type": "http.response.body", "body": b"ok"})


def _get(headers: dict) -> int:
    async def go():
        transport = httpx.ASGITransport(app=_Bearer(_ok, TOKEN))
        async with httpx.AsyncClient(transport=transport, base_url="http://server") as c:
            return (await c.get("/mcp", headers=headers)).status_code
    return asyncio.run(go())


def test_bearer_gate(monkeypatch):
    from locus.observe import mcp_log
    monkeypatch.setattr(mcp_log, "record", lambda *a, **k: None)
    assert _get({"Authorization": f"Bearer {TOKEN}"}) == 200
    assert _get({}) == 401
    assert _get({"Authorization": "Bearer wrong"}) == 401
    assert _get({"Authorization": f"bearer {TOKEN}x"}) == 401
