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
- capture        : save this conversation into the vault as a rough note (Loop C). FREE.
- to_remarkable  : push markdown (rendered) or an existing PDF to the tablet. FREE.
- annotations    : what he marked up on the tablet, and the inked pages as images. FREE.
- critique       : stress-test a project/reasoning against the owner's own corpus (§8.4).
- synthesise     : "what do I know and think about X", incl. the dated belief trajectory.
- objects        : the structured project/concept/question/reading overlays. FREE.
- evolution      : the dated position trajectory for a concept/project. FREE (unless tensions).

All retrieval tools accept the temporal/category facets (`since`/`until`/`category`).

COST NOTE: the *client* model chooses which tool to call from the advertised list - the server
does not decide. Descriptions only nudge that choice; the hard cost control is not exposing
`query` (the default). retrieve + the read tools are local (Ollama + CPU reranker), so free.

`critique`/`synthesise` DO call a model, but through `claude -p` — the owner's SUBSCRIPTION,
not ANTHROPIC_API_KEY (agent/claude.py scrubs that key from the subprocess env precisely so it
cannot happen). That is why they are advertised by default while `query`, which bills the
metered API, stays opt-in. The invariant the default tool list protects is unchanged: no
advertised tool can spend against the API key.
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
            "use `list_documents`/`inspect_document` to see what the corpus contains; use "
            "`capture` to save this conversation's decisions into the vault as a rough note, and "
            "`to_remarkable` to send markdown or an existing PDF to his tablet to read on paper, and "
            "`annotations` to read back what he highlighted and wrote in the margins, "
            "including the marked-up pages as images. "
            "For the owner's own thinking rather than raw material: `critique` stress-tests a "
            "project or argument against what he has read and concluded, `synthesise` gives what "
            "he knows and thinks about a topic including how his view has changed, `objects` "
            "lists his projects/concepts/questions, and `evolution` shows the dated trajectory "
            "of his positions. Prefer `retrieve` and ground every claim in the returned material "
            "with its citation."
        ),
    )

    @mcp.tool()
    def retrieve(
        query: str,
        since: str | None = None,
        until: str | None = None,
        category: str | None = None,
        include_excluded: bool = False,
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
            include_excluded: Default False. Some documents are excluded from retrieval by
                config (e.g. Locus's OWN source code, kept out so it doesn't compete with the
                owner's knowledge). Set True ONLY when the query is explicitly about Locus's
                own implementation/source; leave False for everything else.
        """
        facets = _facets(since, until, category)
        result = run_retrieval(query, facets=facets, include_excluded=include_excluded)
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

    @mcp.tool()
    def capture(content: str, title: str, project: str | None = None) -> str:
        """Save this conversation (or a decision-summary of it) into Locus as a rough note (Loop C).

        Use when the owner wants to preserve reasoning, decisions, or conclusions from this
        conversation into their knowledge vault. WRITE-TO-INBOX ONLY — this does NOT ingest or
        modify the corpus; it drops a markdown note into the capture inbox that the next note-sync
        picks up (as maturity=rough, so it informs retrieval without drowning authoritative
        sources). Prefer capturing a concise decision-summary (what was decided/concluded and why,
        open questions) over the raw transcript.

        Args:
            content: The markdown to save — ideally a decision-summary of the conversation.
            title: A short, descriptive title for the note.
            project: Optional project tag (provenance), e.g. 'regime-ml'.
        """
        from locus.capture.conversations import capture_conversation

        cap = capture_conversation(content, title=title, project=project, source="claude")
        return f"Captured '{cap.title}' to {cap.path} (rough note; ingested on the next note-sync)."

    @mcp.tool()
    def to_remarkable(
        markdown: str | None = None,
        title: str | None = None,
        pdf_path: str | None = None,
        folder: str | None = None,
    ) -> str:
        """Send a document to the owner's reMarkable to read on paper. Two modes.

        MARKDOWN (`markdown=`): pass the TEXT of a document you wrote or read — a summary, a
        plan, an answer, the contents of a file. It is rendered to a device-tuned PDF. This mode
        works from ANY machine, because the text travels with the call. Prefer it whenever what
        you want to send is prose you are holding.

        EXISTING PDF (`pdf_path=`): pass a path to a PDF that already exists — one you just
        generated, or one in the repo or vault. It is pushed unchanged, not re-rendered. The
        path is resolved ON THE LOCUS SERVER: absolute, or relative to the server's working
        directory, or relative to the Locus checkout root ('docs/plan.pdf'). It therefore works
        only when you are running on that machine. If you are not, the error will say so — do
        not retry with a different path, send markdown instead.

        Pass exactly one of `markdown` or `pdf_path`.

        This is DELIVERY ONLY — it does not ingest, capture, or change the corpus (use `capture`
        for that). The document lands in its own device folder, deliberately not in the daily
        page's inbox and not in the reading folders whose contents are auto-ingested.

        Args:
            markdown: The markdown to render. Headings, lists, tables, code and LaTeX math work.
            title: Short title — names the file on the device. Required with `markdown` (it also
                heads page 1); optional with `pdf_path`, where it defaults to the filename.
            pdf_path: Path on the Locus server to an existing PDF to push unchanged.
            folder: Optional device folder override (default `[reading].send_folder`).
        """
        from locus.reading.send import send_markdown, send_pdf

        # Both modes reach the same device folder by the same push, so the ONLY thing that can
        # go wrong here is the caller meaning one and getting the other. Refuse the ambiguous
        # calls in words rather than picking a winner silently.
        if markdown and pdf_path:
            return ("Pass either `markdown` or `pdf_path`, not both — they are two ways to send "
                    "one document and I cannot tell which you meant.")
        if not markdown and not pdf_path:
            return ("Nothing to send: pass `markdown` (the text of the document) or `pdf_path` "
                    "(a PDF on the Locus server).")

        try:
            if pdf_path:
                sent = send_pdf(pdf_path, title=title, folder=folder)
                verb, what = "Pushed", "unchanged"
            else:
                if not title:
                    return "Sending markdown needs a `title` — it names the file and heads page 1."
                sent = send_markdown(markdown, title=title, folder=folder)
                verb, what = "Sent", "rendered"
        except (FileNotFoundError, ValueError) as exc:
            # These are the caller's to fix (wrong path, wrong machine, not a PDF, empty text),
            # and the message says which. Returning it beats raising: the client sees the
            # guidance instead of a stack trace.
            return f"Not sent — {exc}"

        pages = f"{sent.pages} page{'s' if sent.pages != 1 else ''}" if sent.pages else what
        return f"{verb} '{sent.filename}' ({pages}) to reMarkable:{sent.device_path}."

    @mcp.tool()
    def annotations(
        document: str,
        page: int | None = None,
        intent: str | None = None,
        images: bool = False,
        max_images: int = 3,
    ):
        """Read back what he marked up on his reMarkable — the notes, and the pages themselves.

        Every highlight, underline, bracket and margin note he has made on a document is stored
        with the words the ink covered, the whole line it sat on, his transcribed handwriting,
        and a classified intent (`important` / `not_understood` / `idea`). This reads that back.

        Set `images=True` to also SEE the marked-up pages: the ink is composited onto the PDF
        from the device's cloud copy and attached as image blocks. Do that whenever the text
        alone is thin — most of all when a mark is reported as covering no text, which almost
        always means it marks a FIGURE and the ink is the only record of what he meant. Reading
        the image is how you answer "what is he pointing at here".

        The two registers are complementary, so both come back together: images carry arrows,
        diagrams and position; text carries margin writing that runs off the edge of the page,
        which the composited image clips.

        Images cost a live device fetch (a few seconds) and context, so they are off by default
        and capped. Text alone is a local database read — free and instant.

        Args:
            document: Which document — a title fragment ('portfolio') or an exact source_uri.
                Call with an empty string to list everything he has annotated.
            page: Optional 1-based page number, as printed, to restrict to one page.
            intent: Optional filter — 'important', 'not_understood', or 'idea'.
            images: Attach the annotated pages as images. Default False.
            max_images: How many pages to attach (capped by [mcp].annotation_image_cap).
        """
        from locus.capture import review

        conn = get_connection(load().paths.db)
        try:
            candidates = review.resolve(conn, document)
            if not candidates:
                known = review.documents_with_marks(conn)
                if not known:
                    return "No annotations have been captured from the device yet."
                listing = "\n".join(f"  {n:4} marks — {t}" for _, t, n in known)
                return f"No annotated document matches {document!r}. Annotated so far:\n{listing}"
            if len(candidates) > 1:
                listing = "\n".join(f"  {n:4} marks — {t}" for _, t, n in candidates)
                return (f"{document!r} matches several annotated documents — say which:\n"
                        f"{listing}")

            uri = candidates[0][0]
            doc = review.load(conn, uri, page=page, intent=intent)
            text = doc.render()
            if not images or not doc.marks:
                return text

            cap = max(1, min(max_images, load().mcp.annotation_image_cap))
            wanted = _pages_to_image(doc, cap)
            try:
                pngs = review.annotated_page_pngs(
                    review.locate_device_copy(conn, doc)[0], wanted
                )
            except Exception as exc:
                # LOUD, not silent. An images request that comes back as text with no
                # explanation is indistinguishable from a document with no ink on it.
                return (f"{text}\n\n[the pages could not be fetched from the device: {exc}. "
                        "The marks above are from the last sweep and are still accurate.]")

            from mcp.server.fastmcp import Image

            out: list = [text]
            for idx in wanted:
                png = pngs.get(idx)
                if png is None:
                    continue
                out.append(f'[p.{idx + 1} of "{doc.title}", his ink composited on]')
                out.append(Image(data=png, format="png"))
            if len(doc.page_indexes) > len(wanted):
                out.append(f"({len(doc.page_indexes)} pages carry marks; the {len(wanted)} "
                           "most informative are attached. Ask for a specific `page` for others.)")
            return out
        finally:
            conn.close()

    # --- Phase-2 value surfaces (agent-layer §8.4) -----------------------------------------
    # critique/synthesise ground in-process (free, local) and then make ONE `claude -p` call.
    # That call runs on the owner's SUBSCRIPTION, not the metered API key — which is why they
    # are advertised by default while `query` (metered) stays opt-in. The cost guard's shape is
    # unchanged: no tool here can spend against ANTHROPIC_API_KEY.

    @mcp.tool()
    def critique(target: str, object_id: int | None = None) -> str:
        """Stress-test a project or a piece of reasoning against the owner's OWN corpus.

        Use when the owner wants his thinking challenged — a project's approach, a conclusion he
        has drawn, an argument he is about to make. Grounds in his corpus first (retrieval +
        structured objects + his recorded positions + detected gaps), then produces challenges
        that each cite a specific piece of his material. A challenge that cannot be grounded is
        DISCARDED rather than shown, so the absence of challenges means the corpus does not
        support one — not that the reasoning is sound.

        Args:
            target: What to critique — a project name, or the reasoning/claim in full.
            object_id: Optional structured-object id to centre the critique on (see `objects`).
        """
        from locus.surface.critique import critique as run_critique

        conn = get_connection(load().paths.db)
        try:
            result = run_critique(conn, target, object_id=object_id)
        finally:
            conn.close()
        note = "\n\n_(model call degraded; showing only the deterministic half)_" if result.degraded else ""
        return result.render() + note

    @mcp.tool()
    def synthesise(topic: str, with_practice: bool = False) -> str:
        """What the owner knows and THINKS about a topic, including how his view has changed.

        Not a general explanation of the topic — an account of what he has read, built, and
        concluded, every point cited to his own material, plus the dated trajectory of his
        positions (what he used to think, what changed it). Set `with_practice=True` to also
        generate recall questions from his own stored propositions.

        Args:
            topic: The topic to synthesise (e.g. 'portfolio construction').
            with_practice: Also generate practice questions from his propositions.
        """
        from locus.surface.synthesise import synthesise as run_synthesis

        conn = get_connection(load().paths.db)
        try:
            result = run_synthesis(conn, topic, with_practice=with_practice)
        finally:
            conn.close()
        note = "\n\n_(model call degraded; showing only the deterministic half)_" if result.degraded else ""
        return result.render() + note

    @mcp.tool()
    def objects(type: str | None = None, status: str | None = None, limit: int = 25) -> str:
        """List the owner's structured objects — projects, concepts, questions, ideas, readings.
        FREE.

        These are agent-proposed and human-blessed overlays on the corpus: a project carries its
        approach, open threads and learnings; a concept carries mastery; an IDEA is something he
        might build, usually born from a margin note. `status='proposed'` shows what is awaiting
        his blessing. Read-only — blessing happens through the CLI.

        Asked for his IDEAS, look in two places: `type='idea'`, and the `open thread` lines on
        `type='project'` — a next move he writes against a project he is already building is
        recorded there, not as an idea object. This listing omitted 'idea' from its documented
        type list until 2026-08-10, which made the whole reading-born half of his thinking
        invisible to anything reading this docstring to decide what to ask for.

        Args:
            type: Filter by 'project' | 'concept' | 'question' | 'idea' | 'reading'.
            status: Filter by 'proposed' | 'active' | 'archived'.
            limit: Max objects to return.
        """
        from locus.agent import state

        conn = get_connection(load().paths.db)
        try:
            rows = state.list_objects(conn, type_=type, status=status, limit=limit)
            if not rows:
                return "No structured objects match. Run `locus structure` to propose some."
            out = []
            for obj in rows:
                out.append(f"[{obj.id}] {obj.status} {obj.type}: {obj.title}")
                for key in ("why", "approach", "mastery", "state"):
                    if obj.body.get(key):
                        out.append(f"    {key}: {obj.body[key]}")
                for thread in obj.body.get("open_threads", []):
                    out.append(f"    open thread: {thread}")
                for learning in obj.body.get("learnings", []):
                    out.append(f"    learning: {learning}")
                for link in state.links_for(conn, obj.id):
                    out.append(f"    -> {link.relation} {link.target_kind}:{link.target_key}")
            return "\n".join(out)
        finally:
            conn.close()

    @mcp.tool()
    def evolution(subject: str | None = None, tensions: bool = False) -> str:
        """The owner's DATED position trajectory on a concept or project. FREE unless `tensions`.

        Shows what he thought and when, oldest first, each with the note it came from — the
        record of how his understanding actually moved. Omit `subject` to list every subject
        that has a trajectory. `tensions=True` additionally runs a judged check for stored
        claims that contradict his latest position (one model call; advisory only).

        Args:
            subject: Concept name or project title. Omit to list all subjects.
            tensions: Also check the latest position for contradictions.
        """
        from locus.evolve.trajectory import (
            all_trajectories, build_trajectory, render_trajectory, resolve_subject,
        )

        conn = get_connection(load().paths.db)
        try:
            if not subject:
                trajectories = all_trajectories(conn)
                if not trajectories:
                    return "No belief positions recorded yet."
                return "\n".join(
                    f"- [{t.subject_kind}] {t.label} ({len(t.entries)} positions, "
                    f"{t.entries[0].dated_at} → {t.entries[-1].dated_at})"
                    for t in trajectories
                )
            kind, key = resolve_subject(conn, subject)
            if key is None:
                return f"No trajectory recorded for {subject!r}."
            return render_trajectory(build_trajectory(conn, kind, key, with_tensions=tensions))
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
    # Knowledge gaps only — pipeline audit-trail lines (OCR fallbacks, CUDA tracebacks)
    # leaked into the client-facing surface (round-4/5 audits); they live in `locus audit`.
    from locus.eval.metrics import semantic_gaps

    gaps = semantic_gaps(json.loads(doc["gap_flags"] or "[]"))
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


def _build_stamp() -> str:
    """A one-line build identifier (git commit + date + dirty flag) for the startup log.

    The server runs the code as of its process START — a long-lived stdio-over-SSH server
    keeps serving old behaviour after a fix lands until it is restarted. The 2026-06-09
    desktop eval misdiagnosed a fixed feature as broken because it had connected to a server
    that predated the fix (CLAUDE.md §2 / round-7). Logging this at startup makes the running
    version identifiable at connect time: compare it to `git rev-parse --short HEAD`. Best
    effort — degrades to 'unknown' when git is unavailable (e.g. a tarball deploy)."""
    import subprocess

    from locus.config import PROJECT_ROOT

    def _git(*args: str) -> str:
        return subprocess.run(
            ["git", "-C", str(PROJECT_ROOT), *args],
            capture_output=True, text=True, timeout=5,
        ).stdout.strip()

    try:
        commit = _git("rev-parse", "--short", "HEAD") or "unknown"
        when = _git("show", "-s", "--format=%cs", "HEAD")  # authored date, YYYY-MM-DD
        dirty = "+dirty" if _git("status", "--porcelain") else ""
        return f"{commit}{dirty} ({when})" if when else f"{commit}{dirty}"
    except Exception:
        return "unknown"


def run(enable_query: bool = False) -> None:
    """Run the MCP server over stdio (the transport used for stdio-over-SSH).

    `enable_query` opts into the billable server-side `query` tool (default off).
    """
    import os
    import sys

    # stderr ONLY — stdout carries the JSON-RPC protocol on the stdio transport. This is the
    # version-at-connect-time stamp; a stale server is otherwise invisible (see _build_stamp).
    print(
        f"locus mcp starting — build {_build_stamp()} | pid {os.getpid()}"
        + (" | query ENABLED (billable)" if enable_query else ""),
        file=sys.stderr,
        flush=True,
    )
    _build(enable_query=enable_query).run()


def _pages_to_image(doc, cap: int) -> list[int]:
    """Which 0-based pages to attach, best-first, at most `cap`.

    Pages carrying a BLANK mark come first — ink that covered no text is almost always over a
    figure, so the image is the only record of what it marks and the text register has already
    failed on it (measured: 29 of 109 highlights on his live documents). After those, pages with
    the most marks, since a densely worked page carries more of what he was thinking. Ties break
    on page order so the result is stable across calls.
    """
    per_page: dict[int, list] = {}
    for m in doc.marks:
        per_page.setdefault(m.page_index, []).append(m)
    ranked = sorted(
        per_page.items(),
        key=lambda kv: (-sum(m.is_blank for m in kv[1]), -len(kv[1]), kv[0]),
    )
    return sorted(idx for idx, _ in ranked[:cap])
