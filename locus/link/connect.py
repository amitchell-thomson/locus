"""The written reason two documents are worth connecting — and what to DO about it.

THE PROBLEM, in the owner's words. The Think page used to print a connection as a pair:

    keep  for regime projects overarching philosophy is that markets are not... ->
    Identification Verification for Structural Vector Autoregressions with Sparse
    Heterogeneous Markov Switching Heteroskedasticity
    both develop regime — you wrote yours on 2026-08-02

He called it "obscure and hard to read and understand", and he is right: it is two long titles
glued together and a bare noun. It states that an overlap exists and asks nothing. What he asked
for instead is "this paper 'x' discusses regime detection this way, and your note talks about it
that way. Could you use the paper's methods to improve the project's regime detection
performance? ... something directly useful to me."

That is not derivable from a join. `related_documents` knows the two documents share a canonical
concept; it cannot know HOW each treats it. So this is a model pass over two stored texts —
exactly the trade `discover/why.py` already makes, and for the same reason: the RANKING stays
deterministic (a shared canonical is a checkable fact), and the model only says in English what
two texts it was shown have to do with each other.

WHEN IT RUNS. Overnight, never at page-composition time, so `compose_daily` stays aggregate-only
and the page renders whether or not this succeeded (§18). The prose is stored in
`connection_notes` and the page just prints it; with no note, the connection is simply not
offered rather than falling back to the phrasing he rejected.

GROUNDED OR SILENT. The prompt sees only stored text: each document's synthesis and the section
summaries that carry the shared concept. A reason that does not name the shared concept is
DROPPED rather than stored — the same rule `surface/critique` applies to a claim citing evidence
it was not given. Failure degrades to no connection, never to a bad one.
"""

from __future__ import annotations

import logging
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timezone

log = logging.getLogger(__name__)

# Chars of each side's stored text handed to the model. Enough to characterise how a document
# treats a concept; not so much that the prompt becomes the document.
_MAX_SIDE_CHARS = 1400

_SYSTEM = (
    "You connect a person's own work to something they have read. You are given stored text from "
    "two documents and the one concept they both develop. You never invent facts about either."
)

_TEMPLATE = """His own material:
{his_title}
{his_text}

What he read:
{other_title}
{other_text}

Both develop: {shared}

Write ONE short prompt (2-3 sentences, under 300 characters) that tells him how the READ material
treats "{shared}" differently from his own, and asks a concrete question about whether he could
use it in his work. Name "{shared}" explicitly. Be specific about the method or idea, not about
the fact that both mention it. Do not open with "This paper" boilerplate or restate the titles.
Reply with the prompt only."""


@dataclass
class ConnectionNote:
    src_uri: str
    other_uri: str
    shared: str
    prose: str


def _doc_text(conn: sqlite3.Connection, uri: str, shared: str) -> tuple[str, str]:
    """(title, stored text) for one side — synthesis plus the summaries naming the concept."""
    row = conn.execute(
        "SELECT id, title, thesis, method, result FROM documents WHERE source_uri=?", (uri,)
    ).fetchone()
    if row is None:
        return "", ""
    parts = [p for p in (row["thesis"], row["method"], row["result"]) if p]
    like = f"%{shared}%"
    for sec in conn.execute(
        "SELECT summary FROM sections WHERE doc_id=? AND summary LIKE ? LIMIT 3",
        (row["id"], like),
    ):
        if sec["summary"]:
            parts.append(sec["summary"])
    return (row["title"] or uri), " ".join(parts)[:_MAX_SIDE_CHARS]


def write_note(
    conn: sqlite3.Connection,
    *,
    src_uri: str,
    other_uri: str,
    shared: str,
    runner=None,
    model: str | None = None,
) -> ConnectionNote | None:
    """Compose and store one connection's prose. Returns None when it cannot be grounded."""
    from locus.agent.claude import ClaudeError, run_text
    from locus.config import load

    his_title, his_text = _doc_text(conn, src_uri, shared)
    other_title, other_text = _doc_text(conn, other_uri, shared)
    if not (his_text and other_text):
        return None

    prompt = _TEMPLATE.format(
        his_title=his_title, his_text=his_text, other_title=other_title,
        other_text=other_text, shared=shared,
    )
    try:
        text = run_text(
            f"{_SYSTEM}\n\n{prompt}", model=model or load().agent.model, runner=runner
        )
    except ClaudeError as exc:                       # degrade, never block (ingest §7)
        log.warning("connect: model call failed for %s <-> %s: %s", src_uri, other_uri, exc)
        return None

    prose = " ".join((text or "").split())
    # GROUNDED OR SILENT: a prompt that cannot even name the concept it was asked about is not
    # about the connection, so it is dropped rather than shown.
    if not prose or shared.lower() not in prose.lower():
        log.info("connect: dropped ungrounded prose for %r", shared)
        return None
    prose = prose[:400]

    with conn:
        conn.execute(
            "INSERT INTO connection_notes (src_uri, other_uri, shared, prose, written_at) "
            "VALUES (?,?,?,?,?) ON CONFLICT(src_uri, other_uri, shared) DO UPDATE SET "
            "prose=excluded.prose, written_at=excluded.written_at",
            (src_uri, other_uri, shared, prose, datetime.now(timezone.utc).isoformat()),
        )
    return ConnectionNote(src_uri, other_uri, shared, prose)


def stored_note(
    conn: sqlite3.Connection, *, src_uri: str, other_uri: str, shared: str
) -> str:
    """The stored prose for one connection, or '' — what `compose_daily` reads."""
    try:
        row = conn.execute(
            "SELECT prose FROM connection_notes WHERE src_uri=? AND other_uri=? AND shared=?",
            (src_uri, other_uri, shared),
        ).fetchone()
    except sqlite3.OperationalError:
        return ""
    return (row["prose"] if row else "") or ""
