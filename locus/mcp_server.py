"""MCP server — expose the vault to an MCP client (build step 2, CLAUDE.md §16).

WHAT THIS IS
------------
A Model Context Protocol server that makes the Locus corpus callable as *tools* from any MCP
client — Claude Code, the Claude desktop app, or the API. It is the daily-utility surface over
the corpus: instead of shelling into `locus query`, the model calls a tool and pulls the owner's
knowledge into its own context on demand.

ARCHITECTURE (local Claude <-> server-side corpus)
--------------------------------------------------
The MCP *server* runs **where the corpus lives** (it needs the SQLite DB + sqlite-vec, Ollama
for query embedding, and the cross-encoder reranker). The MCP *client* is the local Claude. The
recommended transport is **stdio over SSH**: the client spawns

    ssh locus-server "cd /…/locus && uv run locus mcp"

so the server process runs remotely and its stdio is tunnelled over the SSH connection the owner
already uses — no open ports, no extra auth. The process stays alive for the session, so the
embedder + reranker load once and stay warm (they are process-level caches).

TOOLS
-----
- retrieve       : the core tool. Runs the full hybrid retrieval pipeline and returns the
                   assembled, grounded context + citations. Needs NO Claude key — the *client's*
                   model does the generation, which is the natural MCP pattern.
- query          : OPT-IN. Server-side generation - retrieves and then makes ONE Claude call
                   here. Needs ANTHROPIC_API_KEY and BILLS it. Disabled by default (config
                   [mcp].enable_query / `locus mcp --enable-query`) so the client cannot trigger
                   API spend on a tool that isn't advertised.
- list_documents : what is in the corpus (with date/category facets). FREE (local only).
- inspect_document: what was ingested for one document (synthesis + sections). FREE.

All retrieval tools accept the temporal/category facets (`since`/`until`/`category`).

COST NOTE: the *client* model chooses which tool to call from the advertised list - the server
does not decide. Descriptions only nudge that choice; the hard cost control is not exposing
`query` (the default). retrieve + the read tools are local (Ollama + CPU reranker), so free.
"""

from __future__ import annotations

import json
from datetime import date

from locus.config import load
from locus.db.connection import get_connection
from locus.query import QUERY_MODES
from locus.query import answer as run_answer
from locus.retrieve import Facets
from locus.retrieve import retrieve as run_retrieval

# Bound the size of read-only listings/inspections returned to the client.
_MAX_INSPECT_PROPS = 40
_MAX_INSPECT_ENTITIES = 40


def _build(enable_query: bool = False) -> "FastMCP":  # noqa: F821 - quoted: mcp imported lazily
    """Construct the FastMCP server with its tools registered.

    `enable_query` registers the billable, server-generating `query` tool. It defaults to False
    so the server exposes only the local-only (free) tools — the client cannot call a tool that
    is not advertised, which is the hard guard against surprise API spend.

    The mcp import is deferred into here so importing this module (e.g. for tests, or the CLI
    parser) does not require the `mcp` package to be installed until the server is actually run.
    """
    from mcp.server.fastmcp import FastMCP

    mcp = FastMCP(
        "locus",
        instructions=(
            "Locus is the owner's personal knowledge vault (papers, code, notes, projects, "
            "achievements). Use `retrieve` to pull grounded context + citations into your own "
            "context and answer from it; use `query` to get a finished server-generated answer; "
            "use `list_documents`/`inspect_document` to see what the corpus contains. Prefer "
            "`retrieve` and ground every claim in the returned material with its citation."
        ),
    )

    @mcp.tool()
    def retrieve(
        query: str,
        since: str | None = None,
        until: str | None = None,
        category: str | None = None,
    ):
        """Retrieve grounded context for a query from the knowledge vault.

        Runs hybrid retrieval (dense + lexical + entity -> cross-encoder rerank -> hierarchical
        expansion -> context assembly) and returns the assembled context plus citations, each
        annotated with its document category and rerank score (cross-encoder logit; higher is
        more relevant). When a retrieved unit is a FIGURE (diagram/plot/slide), the actual
        image follows the text as an image content block — interpret it directly; its caption
        and description are in the text context under the same citation. This is the core
        tool: ground your answer in the returned material and cite its sources. Does not call
        any LLM itself — you generate the answer from this context. Results may span multiple
        documents and domains; a bridge you draw between co-retrieved sources is your
        inference, not a stored corpus link — present it as such.

        Args:
            query: The question or topic to retrieve material for.
            since: Optional inclusive lower bound on document date (ISO 'YYYY-MM-DD').
            until: Optional inclusive upper bound on document date (ISO 'YYYY-MM-DD').
            category: Optional document category filter (e.g. 'paper', 'project', 'note').
        """
        facets = _facets(since, until, category)
        result = run_retrieval(query, facets=facets)
        if not result.context:
            return "No relevant material was retrieved for that query (with the given facets)."
        text = f"{_confidence_banner(result)}{result.context}\n\n--- sources ---\n{_sources(result)}"
        return [text, *_figure_images(result)]

    # NOT decorated: registered below only when enable_query is set, so the billable tool is
    # absent from the advertised list by default.
    def query(
        question: str,
        mode: str = "standard",
        since: str | None = None,
        until: str | None = None,
        category: str | None = None,
    ) -> str:
        """Answer a question over the vault with server-side generation (one Claude call).

        Retrieves grounded context and then generates a finished, cited answer ON THE SERVER.
        Requires ANTHROPIC_API_KEY in the server environment. Prefer `retrieve` if you want to
        generate the answer yourself from the raw context.

        Args:
            question: The question to answer.
            mode: Answering persona/framing — one of: standard, gap, synthesis, code, framing,
                project. (Same retrieval; different system prompt.)
            since: Optional inclusive lower bound on document date (ISO 'YYYY-MM-DD').
            until: Optional inclusive upper bound on document date (ISO 'YYYY-MM-DD').
            category: Optional document category filter (e.g. 'paper', 'project', 'note').
        """
        if mode not in QUERY_MODES:
            raise ValueError(f"unknown mode {mode!r}; choose from {sorted(QUERY_MODES)}")
        facets = _facets(since, until, category)
        result = run_answer(question, mode=mode, facets=facets)
        return f"{_confidence_banner(result)}{result.answer}\n\n--- sources ---\n{_sources(result)}"

    if enable_query:
        mcp.add_tool(query)  # opt-in: only now is the billable tool advertised to clients

    @mcp.tool()
    def list_documents(
        category: str | None = None,
        since: str | None = None,
        until: str | None = None,
    ) -> str:
        """List the documents in the vault, optionally filtered by category / date range.

        Args:
            category: Optional document category filter (e.g. 'paper', 'project', 'note').
            since: Optional inclusive lower bound on document date (ISO 'YYYY-MM-DD').
            until: Optional inclusive upper bound on document date (ISO 'YYYY-MM-DD').
        """
        _validate_dates(since, until)
        clauses: list[str] = []
        params: list[str] = []
        if category:
            clauses.append("d.category = ?")
            params.append(category)
        if since:
            clauses.append("d.source_date >= ?")
            params.append(since)
        if until:
            clauses.append("d.source_date <= ?")
            params.append(until)
        where = ("WHERE " + " AND ".join(clauses)) if clauses else ""
        conn = get_connection(load().paths.db)
        try:
            rows = conn.execute(
                f"""
                SELECT d.id, d.title, d.source_type, d.source_date, d.category,
                  (SELECT COUNT(*) FROM sections s     WHERE s.doc_id=d.id) AS secs,
                  (SELECT COUNT(*) FROM chunks c       WHERE c.doc_id=d.id) AS chunks,
                  (SELECT COUNT(*) FROM propositions p WHERE p.doc_id=d.id) AS props,
                  (SELECT COUNT(*) FROM entities e     WHERE e.doc_id=d.id) AS ents
                FROM documents d {where} ORDER BY d.id
                """,
                params,
            ).fetchall()
        finally:
            conn.close()
        if not rows:
            return "No documents match." if clauses else "No documents ingested yet."
        lines = [
            f"[{r['id']}] {r['title']} — {r['source_type']}, "
            f"date {r['source_date'] or '—'}, category {r['category'] or '—'} "
            f"({r['secs']} sections, {r['chunks']} chunks, {r['props']} props, {r['ents']} entities)"
            for r in rows
        ]
        return f"{len(rows)} document(s):\n" + "\n".join(lines)

    @mcp.tool()
    def inspect_document(doc: str, section: int | None = None) -> str:
        """Show what was ingested for one document: synthesis, gaps, and per-section detail.

        Args:
            doc: Document id, or a substring of its title / source path.
            section: Optional 0-based section position to restrict the detail to one section.
        """
        conn = get_connection(load().paths.db)
        try:
            return _inspect(conn, doc, section)
        finally:
            conn.close()

    return mcp


# --- shared helpers ----------------------------------------------------------------------


def _confidence_banner(result) -> str:
    """The coverage warning the 2026-06-05 evaluation found missing: weak matches must not
    arrive looking like strong ones. Empty when retrieval is confident. Wording is shared
    (retrieve.confidence_banner) and band-aware, so cross-domain queries aren't mislabelled
    as absent topics."""
    from locus.retrieve.pipeline import confidence_banner

    text = confidence_banner(getattr(result, "confidence_band", None))
    return f"{text}\n\n" if text else ""


def _figure_images(result) -> list:
    """Retrieved figures as MCP content: a text label + an Image block per figure.

    The actual image rides along with the assembled text (tier 3, §15.1) so the client
    model can interpret the figure directly. FastMCP flattens a mixed [str | Image] return
    into TextContent/ImageContent blocks. Gated by [mcp].include_figure_images; a missing
    or unreadable PNG silently degrades to text-only (the figure's caption+description are
    already in the context).
    """
    figures = getattr(result, "figures", None)
    if not figures or not load().mcp.include_figure_images:
        return []
    from mcp.server.fastmcp import Image  # lazy, like the FastMCP import in _build

    from locus.retrieve.figure_images import load_figure_png

    out: list = []
    attached = 0
    for fig in figures:  # already best-first and capped at [figures].image_cap
        png = load_figure_png(fig.raw_path)
        if png is None:
            continue
        unit = "slide " if fig.kind == "slide" else "figure on p."
        out.append(f"[{unit}{fig.page} of \"{fig.doc_title}\"]")
        out.append(Image(data=png, format="png"))
        attached += 1
    cited = getattr(result, "figures_cited", 0)
    if attached and cited > attached:
        # Say the truncation out loud: a "[figure on p.N]" citation without an attached
        # image otherwise reads as drift (2026-06-06 audit finding).
        out.append(
            f"({cited} figures cited above; the {attached} most relevant attached as images)"
        )
    return out


def _sources(result) -> str:
    """Format citations annotated with document category + best rerank score.

    Falls back to the plain citation strings when details are absent (e.g. a stubbed result).
    """
    details = getattr(result, "citation_details", None)
    if not details:
        return "\n".join(f"- {c}" for c in result.citations) or "- (none)"
    doc_ids = sorted({d.doc_id for d in details})
    categories: dict[int, str | None] = {}
    conn = get_connection(load().paths.db)
    try:
        placeholders = ",".join("?" * len(doc_ids))
        for row in conn.execute(
            f"SELECT id, category FROM documents WHERE id IN ({placeholders})", doc_ids
        ):
            categories[row["id"]] = row["category"]
    finally:
        conn.close()
    lines = []
    for d in details:
        tags = [t for t in (categories.get(d.doc_id),) if t]
        tags.append(f"rerank {d.rerank_score:+.2f}" if d.rerank_score is not None else "rerank n/a")
        lines.append(f"- {d.text} [{', '.join(tags)}]")
    return "\n".join(lines)


def _validate_dates(since: str | None, until: str | None) -> None:
    for label, value in (("since", since), ("until", until)):
        if value is not None:
            try:
                date.fromisoformat(value)
            except ValueError as exc:
                raise ValueError(f"{label} must be an ISO date (YYYY-MM-DD); got {value!r}") from exc


def _facets(since: str | None, until: str | None, category: str | None) -> Facets | None:
    """Build a validated Facets, or None when no facet is set (unrestricted retrieval)."""
    _validate_dates(since, until)
    facets = Facets(since=since, until=until, category=category)
    return facets if facets.active() else None


def _resolve_doc(conn, ident: str):
    """Resolve a document row by numeric id, or by a unique substring of title/source path."""
    if ident.isdigit():
        return conn.execute("SELECT * FROM documents WHERE id=?", (int(ident),)).fetchone()
    rows = conn.execute(
        "SELECT * FROM documents WHERE title LIKE ? OR source_uri LIKE ? ORDER BY id",
        (f"%{ident}%", f"%{ident}%"),
    ).fetchall()
    if len(rows) == 1:
        return rows[0]
    return None  # zero or ambiguous -> caller reports


def _inspect(conn, ident: str, section: int | None) -> str:
    doc = _resolve_doc(conn, ident)
    if doc is None:
        return f"No unique document matches {ident!r}. Use `list_documents` to find its id."

    doc_id = doc["id"]
    out: list[str] = [
        f"[{doc_id}] {doc['title']}",
        f"  source   : {doc['source_uri']}",
        f"  type     : {doc['source_type']} | date {doc['source_date'] or '—'} | "
        f"category {doc['category'] or '—'}",
        f"  ingested : {doc['ingested_at']} (model {doc['ingest_model']})",
        "",
        "SYNTHESIS",
    ]
    for field in ("thesis", "method", "result", "limitations"):
        out.append(f"  {field:<12}: {doc[field]}")
    gaps = json.loads(doc["gap_flags"] or "[]")
    out.append(f"  gaps ({len(gaps)}):")
    out.extend(f"    - {g}" for g in gaps)

    from locus.link.related import format_related

    out.append("")
    out.extend(format_related(conn, doc_id))

    section_map = {m["position"]: m for m in json.loads(doc["section_map"] or "[]")}
    rows = conn.execute(
        "SELECT * FROM sections WHERE doc_id=? ORDER BY position", (doc_id,)
    ).fetchall()
    by_pos = {s["position"]: s for s in rows}
    positions = [section] if section is not None else sorted(by_pos)

    for pos in positions:
        s = by_pos.get(pos)
        if s is None:
            continue
        pm = section_map.get(pos, {})
        out.append(
            f"\n### Section {pos}: {s['title']!r} "
            f"(pp {pm.get('page_start', '?')}-{pm.get('page_end', '?')})"
        )
        out.append(f"  SUMMARY: {s['summary']}")
        props = conn.execute(
            "SELECT text FROM propositions WHERE section_id=? ORDER BY position LIMIT ?",
            (s["id"], _MAX_INSPECT_PROPS),
        ).fetchall()
        out.append(f"  PROPOSITIONS ({len(props)}):")
        out.extend(f"    - {p['text']}" for p in props)
        ents = conn.execute(
            "SELECT name, type FROM entities WHERE section_id=? ORDER BY type, name LIMIT ?",
            (s["id"], _MAX_INSPECT_ENTITIES),
        ).fetchall()
        out.append(f"  ENTITIES ({len(ents)}):")
        out.extend(f"    - {e['name']} ({e['type']})" for e in ents)
        figs = conn.execute(
            "SELECT page, kind, caption, description FROM figures "
            "WHERE section_id=? ORDER BY position",
            (s["id"],),
        ).fetchall()
        if figs:
            out.append(f"  FIGURES ({len(figs)}):")
            for f in figs:
                label = f"p.{f['page']} {f['kind']}"
                body = f["caption"] or f["description"] or "(no caption/description)"
                out.append(f"    - [{label}] {body}")

    return "\n".join(out)


def run(enable_query: bool = False) -> None:
    """Run the MCP server over stdio (the transport used for stdio-over-SSH).

    `enable_query` opts into the billable server-side `query` tool (default off).
    """
    _build(enable_query=enable_query).run()
