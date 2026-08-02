"""What a book he chose himself has to do with his projects.

THE ASYMMETRY THIS CLOSES. A paper the discovery loop proposed arrives with `evidence_key`
("project:regime-ml") and a written reason, so the daily page can say what it bears on. A book he
bought, or one a colleague recommended, arrives as a filename in `/Reading/In-Progress` and
nothing else. That is backwards. Material he sought out himself is what he is most committed to,
and it is where a cross-domain link is most interesting precisely because no ranker chose it to be
relevant — nobody has already decided it fits.

WHAT IT IS SCORED ON. Usually the book is not in the corpus and has no abstract, so there is no
publisher's summary to embed. What there IS, if he has been reading it, is `pdf_annotations`: the
passages he underlined and the notes he wrote beside them. Those are a better signal than an
abstract would have been — an abstract says what the author thinks the book is about, whereas the
marks say which parts made HIM stop. *Advanced Portfolio Management* has 26 marks and ~4,300
characters, which is what its first link is computed from.

A book with no marks yet falls back to its title alone. That is thin, and the fit it produces is
honestly thin — which is why `fit` is stored and rendered rather than hidden: a weak link should
look weak. Nothing is invented to fill the gap.

Everything here is an embedding and a cosine against the profiles `discover/profiles.py` already
builds — the same `(subject_kind, label)` space a proposed paper resolves into, so a book and a
paper link to his work in exactly the same way and the page needs no second code path.
"""

from __future__ import annotations

import logging
import sqlite3
from dataclasses import dataclass
from pathlib import Path

from locus.discover.rank import cos_from_l2

log = logging.getLogger(__name__)

# Profiles are per-facet, so one project contributes many vectors; this is how many nearest
# profile facets to consider per item. Small: we want the best-matching facet, not a survey.
_TOP_PROFILES = 8

# Marked passages offered to the embedder. Enough to characterise what he cared about without
# turning one heavily-annotated book into a wall of text whose embedding means nothing in
# particular.
_MAX_MARK_CHARS = 4000


@dataclass
class Linked:
    target_id: int
    title: str
    subject_kind: str = ""
    subject_label: str = ""
    fit: float = 0.0
    detail: str = ""

    @property
    def ok(self) -> bool:
        return bool(self.subject_label)


def title_for(device_path: str, source_uri: str | None) -> str:
    """A human name for a reading target.

    `device_path` is preferred but goes stale — the 2026-08-02 reorganisation moved every document
    and nothing rewrote the stored paths — so a `source_uri` filename is the fallback.
    """
    for candidate in (device_path or "", source_uri or ""):
        name = Path(candidate.rstrip("/")).name
        if name:
            return name[:-4] if name.lower().endswith(".pdf") else name
    return "(untitled)"


def evidence_text(conn: sqlite3.Connection, target: sqlite3.Row) -> tuple[str, str]:
    """(text to score on, what that text is) for one reading target.

    Prefers HIS marks over anything else available, for the reason in the module docstring.
    """
    title = target["title"] or title_for(target["device_path"], target["source_uri"])
    try:
        rows = conn.execute(
            "SELECT covered_text, note FROM pdf_annotations WHERE source_uri=? "
            "ORDER BY id LIMIT 200",
            (target["source_uri"],),
        ).fetchall()
    except sqlite3.OperationalError:
        rows = []

    marks: list[str] = []
    for row in rows:
        for part in (row["covered_text"], row["note"]):
            cleaned = " ".join((part or "").split())
            if cleaned:
                marks.append(cleaned)
    if marks:
        body = " ".join(marks)[:_MAX_MARK_CHARS]
        return f"{title}. {body}", f"{len(rows)} passage(s) you marked"

    # A proposal's abstract, when this target came from one.
    if target["proposal_id"]:
        row = conn.execute(
            "SELECT abstract FROM reading_proposals WHERE id=?", (target["proposal_id"],)
        ).fetchone()
        abstract = " ".join((row["abstract"] or "").split()) if row else ""
        if abstract:
            return f"{title}. {abstract}", "its abstract"

    return title, "its title alone"


def best_subject(
    conn: sqlite3.Connection, text: str, *, embed_fn=None
) -> tuple[str, str, float]:
    """(subject_kind, label, fit) — the profile facet this text sits closest to.

    Returns ('', '', 0.0) when there are no profiles to compare against, which is the honest
    answer before `locus discover --profiles` has ever run.
    """
    from locus.ingest.embed import embed_text

    embed_fn = embed_fn or embed_text
    try:
        vector = embed_fn(text)
    except Exception as exc:                       # ollama down, model missing
        log.warning("relevance: embedding failed: %s", exc)
        return "", "", 0.0

    import struct

    blob = struct.pack(f"{len(vector)}f", *vector)
    # THE KNN MUST STAND ALONE. A vec0 MATCH needs its LIMIT visible on the same query, and a
    # JOIN hides it — "A LIMIT or 'k = ?' constraint is required on vec0 knn queries", which
    # surfaced only as every single item reporting "no profile to compare against". `rank.py`
    # never joins for exactly this reason; the subject lookup is a second, ordinary query.
    try:
        hits = conn.execute(
            "SELECT profile_id, distance FROM discovery_profile_vectors "
            "WHERE embedding MATCH ? ORDER BY distance LIMIT ?",
            (blob, _TOP_PROFILES),
        ).fetchall()
    except sqlite3.OperationalError:
        return "", "", 0.0
    if not hits:
        return "", "", 0.0

    best = min(hits, key=lambda r: r["distance"])
    row = conn.execute(
        "SELECT subject_kind, label FROM discovery_profiles WHERE id=?", (best["profile_id"],)
    ).fetchone()
    if row is None:
        return "", "", 0.0
    return row["subject_kind"], row["label"], cos_from_l2(best["distance"])


def targets_needing_a_link(conn: sqlite3.Connection, *, only_owner: bool = False) -> list[int]:
    """Reading targets with no project link yet, most recently touched first.

    `only_owner` restricts to material he added himself (`proposal_id IS NULL`) — the case that
    had no relevance at all. By default a delivered paper is included too, so its link is stored
    on the same row the page reads rather than only on its proposal.
    """
    sql = (
        "SELECT id FROM reading_targets WHERE (subject_label IS NULL OR TRIM(subject_label)='') "
    )
    if only_owner:
        sql += "AND proposal_id IS NULL "
    sql += "ORDER BY COALESCE(last_swept, created_at) DESC"
    return [r["id"] for r in conn.execute(sql)]


def link_target(conn: sqlite3.Connection, target_id: int, *, embed_fn=None) -> Linked:
    """Score one reading target against his projects and store the link. Never raises."""
    target = conn.execute(
        "SELECT * FROM reading_targets WHERE id=?", (target_id,)
    ).fetchone()
    if target is None:
        return Linked(target_id, "", detail="no such reading target")

    title = target["title"] or title_for(target["device_path"], target["source_uri"])
    text, basis = evidence_text(conn, target)
    kind, label, fit = best_subject(conn, text, embed_fn=embed_fn)
    if not label:
        return Linked(target_id, title, detail="no profile to compare against")

    with conn:
        conn.execute(
            "UPDATE reading_targets SET title=?, subject_kind=?, subject_label=?, fit=? "
            "WHERE id=?",
            (title, kind, label, fit, target_id),
        )
    return Linked(target_id, title, kind, label, fit, detail=f"scored on {basis}")


def link_all(
    conn: sqlite3.Connection, *, only_owner: bool = False, limit: int = 50, embed_fn=None
) -> list[Linked]:
    """Link every unlinked reading target. Free and local — embeddings only, no API."""
    return [
        link_target(conn, tid, embed_fn=embed_fn)
        for tid in targets_needing_a_link(conn, only_owner=only_owner)[:limit]
    ]


def in_progress(conn: sqlite3.Connection) -> list[sqlite3.Row]:
    """What he is actually reading right now — owner-added books included.

    THE SOURCE OF TRUTH FOR "in progress" IS THIS TABLE, not `reading_proposals`. The first cut of
    the Read page read proposals, so the one document he was genuinely reading and annotating —
    a book he added himself, with 26 marks on it — was the one thing the section left out.
    """
    try:
        return list(
            conn.execute(
                "SELECT * FROM reading_targets WHERE device_folder='In-Progress' "
                "ORDER BY COALESCE(last_swept, created_at) DESC"
            )
        )
    except sqlite3.OperationalError:
        return []
