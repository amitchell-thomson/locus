"""MCP server (PLAN.md step 2): tool registration, facet validation, and read tools.

The tools are thin wrappers over retrieve()/answer() (covered elsewhere) plus read-only DB
queries. These tests cover the wiring: the right tools are registered, facet validation rejects
bad dates, and the read tools render a seeded DB correctly — no Ollama / Claude key needed.
"""

import asyncio
import types
from pathlib import Path

import pytest

from locus import mcp_server
from locus.db.connection import get_connection
from locus.db.migrate import migrate


def _seed(conn):
    conn.execute(
        "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title,"
        " ingest_model, source_date, category, thesis, method, result, limitations, gap_flags,"
        " section_map) VALUES "
        "(1,'h','pdf','/in/papers/control.pdf','p','Control Notes','m','2023-06-01','paper',"
        " 'THESIS','METHOD','RESULT','LIMITS','[\"missing proof\"]',"
        " '[{\"position\":0,\"title\":\"Stability\",\"page_start\":5,\"page_end\":7}]')"
    )
    conn.execute(
        "INSERT INTO sections (id, doc_id, position, title, summary) "
        "VALUES (1,1,0,'Stability','poles determine stability')"
    )
    conn.execute(
        "INSERT INTO propositions (id, section_id, doc_id, position, text, embed_model) "
        "VALUES (1,1,1,0,'Stability is determined by the poles.','m')"
    )
    conn.execute(
        "INSERT INTO entities (doc_id, section_id, name, type) "
        "VALUES (1,1,'Nyquist criterion','theorem')"
    )
    conn.commit()


@pytest.fixture()
def seeded_db(tmp_path: Path, monkeypatch):
    db = tmp_path / "mcp.db"
    migrate(db)
    c = get_connection(db)
    _seed(c)
    c.close()
    # Point the server's config loader at the seeded DB.
    cfg = types.SimpleNamespace(paths=types.SimpleNamespace(db=db))
    monkeypatch.setattr(mcp_server, "load", lambda *a, **k: cfg)
    return db


def _text(call_result) -> str:
    """call_tool returns (content_blocks, ...); pull the first text block."""
    blocks = call_result[0]
    return blocks[0].text


def test_query_tool_is_opt_in():
    # Default: the billable `query` tool is NOT advertised (hard cost guard).
    default_tools = {t.name for t in mcp_server._build()._tool_manager.list_tools()}
    assert default_tools == {"retrieve", "list_documents", "inspect_document"}
    assert "query" not in default_tools
    # Opting in adds exactly `query`.
    enabled_tools = {t.name for t in mcp_server._build(enable_query=True)._tool_manager.list_tools()}
    assert enabled_tools == {"retrieve", "query", "list_documents", "inspect_document"}


def test_facets_validation_and_active():
    assert mcp_server._facets(None, None, None) is None  # nothing set -> unrestricted
    f = mcp_server._facets("2023-01-01", None, "paper")
    assert f is not None and f.since == "2023-01-01" and f.category == "paper"
    with pytest.raises(ValueError):
        mcp_server._facets("not-a-date", None, None)


def test_list_documents_renders_and_filters(seeded_db):
    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool("list_documents", {})))
    assert "Control Notes" in out and "category paper" in out
    # A non-matching category yields the explicit no-match message.
    out2 = _text(asyncio.run(m.call_tool("list_documents", {"category": "project"})))
    assert "No documents match" in out2


def test_inspect_document_shows_synthesis_and_sections(seeded_db):
    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool("inspect_document", {"doc": "Control"})))
    assert "THESIS" in out
    assert "Stability is determined by the poles." in out
    assert "Nyquist criterion (theorem)" in out
    assert "missing proof" in out  # gap flag


def test_inspect_document_unknown_is_reported(seeded_db):
    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool("inspect_document", {"doc": "no-such-doc"})))
    assert "No unique document matches" in out
