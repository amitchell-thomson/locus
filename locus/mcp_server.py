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
- to_remarkable  : push a document to the tablet. FREE. Anything CLAUDE writes goes as LaTeX
                   (`latex=`, compiled by reading/tex2pdf) — his standing preference, for the
                   equations, layout and figures markdown cannot express. `markdown=` relays
                   text that was already markdown; `pdf_path=` pushes a finished PDF.
- markups        : ONE call — find a marked-up document anywhere on the device, sweep it,
                   render the inked pages with margins intact, and return them as images
                   with the text register. `images=False` for the cheap text-only read. FREE.
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
import time
import traceback
from datetime import date
from pathlib import Path

from locus.config import load
from locus.db.connection import get_connection
from locus.observe import mcp_log
from locus.query import QUERY_MODES
from locus.query import answer as run_answer
from locus.retrieve import Facets
from locus.retrieve import retrieve as run_retrieval

# Bound the size of read-only listings/inspections returned to the client.
_MAX_INSPECT_PROPS = 40
_MAX_INSPECT_ENTITIES = 40

# The build this PROCESS started with, and its pid. Set by `run()`; a server built directly
# (tests) reports "unstarted". Both travel into the call log and into the message a failing
# tool returns, because "which server was I even talking to" is the first question every time.
_SERVER_BUILD = "unstarted"
_SERVER_PID = 0

# `_stale_note` re-reads the checkout's HEAD at most this often. A tool call is not a hot loop,
# but neither should every call pay a subprocess.
_STALE_POLL_S = 30.0
_stale_cache: tuple[float, str] = (0.0, "")


def _commit_of(stamp: str) -> str:
    """The commit id out of a build stamp, dropping the `+dirty` flag and the date."""
    return stamp.split()[0].split("+")[0] if stamp else "unknown"


def _stale_note() -> str:
    """A one-line warning when this process is older than the checkout it serves, else "".

    A long-lived stdio server keeps running the code it started with, so a fix that has landed
    is not a fix that is running (CLAUDE.md §13, "restart `locus mcp` after any retrieval
    change"). That rule has been missed repeatedly, and it is missed silently: the startup
    stamp goes to stderr, which the model on the other side never sees. On 2026-09-08 a session
    holding a pre-fix server reported a bug that had already been fixed, twice.

    So the server says it in the one place he does see — the tool result. Best effort: if git
    cannot be read, say nothing rather than crying wolf.
    """
    global _stale_cache
    if _SERVER_BUILD in ("unstarted", "unknown"):
        return ""
    cached_at, note = _stale_cache
    if time.monotonic() - cached_at < _STALE_POLL_S:
        return note
    current = _build_stamp()
    note = ""
    # Compare COMMITS, not the full stamp: `_build_stamp` carries a `+dirty` flag, and during
    # any editing session the working tree is dirty by definition. Firing on that would put the
    # banner on every result all day and train him to skip past it — the alarm this exists to
    # be worth reading. What it is for is commit-level: a fix landed and this process predates
    # it.
    if _commit_of(current) not in ("unknown", _commit_of(_SERVER_BUILD)):
        note = (
            f"[locus mcp is STALE — this server process started at build {_SERVER_BUILD} and "
            f"the checkout is now at {current}. It is still running the OLD code. Restart the "
            "MCP server (a new session, or reconnect it) before trusting this result or "
            "reporting a bug against it.]"
        )
    _stale_cache = (time.monotonic(), note)
    return note


def _instrument(fn):
    """Wrap one tool so no call is invisible and no failure reaches the client empty.

    Two guarantees, both learned from the same day (2026-09-08):

    1. ARRIVAL IS RECORDED BEFORE THE WORK STARTS. The failure that cost the day was a call the
       client gave up on while the server was still working. An end-of-call log would have
       recorded nothing at all for it, which is precisely the case that needs evidence. A
       `start` with no `end` in `vault/logs/mcp.jsonl` names that case exactly.
    2. EVERY EXCEPTION BECOMES TEXT. `to_remarkable` grew this guard alone in 1dae4ee after
       `subprocess.TimeoutExpired` slipped past its enumerated handlers; but the hole was never
       specific to that tool. Any tool that raises across the MCP boundary arrives as a bare
       transport error, and the model on the far side then cannot tell a bug in one tool from a
       dead server — so it misreports which machine is broken. Returning the exception CLASS and
       message as the tool's own result keeps the diagnosis with the thing that failed.

    Signature and docstring are preserved (`functools.wraps` + `__wrapped__`), so the schema
    FastMCP advertises is byte-identical to the unwrapped function's.
    """
    import functools

    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        global _call_seq
        _call_seq += 1
        call = _call_seq
        digest = mcp_log.arg_digest(kwargs)
        mcp_log.record("start", call=call, tool=fn.__name__, build=_SERVER_BUILD, args=digest)
        started = mcp_log.now()
        try:
            result = fn(*args, **kwargs)
        except Exception as exc:  # noqa: BLE001 — deliberate boundary guard, see docstring
            mcp_log.record(
                "end", call=call, tool=fn.__name__, ok=False,
                ms=round((mcp_log.now() - started) * 1000),
                error=type(exc).__name__, message=str(exc),
                traceback=traceback.format_exc(),
            )
            return (
                f"{fn.__name__} raised {type(exc).__name__}: {exc}\n\n"
                "The call REACHED the server and this tool is what failed, so the server is "
                "not down. If the message above does not say what to fix, it is a Locus bug "
                f"rather than a problem with the request. Server build {_SERVER_BUILD}, pid "
                f"{_SERVER_PID}; full traceback in {mcp_log.log_path()} "
                "(`locus mcp-log --errors --traceback`)."
            )
        mcp_log.record(
            "end", call=call, tool=fn.__name__, ok=True,
            ms=round((mcp_log.now() - started) * 1000),
        )
        note = _stale_note()
        if note and isinstance(result, str):
            return f"{note}\n\n{result}"
        return result

    return wrapper


_call_seq = 0


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
            "`to_remarkable` to send him a document to read on paper — anything YOU write for "
            "the tablet is authored in LaTeX and passed as `latex=`, which is his standing "
            "preference and the system's interface with him, not a per-document choice; "
            "`markdown=` is for relaying text that was already markdown and `pdf_path=` for a "
            "finished PDF — and "
            "`markups` to READ a document he marked up — one call finds it anywhere on the "
            "device, sweeps it if needed, and returns the inked pages as images with his "
            "margin writing intact, plus what each mark covered. "
            "For the owner's own thinking rather than raw material: `critique` stress-tests a "
            "project or argument against what he has read and concluded, `synthesise` gives what "
            "he knows and thinks about a topic including how his view has changed, `objects` "
            "lists his projects/concepts/questions, and `evolution` shows the dated trajectory "
            "of his positions. Prefer `retrieve` and ground every claim in the returned material "
            "with its citation."
        ),
    )

    def tool():
        """Register a tool through `_instrument` — the logging and error-to-text guard.

        Every tool goes through here, with no opt-out: the guarantee is only worth anything if
        it holds for the tool that turns out to be broken, and which one that is cannot be
        known in advance.
        """

        def deco(fn):
            return mcp.tool()(_instrument(fn))

        return deco

    @tool()
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
        # opt-in: only now is the billable tool advertised to clients
        mcp.add_tool(_instrument(query))

    @tool()
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

    @tool()
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

    @tool()
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

    @tool()
    def to_remarkable(
        latex: str | None = None,
        title: str | None = None,
        markdown: str | None = None,
        pdf_path: str | None = None,
        folder: str | None = None,
        resource_dir: str | None = None,
    ) -> str:
        r"""Send a document to the owner's reMarkable to read on paper. Three modes.

        LATEX (`latex=`) — **THE DEFAULT FOR ANYTHING YOU WRITE.** When he says "write this up
        and send it to my reMarkable", "push that to the tablet", or asks for a note, summary,
        derivation, plan or answer on paper: AUTHOR IT IN LATEX AND PASS IT HERE. He asked for
        this specifically. It is not a stylistic preference to weigh against convenience — it is
        how this system is meant to talk to him, because LaTeX gives real equations (`align`,
        `cases`, numbered and referenced), real layout control, and real figure placement.
        Writing markdown and sending it through `markdown=` instead is the wrong call even when
        the document has no maths in it.

        Pass a FRAGMENT — open with body text or `\section{...}`, and pass the document's name as
        `title` rather than writing your own heading first. It is wrapped in a preamble already
        tuned to the device's page size, margins and reading leading, with amsmath, graphicx,
        booktabs, enumitem, hyperref, microtype, TikZ and pgfplots loaded. Do NOT write your own
        `\documentclass` unless you specifically need to override the layout: if you do include
        one, the document is compiled EXACTLY as you wrote it and the device geometry is yours to
        get right.

        LAYOUT: the page is set in TWO COLUMNS at 9pt. Each column is about 2.9in, so a display
        equation wider than that overflows into the gutter — break wide maths with `split` or
        `aligned`, and use the starred floats `figure*` and `table*` for anything that genuinely
        needs the full width. `\linewidth` inside a float is the column, which is what you want
        for `\includegraphics` and for pgfplots `width=`.

        DRAW THINGS. This is the main reason he reads on the tablet: he is trying to UNDERSTAND
        something, and a diagram beside the prose is most of what makes that work on paper. TikZ
        and pgfplots are already loaded, so draw the mechanism rather than describing it — a
        block diagram of the pipeline, the geometry of the thing being derived, a plot of the
        function under discussion, an annotated axis showing what a parameter does. Prefer a
        drawn figure to a paragraph explaining what the figure would show. Two constraints: the
        screen is GREYSCALE, so never encode meaning in colour (the default pgfplots cycle list
        separates series by dash pattern for exactly this reason), and keep line weights at the
        preamble's defaults or heavier, because hairlines disappear on e-ink.

        FIGURES FROM HIS CORPUS are an option, not a first resort (he confirmed 2026-09-08 that
        drawn figures are what he wants; do not open with a figure hunt, and do not treat a
        schematic as a compromise). When a real figure from a paper he read genuinely fits,
        `\includegraphics[width=\linewidth]{<name>}` resolves against the raw store by default,
        so an ingested figure drops in by the filename in `figures.raw_path`. Use `resource_dir`
        to point at some other directory instead, e.g. one holding plots you just generated with
        matplotlib. A name that is not there comes back as an error naming the file.

        SAY WHEN A FIGURE IS ILLUSTRATIVE. A schematic with invented numbers is fine and often
        the clearest thing to draw, but a plot that looks like a result and is not must say so in
        its caption. That distinction is what makes a drawn figure safe to read.

        STYLE. Write it as a document, not as chat that has been typeset.
          - NO EM DASHES. This is enforced: `---` or an em dash character anywhere in the body or
            title is refused with an error, and you should rewrite the sentence rather than
            reach for a substitute punctuation mark that keeps the same shape. Use a colon, a
            semicolon, parentheses, or two sentences. `--` for a numeric range is fine.
          - No assistant register. Cut "Let's dive in", "It's worth noting that", "In this
            section we will", "I hope this helps", and any closing paragraph that summarises the
            section it just ended. Do not open a section by announcing what the section is about.
            State the thing itself.
          - No hedging filler, no rhetorical questions to the reader, no bulleted restatement of
            a paragraph that already said it. Prose carries the argument; lists carry lists.
          - Define a term at first use and then use it. He is reading to learn, so an undefined
            piece of jargon is a dead end, and a defined one restated three times is padding.

        MARKDOWN (`markdown=`): for relaying text that ALREADY EXISTS as markdown — a file you
        read, a stored note, a pass's output. Use it to send his own words unchanged. Do not use
        it for prose you are composing; that is what `latex=` is for.

        EXISTING PDF (`pdf_path=`): a PDF that already exists — one you just generated, or one in
        the repo or vault. Pushed unchanged, not re-rendered. The path resolves ON THE LOCUS
        SERVER: absolute, or relative to the server's working directory, or to the checkout root
        ('docs/plan.pdf'). It therefore works only when you are running on that machine. If you
        are not, the error will say so — do not retry with a different path, send `latex`.

        Pass exactly one of `latex`, `markdown` or `pdf_path`.

        This is DELIVERY ONLY — it does not ingest, capture, or change the corpus (use `capture`
        for that). The document lands in its own device folder, deliberately not in the daily
        page's inbox and not in the reading folders whose contents are auto-ingested.

        A LaTeX compile error comes back as the engine's own error lines, naming the line that
        broke. That is a fixable failure: correct the source and call again.

        Args:
            latex: LaTeX source — a fragment (preferred) or a full `\documentclass` document.
                The default mode for a document you are writing for him.
            title: Short title — names the file on the device. Required with `latex`/`markdown`
                (it also heads page 1); optional with `pdf_path`, where it defaults to the
                filename.
            markdown: Existing markdown to relay unchanged. Not for prose you are composing.
            pdf_path: Path on the Locus server to an existing PDF to push unchanged.
            folder: Optional device folder override (default `[reading].send_folder`).
            resource_dir: Directory on the Locus server that relative `\includegraphics` names
                resolve against. Defaults to the corpus raw store, so ingested figures work by
                their stored filename; set it to point at generated plots instead.
        """
        from locus.reading.send import send_latex, send_markdown, send_pdf

        # All three modes reach the same device folder by the same push, so the ONLY thing that
        # can go wrong here is the caller meaning one and getting another. Refuse the ambiguous
        # calls in words rather than picking a winner silently.
        given = [name for name, value in
                 (("latex", latex), ("markdown", markdown), ("pdf_path", pdf_path)) if value]
        if len(given) > 1:
            return (f"Pass exactly one of `latex`, `markdown` or `pdf_path` — got {given}. They "
                    "are three ways to send one document and I cannot tell which you meant.")
        if not given:
            return ("Nothing to send: pass `latex` (a document you wrote — the default), "
                    "`markdown` (text that was already markdown), or `pdf_path` (a PDF on the "
                    "Locus server).")

        try:
            if pdf_path:
                sent = send_pdf(pdf_path, title=title, folder=folder)
                verb, what = "Pushed", "unchanged"
            elif latex:
                if not title:
                    return "Sending LaTeX needs a `title` — it names the file and heads page 1."
                sent = send_latex(latex, title=title, folder=folder,
                                  resource_dir=resource_dir)
                verb, what = "Sent", "typeset"
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
        except RuntimeError as exc:
            # A LaTeX compile failure, carrying the engine's error lines, or a missing engine.
            # Also the caller's to fix, and specifically ACTIONABLE — it names the bad line — so
            # it must reach the model as text rather than as a transport-level exception.
            return f"Not sent — {exc}"
        except Exception as exc:  # noqa: BLE001 — deliberate boundary guard, see below
            # The three handlers above enumerate the failures this tool EXPECTED, and anything
            # they missed crossed the MCP boundary as a bare transport error carrying no message.
            # That is the §3 failure class wearing a different hat: the model on the other side
            # cannot distinguish "your LaTeX timed out" from "the server is gone", so it
            # misreports the cause to him and he debugs the wrong machine. It happened for real
            # (2026-09-08) with `subprocess.TimeoutExpired`, which is a `SubprocessError` and so
            # matched no clause here; that specific case is now a RuntimeError at its source, but
            # the hole was the enumeration, not the one exception that found it. Name the type,
            # because an unexpected error's CLASS is most of its diagnosis.
            return (f"Not sent — unexpected {type(exc).__name__}: {exc}. This is a Locus bug "
                    "rather than something wrong with the document.")

        pages = f"{sent.pages} page{'s' if sent.pages != 1 else ''}" if sent.pages else what
        return f"{verb} '{sent.filename}' ({pages}) to reMarkable:{sent.device_path}."

    @tool()
    def markups(
        document: str,
        pages: list[int] | None = None,
        intent: str | None = None,
        refresh: bool = False,
        images: bool = True,
        margins: bool = True,
        max_images: int = 12,
        dpi: int = 130,
        out_dir: str | None = None,
    ):
        """Read a document he marked up on the reMarkable — the inked pages AND the text register.

        ONE call does the whole thing: finds the document anywhere on the device (not just the
        reading folders), sweeps it for marks if nothing has yet, renders the inked pages with
        his handwriting composited on, and returns them as images with the text register first.

        The pages are rendered on a canvas ENLARGED to hold margin ink. His marginalia routinely
        runs off the edge of the paper, and the older renderer clipped it at the page rect — on
        one draft, 12 of 27 marks were margin notes whose words were mostly outside the page and
        came back as a few stray letters. A faint grey rectangle marks where the paper ended, so
        writing beyond it reads as margin writing rather than as text in the wrong place.

        Set `images=False` for a cheap text-only read: no device fetch, no rendering, just what
        each mark covered. That is the right call when you only need to know WHAT he marked.

        Sweeping is free and geometric — it never calls the billed handwriting transcription, so
        a freshly swept document has the covered text and the line for every mark but no
        transcribed note until Loop B or `locus annotate --transcribe` runs.

        Args:
            document: Title fragment, source_uri, device path, or xochitl uuid. Ambiguous
                fragments come back as a list of candidates rather than a guess.
            pages: Optional 1-based page numbers. Default: every inked page, densest first,
                up to `max_images`. Named pages are always returned, never trimmed, and they
                narrow the text register too.
            intent: Optional filter — 'important', 'not_understood', or 'idea'. Narrows the
                text register AND picks the pages to render, so "what did I not understand"
                returns those pages rather than the most heavily inked ones.
            refresh: Re-fetch from the device and re-sweep. Use when he has just written more.
            images: Set False for the text register alone (no device fetch, much faster).
            margins: Keep the enlarged canvas. False reproduces the old page-clipped render.
            max_images: Cap on pages returned when `pages` is not given.
            dpi: Render resolution. 130 reads his handwriting; lower it for a lighter reply.
            out_dir: Also write pNNNN.png files into this server-side directory.
        """
        from locus.capture import review

        cfg = load()
        conn = get_connection(cfg.paths.db)
        try:
            candidates = review.resolve_target(conn, document, rmapi_binary=cfg.capture.rmapi_binary)
            if not candidates:
                return (f"Nothing on the device or in the vault matches {document!r}. "
                        "Try a shorter fragment, or the exact device path.")
            if len(candidates) > 1:
                listing = "\n".join(f"  {c.title}  [{c.device_path}]" for c in candidates)
                return f"{document!r} matches several documents — say which:\n{listing}"

            try:
                m = review.markups(
                    conn, candidates[0], cfg=cfg, pages=pages, intent=intent, refresh=refresh,
                    images=images, margins=margins, max_images=max_images, dpi=dpi,
                )
            except Exception as exc:
                return f"Could not read {document!r} from the device: {exc}"

            if not m.marks.marks:
                # Three different findings that used to print as one. A filter that matched
                # nothing is not an empty document, and neither is a text-only call that never
                # looked at the device — reporting them alike is how a working sweep gets
                # mistaken for a broken one.
                if m.filtered:
                    return (f'No marks in "{m.target.title}" match that filter. '
                            "Drop `intent`/`pages` to see everything on it.")
                if not m.looked:
                    return (f'No marks stored for "{m.target.title}" yet. '
                            "Call again with `refresh=True` to sweep it from the device.")
                if not m.inked_pages:
                    return (f'"{m.target.title}" is on the device at {m.target.device_path} but '
                            "carries no ink — nothing has been marked on it yet.")

            head = m.marks.render(image_hint=not images)
            if m.swept:
                head = (f"[swept {m.swept} mark(s) from the device on this call — geometric only, "
                        f"so handwriting is not transcribed yet]\n\n{head}")
            out: list = [head]

            written: list[str] = []
            if out_dir:
                target_dir = Path(out_dir)
                target_dir.mkdir(parents=True, exist_ok=True)
                for idx, png in sorted(m.pages.items()):
                    dest = target_dir / f"p{idx + 1:04d}.png"
                    dest.write_bytes(png)
                    written.append(str(dest))

            if m.pages:
                from mcp.server.fastmcp import Image

                for idx, png in sorted(m.pages.items()):
                    out.append(f'[p.{idx + 1} of "{m.target.title}" — his ink, margins included]')
                    out.append(Image(data=png, format="png"))

            tail = []
            if m.omitted:
                tail.append(
                    f"{len(m.omitted)} inked page(s) not shown: "
                    f"{', '.join(str(p + 1) for p in m.omitted)}. "
                    "Ask for them by number with `pages=[...]`."
                )
            if written:
                tail.append(f"Also written to: {', '.join(written)}")
            if tail:
                out.append(" ".join(tail))
            return out
        finally:
            conn.close()

    # --- Phase-2 value surfaces (agent-layer §8.4) -----------------------------------------
    # critique/synthesise ground in-process (free, local) and then make ONE `claude -p` call.
    # That call runs on the owner's SUBSCRIPTION, not the metered API key — which is why they
    # are advertised by default while `query` (metered) stays opt-in. The cost guard's shape is
    # unchanged: no tool here can spend against ANTHROPIC_API_KEY.

    @tool()
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

    @tool()
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

    @tool()
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

    @tool()
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

    global _SERVER_BUILD, _SERVER_PID
    _SERVER_BUILD = _build_stamp()
    _SERVER_PID = os.getpid()

    # stderr ONLY — stdout carries the JSON-RPC protocol on the stdio transport. This is the
    # version-at-connect-time stamp; a stale server is otherwise invisible (see _build_stamp).
    print(
        f"locus mcp starting — build {_SERVER_BUILD} | pid {_SERVER_PID}"
        + (" | query ENABLED (billable)" if enable_query else ""),
        file=sys.stderr,
        flush=True,
    )
    # The same fact, written where it OUTLIVES the process. stderr goes to the client and is
    # gone when the client is; the question "which build was that session actually running"
    # is always asked afterwards, about a session that has already ended.
    mcp_log.record("server_start", build=_SERVER_BUILD, query_enabled=enable_query)
    # No matching "stop" event, deliberately. An `atexit` hook does not run on SIGKILL or on the
    # SIGTERM a client sends when it drops the server — which are exactly the exits worth
    # knowing about — so it would record the ordinary case and miss every interesting one.
    # `live_servers` decides liveness by signalling the pid instead, which needs no cooperation
    # from a process that has already died.
    _build(enable_query=enable_query).run()
