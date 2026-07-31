"""Data access for `reading_proposals` / `reading_targets` (Phase 4, the accept loop).

Every write rule for a proposed reading lives here, so the two callers that will exist by the end
of Phase 4 — the discovery engine and the citation channels — cannot each invent their own:

  1. **Propose, never mutate (invariant 2).** A proposal is not a corpus document. `add_candidate`
     writes a row with `status='candidate'` and nothing else; only the owner's physical folder
     move on the device promotes it, and only `accept` writes anything the corpus will see.
  2. **Grounded or silent (invariant 3).** `why` and `evidence_key` are NOT NULL in the schema and
     re-checked here, because an ungrounded suggestion is what teaches him to stop opening the
     folder. A candidate that cannot say why it exists is refused, not softened.
  3. **Caps are on the STOCK, not the flow.** `slots_free` counts what is sitting in `Proposed`
     right now. A full folder proposes nothing, so falling behind for a fortnight produces silence
     rather than a backlog — which is what makes "empty is a valid state" structural rather than
     aspirational.
  4. **Identity is `dedupe_key`.** See `dedupe_key` for why it is a normalised title rather than
     an external id, and what the normaliser has to survive.

Nothing in this module calls a model or the network; the whole accept loop is joins and counts.
"""

from __future__ import annotations

import sqlite3
import unicodedata
from dataclasses import dataclass
from datetime import datetime, timezone

# The device folders. `Proposed` is a holding pen with no obligation attached: leaving something
# there is the rejection signal, and moving it anywhere else is the acceptance signal.
FOLDER_PROPOSED = "Proposed"
FOLDER_IN_PROGRESS = "In-Progress"
FOLDER_FINISHED = "Finished"
READING_FOLDERS = (FOLDER_PROPOSED, FOLDER_IN_PROGRESS, FOLDER_FINISHED)

# `acceptance_log.surface` for discovery judgments. Deliberately NOT 'reading', which
# `pull_daily.SURFACE_READING` already owns for the daily page's re-read slot — see migration 0017.
SURFACE_DISCOVERY = "discovery"

# Stock caps (plan §7). Papers cost one swipe; a book is a month of his time, so it gets exactly
# one considered suggestion at a time rather than a feed.
DEFAULT_CAPS = {"paper": 3, "book": 1}

STATUSES = ("candidate", "proposed", "accepted", "ingested", "rejected", "superseded")

# Standalone diacritic characters that OCR emits BEFORE the letter they belong to, turning
# "Lütkepohl" into "L¨utkepohl". Stripping them is what lets the two surfaces of one work collapse
# to a single key (see `dedupe_key`).
_LOOSE_DIACRITICS = "¨´ˇ˜ˆ`^°˙˚¯"


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


def dedupe_key(title: str, authors: str = "", year: int | None = None) -> str:
    """Stable identity for a work: normalised title, falling back to authors+year.

    IDENTITY IS NOT `external_id`. The same paper reaches us as an arXiv id from one citing
    document and a DOI from another, and a second proposal of something he has already seen is the
    suggestion fatigue that kills this layer (failure mode #7).

    The normaliser has to survive the corpus as it actually is. The entity pass stores OCR-mangled
    citations, and the SAME WORK is present twice today under two surfaces:

        'L¨utkepohl and Wo´zniak, 2020'   (loose diacritics, emitted before their letter)
        'Lütkepohl and Woźniak, 2020'     (real combining marks)

    Both must produce one key or the flywheel counts one work as two and proposes it twice. So:
    strip loose diacritics, NFKD-decompose and drop combining marks, casefold, and collapse
    everything that is not alphanumeric to a single space.

    CAVEAT, deliberately not solved here: an unresolved citation string ("Denev and Amin, 2020")
    and the resolved work it names ("Portfolio Management under Stress") key differently. Resolve
    before storing where possible; a later re-resolution is a `supersede` + re-add, not a key
    rewrite, because rewriting an identity silently merges two histories.
    """
    basis = title.strip() or f"{authors} {year or ''}"
    stripped = "".join(ch for ch in basis if ch not in _LOOSE_DIACRITICS)
    decomposed = unicodedata.normalize("NFKD", stripped)
    folded = "".join(ch for ch in decomposed if not unicodedata.combining(ch)).casefold()
    return " ".join("".join(ch if ch.isalnum() else " " for ch in folded).split())


@dataclass
class Proposal:
    id: int
    kind: str
    dedupe_key: str
    title: str
    why: str
    why_kind: str
    evidence_key: str
    status: str
    resolution: str | None = None
    external_id: str | None = None
    authors: str | None = None
    year: int | None = None
    url: str | None = None
    abstract: str | None = None
    oa_pdf_url: str | None = None
    score: float | None = None
    device_uuid: str | None = None
    device_folder: str | None = None
    filename: str | None = None
    local_path: str | None = None
    proposed_at: str | None = None
    resolved_at: str | None = None
    created_at: str = ""

    @property
    def is_stub(self) -> bool:
        """True when there is no real file behind this proposal — it is a description only.

        A stub is agent output and is NEVER ingested (invariant 5): the corpus gets the real book
        the owner supplies, or nothing at all.
        """
        return not self.local_path


def _row(r) -> Proposal:
    return Proposal(
        id=r["id"], kind=r["kind"], dedupe_key=r["dedupe_key"], title=r["title"], why=r["why"],
        why_kind=r["why_kind"], evidence_key=r["evidence_key"], status=r["status"],
        resolution=r["resolution"],
        external_id=r["external_id"], authors=r["authors"], year=r["year"], url=r["url"],
        abstract=r["abstract"], oa_pdf_url=r["oa_pdf_url"], score=r["score"],
        device_uuid=r["device_uuid"], device_folder=r["device_folder"], filename=r["filename"],
        local_path=r["local_path"], proposed_at=r["proposed_at"], resolved_at=r["resolved_at"],
        created_at=r["created_at"],
    )


# --- writes -----------------------------------------------------------------------------------


def add_candidate(
    conn: sqlite3.Connection,
    *,
    kind: str,
    title: str,
    why: str,
    why_kind: str,
    evidence_key: str,
    authors: str = "",
    year: int | None = None,
    external_id: str | None = None,
    url: str | None = None,
    abstract: str | None = None,
    oa_pdf_url: str | None = None,
    score: float | None = None,
    source_run: int | None = None,
) -> int | None:
    """Record a candidate. Returns its row id, or None if it was already known.

    Already-known covers every prior status including `rejected` and `superseded` — re-proposing
    something he has already turned down is the single fastest way to make him stop looking.
    """
    if not why.strip() or not evidence_key.strip():
        raise ValueError("a proposal must carry a why and an evidence key (grounded-or-silent)")
    key = dedupe_key(title, authors, year)
    with conn:
        cur = conn.execute(
            "INSERT INTO reading_proposals (kind, dedupe_key, external_id, title, authors, year, "
            "url, abstract, oa_pdf_url, why, why_kind, evidence_key, score, status, created_at, "
            "source_run) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,'candidate',?,?) "
            "ON CONFLICT(dedupe_key) DO NOTHING",
            (kind, key, external_id, title, authors or None, year, url, abstract, oa_pdf_url,
             why, why_kind, evidence_key, score, _utcnow(), source_run),
        )
        return cur.lastrowid if cur.rowcount else None


def mark_proposed(
    conn: sqlite3.Connection,
    proposal_id: int,
    *,
    filename: str,
    device_uuid: str | None = None,
    device_folder: str = FOLDER_PROPOSED,
    local_path: str | None = None,
) -> None:
    """Record that a candidate has been delivered to the device and is now awaiting his verdict."""
    with conn:
        conn.execute(
            "UPDATE reading_proposals SET status='proposed', filename=?, device_uuid=?, "
            "device_folder=?, local_path=COALESCE(?, local_path), proposed_at=? WHERE id=?",
            (filename, device_uuid, device_folder, local_path, _utcnow(), proposal_id),
        )


def set_status(
    conn: sqlite3.Connection,
    proposal_id: int,
    status: str,
    *,
    device_folder: str | None = None,
    resolution: str | None = None,
) -> None:
    """Move a proposal to `status`, stamping `resolved_at` for the terminal ones.

    `resolution` records HOW it ended (`moved`/`ttl`/`removed`/`manual`) — the distinction the
    flywheel needs and `acceptance_log` cannot express.
    """
    if status not in STATUSES:
        raise ValueError(f"unknown status {status!r}")
    if resolution and resolution not in ("moved", "ttl", "removed", "manual"):
        raise ValueError(f"unknown resolution {resolution!r}")
    resolved = _utcnow() if status in ("accepted", "ingested", "rejected", "superseded") else None
    with conn:
        conn.execute(
            "UPDATE reading_proposals SET status=?, device_folder=COALESCE(?, device_folder), "
            "resolution=COALESCE(?, resolution), resolved_at=COALESCE(?, resolved_at) WHERE id=?",
            (status, device_folder, resolution, resolved, proposal_id),
        )


def record_verdict(conn: sqlite3.Connection, key: str, verdict: str) -> None:
    """Append a discovery judgment to `acceptance_log` — the flywheel substrate.

    Keyed by `dedupe_key` rather than row id so the judgment survives the row being superseded;
    the surface is `discovery`, never `reading` (migration 0017 explains the split).
    """
    if verdict not in ("kept", "rejected"):
        raise ValueError(f"unknown verdict {verdict!r}")
    with conn:
        conn.execute(
            "INSERT INTO acceptance_log (surface, candidate_key, verdict, at) VALUES (?,?,?,?)",
            (SURFACE_DISCOVERY, key, verdict, _utcnow()),
        )


def link_target(
    conn: sqlite3.Connection,
    *,
    source_uri: str,
    doc_uuid: str | None = None,
    device_path: str | None = None,
    proposal_id: int | None = None,
    linked_by: str = "delivery",
) -> None:
    """Map a reMarkable document to the corpus `source_uri` it was ingested as.

    Without this the marks he makes never reach the document: `pdf_annotations.source_uri` holds a
    DEVICE path and `documents.source_uri` a filesystem path, so the join that measures engagement
    scores his most-annotated reading zero (migration 0017).
    """
    with conn:
        conn.execute(
            "INSERT INTO reading_targets (doc_uuid, device_path, source_uri, proposal_id, "
            "linked_by, created_at) VALUES (?,?,?,?,?,?) "
            "ON CONFLICT(doc_uuid) DO UPDATE SET source_uri=excluded.source_uri, "
            "device_path=COALESCE(excluded.device_path, device_path)",
            (doc_uuid, device_path, source_uri, proposal_id, linked_by, _utcnow()),
        )


# --- reads ------------------------------------------------------------------------------------


def list_proposals(
    conn: sqlite3.Connection, *, status: str | None = None, kind: str | None = None,
    limit: int = 100,
) -> list[Proposal]:
    sql = "SELECT * FROM reading_proposals WHERE 1=1"
    args: list = []
    if status:
        sql += " AND status=?"
        args.append(status)
    if kind:
        sql += " AND kind=?"
        args.append(kind)
    sql += " ORDER BY COALESCE(score, 0) DESC, id ASC LIMIT ?"
    args.append(limit)
    return [_row(r) for r in conn.execute(sql, args)]


def get_by_filename(conn: sqlite3.Connection, filename: str) -> Proposal | None:
    r = conn.execute(
        "SELECT * FROM reading_proposals WHERE filename=? ORDER BY id DESC LIMIT 1", (filename,)
    ).fetchone()
    return _row(r) if r else None


def get_by_uuid(conn: sqlite3.Connection, device_uuid: str) -> Proposal | None:
    r = conn.execute(
        "SELECT * FROM reading_proposals WHERE device_uuid=? ORDER BY id DESC LIMIT 1",
        (device_uuid,),
    ).fetchone()
    return _row(r) if r else None


def slots_free(conn: sqlite3.Connection, kind: str, *, caps: dict[str, int] | None = None) -> int:
    """How many more of `kind` may be delivered — a cap on the STOCK sitting in `Proposed`.

    Not a rate. A weekly quota keeps topping up a folder he has not cleared, which is how a
    reading list becomes a guilt metric; a stock cap means an untouched folder proposes nothing.
    """
    cap = (caps or DEFAULT_CAPS).get(kind, 0)
    held = conn.execute(
        "SELECT COUNT(*) n FROM reading_proposals WHERE status='proposed' AND kind=?", (kind,)
    ).fetchone()["n"]
    return max(cap - held, 0)


def channel_stats(conn: sqlite3.Connection) -> dict[str, dict[str, int]]:
    """Per-`why_kind` outcome counts — what the flywheel actually learns from.

    Per-CHANNEL, not per-item: there are three judgments on the whole reading surface today, so an
    item-level prior would be noise wearing the costume of evidence. "discovery 4/5, co_citation
    0/6" is a claim that small numbers can support.

    Read from `reading_proposals` rather than `acceptance_log` because only this table records HOW
    a proposal ended. `ttl` and `removed` are both rejections and must not be pooled: one means he
    left it sitting, the other means he threw it away. Keys: `kept`, `ttl`, `removed`, `open`.
    """
    out: dict[str, dict[str, int]] = {}
    for r in conn.execute(
        "SELECT why_kind AS k, status AS s, resolution AS res, COUNT(*) AS n "
        "FROM reading_proposals GROUP BY 1, 2, 3"
    ):
        bucket = out.setdefault(r["k"], {"kept": 0, "ttl": 0, "removed": 0, "open": 0})
        if r["s"] in ("accepted", "ingested"):
            bucket["kept"] += r["n"]
        elif r["s"] == "rejected":
            bucket[r["res"] if r["res"] in ("ttl", "removed") else "removed"] += r["n"]
        elif r["s"] in ("candidate", "proposed"):
            bucket["open"] += r["n"]
    return out


def annotated_source_uris(conn: sqlite3.Connection) -> dict[str, int]:
    """`documents.source_uri` -> how many marks he has made on it, resolved through the mapping.

    This is the engagement signal the §7 ranking wants, and it returns nothing useful until
    `reading_targets` has been populated — which is the whole point of migration 0017.
    """
    rows = conn.execute(
        """
        SELECT t.source_uri AS uri, COUNT(*) AS n
        FROM pdf_annotations a
        JOIN reading_targets t
          ON (a.doc_uuid IS NOT NULL AND a.doc_uuid = t.doc_uuid)
          OR (a.doc_uuid IS NULL AND a.source_uri = t.device_path)
        GROUP BY t.source_uri
        """
    ).fetchall()
    return {r["uri"]: r["n"] for r in rows}
