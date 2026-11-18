"""One locus MCP (18 Nov): kairos's and keryx's tools served through locus's, prefixed."""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

from mcp import types

from locus import mcp_gateway as G


def _gw(tmp_path, prefixes=("kairos", "keryx")):
    bs = []
    for p in prefixes:
        f = tmp_path / f"{p}-token"
        f.write_text("tok\n")
        bs.append(G.Backend(prefix=p, url=f"http://x/{p}/mcp", token_file=str(f)))
    bs.append(G.Backend(prefix="nope", url="http://x", token_file=str(tmp_path / "missing")))
    return G.Gateway(bs)


class FakeSession:
    def __init__(self, prefix, calls):
        self.prefix, self.calls = prefix, calls

    async def list_tools(self):
        return SimpleNamespace(tools=[types.Tool(name="search", description="find", inputSchema={"type": "object"}),
                                      types.Tool(name="undo", description="undo", inputSchema={"type": "object"})])

    async def call_tool(self, name, args):
        self.calls.append((self.prefix, name, args))
        return types.CallToolResult(content=[types.TextContent(type="text", text=f"{self.prefix}:{name}")])


def _fake(gw, calls, down=()):
    async def session(b, fn):
        if b.prefix in down:
            raise ConnectionError("no route")
        return await fn(FakeSession(b.prefix, calls), SimpleNamespace(instructions=f"{b.prefix} rules"))
    gw._session = session


def test_tools_are_prefixed_and_a_backend_without_its_token_is_not_served(tmp_path):
    gw = _gw(tmp_path)
    assert set(gw.backends) == {"kairos", "keryx"}
    _fake(gw, [])
    names = [t.name for t in asyncio.run(gw.tools())]
    assert names == ["kairos_search", "kairos_undo", "keryx_search", "keryx_undo"]
    assert gw.owns("keryx_search") == "keryx" and gw.owns("search") is None and gw.owns("nope_x") is None


def test_a_call_reaches_its_backend_under_its_own_name_and_returns_its_result_untouched(tmp_path):
    gw, calls = _gw(tmp_path), []
    _fake(gw, calls)
    r = asyncio.run(gw.call("keryx_undo", {"id": "c1"}))
    assert calls == [("keryx", "undo", {"id": "c1"})]
    assert r.content[0].text == "keryx:undo" and not r.isError


def test_a_backend_that_is_down_says_which_and_why_and_keeps_the_others(tmp_path):
    gw = _gw(tmp_path)
    _fake(gw, [], down=("keryx",))
    names = [t.name for t in asyncio.run(gw.tools())]
    assert names == ["kairos_search", "kairos_undo"]
    r = asyncio.run(gw.call("keryx_search", {}))
    assert r.isError and "keryx is unreachable" in r.content[0].text and "laptop" in r.content[0].text


def test_their_instructions_are_appended_and_the_fixed_rules_stand_in_when_one_is_down(tmp_path):
    gw = _gw(tmp_path)
    _fake(gw, [], down=("keryx",))
    text = gw.instructions()
    assert "kairos rules" in text and "call kairos_<that name>" in text
    assert "You cannot send" in text                                     # keryx's fallback


def test_the_server_lists_and_routes_both(tmp_path, monkeypatch):
    from locus import mcp_server

    gw = _gw(tmp_path, ("kairos",))
    calls = []
    _fake(gw, calls)
    monkeypatch.setattr(mcp_server, "_gateway_config", lambda: [])
    mcp = mcp_server._build()
    G.install(mcp, gw)
    handler = mcp._mcp_server.request_handlers[types.ListToolsRequest]
    res = asyncio.run(handler(types.ListToolsRequest(method="tools/list")))
    names = {t.name for t in res.root.tools}
    assert {"context", "search", "kairos_search", "kairos_undo"} <= names
    call = mcp._mcp_server.request_handlers[types.CallToolRequest]
    out = asyncio.run(call(types.CallToolRequest(method="tools/call", params=types.CallToolRequestParams(
        name="kairos_undo", arguments={"id": "c1"}))))
    assert calls == [("kairos", "undo", {"id": "c1"})] and out.root.content[0].text == "kairos:undo"
