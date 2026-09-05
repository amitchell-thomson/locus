"""MCP server (build step 2): tool registration, facet validation, and read tools.

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
    """The cost guard: `query` is the ONLY billable tool and is absent unless opted into.

    Asserted as an invariant (the delta between the two tool sets), not as an exact tool list —
    the exact-set version went stale the moment Loop C added `capture`, which is the wrong
    failure: a new FREE tool must not break the cost-guard test."""
    default_tools = {t.name for t in mcp_server._build()._tool_manager.list_tools()}
    enabled_tools = {t.name for t in mcp_server._build(enable_query=True)._tool_manager.list_tools()}
    assert "query" not in default_tools
    assert enabled_tools - default_tools == {"query"}  # opting in adds exactly `query`
    assert {"retrieve", "list_documents", "inspect_document"} <= default_tools
    # The two write-side tools the desktop app / laptop reach the server FOR: both free.
    assert {"capture", "to_remarkable"} <= default_tools


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
    # Alias substrate not built in this fixture -> graceful hint, not an error (step 12).
    assert "RELATED DOCUMENTS" in out and "locus link" in out


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
        mcp_server, "run_retrieval", lambda q, facets=None, **kw: _stub_result(band=None)
    )
    out = _text(asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"})))
    assert "rerank +4.21" in out
    assert "[paper," in out  # doc 1's category, looked up from the seeded DB
    assert "LOW CONFIDENCE" not in out


def test_retrieve_tool_passes_include_excluded(seeded_db, monkeypatch):
    """The MCP tool exposes include_excluded (default False) so Locus's own source — excluded
    from retrieval by config — can be queried on request, matching the CLI flag."""
    seen = {}
    monkeypatch.setattr(
        mcp_server, "run_retrieval",
        lambda q, facets=None, **kw: seen.update(kw) or _stub_result(band=None),
    )
    asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"}))
    assert seen.get("include_excluded") is False  # default: excluded docs stay out
    asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q", "include_excluded": True}))
    assert seen.get("include_excluded") is True  # opt-in reaches the pipeline


def test_retrieve_tool_flags_low_confidence_by_band(seeded_db, monkeypatch):
    # 'absent': strong wording. 'ambiguous': must NOT claim the corpus lacks the topic
    # (the cross-domain mislabel from the 2026-06-05 second evaluation).
    monkeypatch.setattr(
        mcp_server, "run_retrieval", lambda q, facets=None, **kw: _stub_result(band="absent")
    )
    out = _text(asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"})))
    assert out.startswith("LOW CONFIDENCE") and "very likely does not cover" in out

    monkeypatch.setattr(
        mcp_server, "run_retrieval", lambda q, facets=None, **kw: _stub_result(band="ambiguous")
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
    monkeypatch.setattr(mcp_server, "run_retrieval", lambda q, facets=None, **kw: result)
    blocks = asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"}))
    kinds = [b.type for b in blocks]
    assert kinds == ["text", "text", "image"]
    assert 'figure on p.5 of "Control Notes"' in blocks[1].text
    assert blocks[2].mimeType == "image/png"

    # gate off => text only
    cfg.mcp.include_figure_images = False
    blocks = asyncio.run(mcp_server._build().call_tool("retrieve", {"query": "q"}))
    assert [b.type for b in blocks] == ["text"]


def test_build_stamp_is_nonempty_and_never_raises():
    # In this repo it resolves to a real commit; off-git it degrades to 'unknown' — but it
    # is always a non-empty string and never raises (best-effort startup log).
    stamp = mcp_server._build_stamp()
    assert isinstance(stamp, str) and stamp


def test_run_logs_build_stamp_to_stderr_not_stdout(monkeypatch, capsys):
    # The stamp MUST go to stderr: stdout is the JSON-RPC channel on the stdio transport,
    # so anything printed there would corrupt the protocol. Stub _build so run() doesn't block.
    started = {}
    monkeypatch.setattr(
        mcp_server, "_build",
        lambda enable_query=False: types.SimpleNamespace(run=lambda: started.update(ran=True)),
    )
    mcp_server.run(enable_query=False)
    captured = capsys.readouterr()
    assert started.get("ran")  # the server was actually handed off to .run()
    assert "locus mcp starting" in captured.err and "build" in captured.err
    assert captured.out == ""  # nothing leaked onto the protocol channel


def test_to_remarkable_passes_content_through_and_reports_the_device_path(monkeypatch):
    """The tool takes markdown TEXT, not a path: the desktop app and the laptop have no access
    to this filesystem, so a path-taking tool would look wired and fail on every remote call."""
    import locus.reading.send as send_mod

    seen = {}

    def fake_send(markdown, *, title, folder=None):
        seen.update(markdown=markdown, title=title, folder=folder)
        return send_mod.SentDoc(filename="2026-08-29 Notes.pdf", remote_folder="Notes", pages=2)

    monkeypatch.setattr(send_mod, "send_markdown", fake_send)

    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool(
        "to_remarkable", {"markdown": "# Hi\n\nbody", "title": "Notes"}
    )))

    assert seen["markdown"] == "# Hi\n\nbody" and seen["title"] == "Notes"
    assert "/Notes/2026-08-29 Notes.pdf" in out and "2 pages" in out


def test_to_remarkable_pushes_an_existing_pdf_by_path(monkeypatch):
    """The second mode. A PDF cannot travel as a tool argument — base64 of a 2 MB paper is
    ~2.7 MB the client model would have to emit token by token — so this one takes a path and
    resolves it on the server."""
    import locus.reading.send as send_mod

    seen = {}

    def fake_send_pdf(pdf, *, title=None, folder=None):
        seen.update(pdf=pdf, title=title, folder=folder)
        return send_mod.SentDoc(filename="2026-09-05 Plan.pdf", remote_folder="Inbox", pages=3)

    monkeypatch.setattr(send_mod, "send_pdf", fake_send_pdf)

    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool(
        "to_remarkable", {"pdf_path": "docs/plan.pdf", "title": "Plan"}
    )))

    assert seen["pdf"] == "docs/plan.pdf" and seen["title"] == "Plan"
    assert "/Inbox/2026-09-05 Plan.pdf" in out and "3 pages" in out


def test_to_remarkable_refuses_both_modes_at_once(monkeypatch):
    """Both modes end at the same device folder by the same push, so the only thing that can go
    wrong is the caller meaning one and getting the other. Refuse in words rather than picking."""
    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool(
        "to_remarkable", {"markdown": "# hi", "title": "X", "pdf_path": "a.pdf"}
    )))
    assert "not both" in out


def test_to_remarkable_refuses_neither_mode():
    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool("to_remarkable", {"title": "X"})))
    assert "Nothing to send" in out


def test_to_remarkable_requires_a_title_for_markdown():
    """A markdown send has no filename to fall back on; a PDF send does."""
    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool("to_remarkable", {"markdown": "# hi"})))
    assert "needs a `title`" in out


def test_to_remarkable_returns_the_guidance_when_the_path_is_unresolvable(monkeypatch):
    """A wrong-machine call must come back as the advice, not a stack trace — that message is
    the only thing that distinguishes 'typo' from 'this cannot work from here'."""
    import locus.reading.send as send_mod

    def boom(pdf, *, title=None, folder=None):
        raise FileNotFoundError("no such PDF on the Locus server. Tried:\n  /x.pdf")

    monkeypatch.setattr(send_mod, "send_pdf", boom)

    m = mcp_server._build()
    out = _text(asyncio.run(m.call_tool("to_remarkable", {"pdf_path": "/x.pdf"})))
    assert "Not sent" in out and "no such PDF" in out
