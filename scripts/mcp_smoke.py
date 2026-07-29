"""Drive a real `locus mcp` server over stdio and exercise the Phase-2 tools.

Throwaway verification harness: spawns the server exactly as an MCP client would (stdio), lists
the advertised tools, and calls one. Proves the MCP wiring end-to-end — tool registration, schema,
transport, and the handler — rather than calling the Python function directly, which is all the
unit tests can do.

Usage:  uv run python scripts/mcp_smoke.py [tool] [json-args]
"""

from __future__ import annotations

import asyncio
import json
import sys


async def main() -> None:
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client

    tool = sys.argv[1] if len(sys.argv) > 1 else None
    args = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}

    params = StdioServerParameters(command="uv", args=["run", "locus", "mcp"])
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            listing = await session.list_tools()
            names = sorted(t.name for t in listing.tools)
            print(f"ADVERTISED TOOLS ({len(names)}): {', '.join(names)}\n")
            if tool is None:
                return
            print(f"--- calling {tool}({json.dumps(args)}) ---\n", flush=True)
            result = await session.call_tool(tool, args)
            for block in result.content:
                print(getattr(block, "text", f"[{block.type} block]"))


if __name__ == "__main__":
    asyncio.run(main())
