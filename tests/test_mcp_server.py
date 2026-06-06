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
    """Pull the first text block. call_tool returns (content_blocks, structured) for
    str-returning tools but a plain block list for list-returning tools (retrieve, which
    may append ImageContent figure blocks) — handle both shapes."""
    blocks = call_result[0]
    if not isinstance(blocks, list):
        return blocks.text
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


def _stub_result(band: str | None = None):
    from locus.retrieve.assemble import Citation

    cite = '"Control Notes", §Stability, pp 5–7'
    return types.SimpleNamespace(
        context="CTX",
        citations=[cite],
        citation_details=[Citation(text=cite, doc_id=1, rerank_score=4.21)],
        survivors=[],
        low_confidence=band is not None,
        confidence_band=band,
    )


def test_retrieve_tool_annotates_category_and_score(seeded_db, monkeypatch):
    monkeypatch.setattr(
        mcp_server, "run_retrieval", lambda q, facets=None: _stub_result(band=None)
    )
    out = _text(asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"})))
    assert "rerank +4.21" in out
    assert "[paper," in out  # doc 1's category, looked up from the seeded DB
    assert "LOW CONFIDENCE" not in out


def test_retrieve_tool_flags_low_confidence_by_band(seeded_db, monkeypatch):
    # 'absent': strong wording. 'ambiguous': must NOT claim the corpus lacks the topic
    # (the cross-domain mislabel from the 2026-06-05 second evaluation).
    monkeypatch.setattr(
        mcp_server, "run_retrieval", lambda q, facets=None: _stub_result(band="absent")
    )
    out = _text(asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"})))
    assert out.startswith("LOW CONFIDENCE") and "very likely does not cover" in out

    monkeypatch.setattr(
        mcp_server, "run_retrieval", lambda q, facets=None: _stub_result(band="ambiguous")
    )
    out = _text(asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"})))
    assert out.startswith("LOW CONFIDENCE") and "covers the parts separately" in out
    assert "does not cover" not in out


def test_sources_fall_back_to_plain_citations():
    # A result without citation_details (e.g. older callers) renders the plain strings.
    result = types.SimpleNamespace(citations=["plain cite"], citation_details=[])
    assert mcp_server._sources(result) == "- plain cite"


def test_retrieve_tool_attaches_figure_images(seeded_db, monkeypatch, tmp_path):
    """A figure survivor rides along as an ImageContent block (step 11 tier 3 over MCP)."""
    import pymupdf

    from locus.retrieve.pipeline import RetrievedFigure

    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 120, 80))
    pix.clear_with(100)
    (tmp_path / "h_fig0.png").write_bytes(pix.tobytes("png"))

    cfg = types.SimpleNamespace(
        paths=types.SimpleNamespace(db=seeded_db, raw_store=tmp_path),
        mcp=types.SimpleNamespace(include_figure_images=True),
    )
    monkeypatch.setattr(mcp_server, "load", lambda *a, **k: cfg)
    # figure_images.load_figure_png uses its own config import
    import locus.retrieve.figure_images as fi

    monkeypatch.setattr(fi, "load", lambda *a, **k: cfg)

    result = _stub_result()
    result.figures = [
        RetrievedFigure(raw_path="h_fig0.png", page=5, kind="vector",
                        caption="Figure 1", doc_title="Control Notes", rerank_score=4.0),
    ]
    monkeypatch.setattr(mcp_server, "run_retrieval", lambda q, facets=None: result)
    blocks = asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"}))
    kinds = [b.type for b in blocks]
    assert kinds == ["text", "text", "image"]
    assert 'figure on p.5 of "Control Notes"' in blocks[1].text
    assert blocks[2].mimeType == "image/png"

    # gate off => text only
    cfg.mcp.include_figure_images = False
    blocks = asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"}))
    assert [b.type for b in blocks] == ["text"]
