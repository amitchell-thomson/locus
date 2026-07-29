"""Data access for the Phase-2 agent-state tables (agent-layer plan §6.2-6.4).

`journal.py` is this module's sibling: the same "agent state lives in its own tables, the spine is
never touched" contract, applied to objects / links / belief positions / acceptance. Every consumer
(structure/propose, evolve/trajectory, learn/*, the MCP surface) goes through here so the write
rules live in ONE place:

  1. **Propose, never mutate (invariant 2).** `upsert_object` never changes an existing object's
     `status`. An agent re-proposing a subject the owner has already blessed (`active`) or
     retired (`archived`) updates its BODY only — the owner's decision is not an agent's to undo.
  2. **Additive body merge.** Re-proposing merges: list fields union (new open threads/learnings
     accumulate), scalar fields fill only where the stored value is empty. A second proposal can
     therefore never delete a thread the owner has been tracking, which a wholesale body replace
     would do silently.
  3. **Stable string keys.** Links and belief subjects address a `source_uri` / canonical
     `(name,type)` / object id — NEVER a doc row id, which a re-ingest changes. (The round-7 eval
     labels learned this the hard way when `retitle` invalidated every title-substring key.)

Positions are deduped by their UNIQUE constraint: re-running the proposer over an unchanged note
records nothing new, so a trajectory does not grow phantom repetitions of one stance.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Callable, Iterable

# Separator inside a canonical-entity key. A unit separator cannot occur in an entity name, so
# `name\x1ftype` round-trips unambiguously (a ':' or '|' would collide with real names).
_KEY_SEP = "\x1f"

OBJECT_TYPES = ("project", "concept", "question", "reading")
RELATIONS = ("implements", "about", "raised_by", "answered_by", "reads", "relates")


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


def entity_key(name: str, type_: str) -> str:
    """Stable key for a canonical entity subject/target."""
    return f"{name}{_KEY_SEP}{type_}"


def parse_entity_key(key: str) -> tuple[str, str]:
    """Inverse of `entity_key`; a key without a separator is treated as an untyped name."""
    name, _, type_ = key.partition(_KEY_SEP)
    return name, type_ or "concept"


@dataclass
class ObjectLink:
    target_kind: str  # 'doc' | 'entity' | 'object'
    target_key: str   # source_uri | entity_key(name,type) | str(object_id)
    relation: str


@dataclass
class AgentObject:
    id: int
    type: str
    title: str
    status: str
    maturity: str
    body: dict
    created_at: str
    updated_at: str
    source_run: int | None = None
    links: list[ObjectLink] = field(default_factory=list)


def _row_to_object(row) -> AgentObject:
    return AgentObject(
        id=row["id"], type=row["type"], title=row["title"], status=row["status"],
        maturity=row["maturity"], body=json.loads(row["body"] or "{}"),
        created_at=row["created_at"], updated_at=row["updated_at"],
        source_run=row["source_run"],
    )


def merge_body(existing: dict, incoming: dict) -> dict:
    """Additively merge a proposed body into a stored one (rule 2).

    Lists union preserving order (stored items first, new ones appended, exact duplicates
    dropped); scalars fill only where the stored value is missing/empty; nested dicts recurse.
    The asymmetry is deliberate — an agent may ADD to what the owner is tracking, never silently
    replace it."""
    merged = dict(existing)
    for key, new in incoming.items():
        old = merged.get(key)
        if isinstance(old, list) and isinstance(new, list):
            merged[key] = old + [v for v in new if v not in old]
        elif isinstance(old, dict) and isinstance(new, dict):
            merged[key] = merge_body(old, new)
        elif old in (None, "", [], {}):
            merged[key] = new
    for key, new in incoming.items():
        merged.setdefault(key, new)
    return merged


# --- objects ---------------------------------------------------------------------------------


def find_object(conn, type_: str, title: str) -> AgentObject | None:
    row = conn.execute(
        "SELECT * FROM objects WHERE type=? AND title=?", (type_, title)
    ).fetchone()
    return _row_to_object(row) if row else None


def get_object(conn, object_id: int) -> AgentObject | None:
    row = conn.execute("SELECT * FROM objects WHERE id=?", (object_id,)).fetchone()
    if row is None:
        return None
    obj = _row_to_object(row)
    obj.links = links_for(conn, object_id)
    return obj


def upsert_object(
    conn,
    *,
    type_: str,
    title: str,
    body: dict | None = None,
    maturity: str = "rough",
    source_run: int | None = None,
    now: Callable[[], str] = _utcnow,
) -> tuple[int, bool]:
    """Create a proposed object, or additively update an existing one. Returns (id, created).

    NEVER writes `status` on an update (rule 1): a blessed object stays blessed, an archived one
    stays archived, and only the owner moves it. Committed immediately so a crash mid-run keeps
    the objects already proposed (the per-verdict-commit contract `link`/`retitle` set)."""
    if type_ not in OBJECT_TYPES:
        raise ValueError(f"unknown object type {type_!r}; expected one of {OBJECT_TYPES}")
    stamp = now()
    existing = find_object(conn, type_, title)
    if existing is None:
        with conn:
            cur = conn.execute(
                "INSERT INTO objects (type, title, status, maturity, body, created_at, "
                "updated_at, source_run) VALUES (?,?,'proposed',?,?,?,?,?)",
                (type_, title, maturity, json.dumps(body or {}), stamp, stamp, source_run),
            )
        return int(cur.lastrowid), True
    merged = merge_body(existing.body, body or {})
    if merged == existing.body:
        return existing.id, False
    with conn:
        conn.execute(
            "UPDATE objects SET body=?, updated_at=? WHERE id=?",
            (json.dumps(merged), stamp, existing.id),
        )
    return existing.id, False


def set_status(conn, object_id: int, status: str, *, now: Callable[[], str] = _utcnow) -> bool:
    """The owner's blessing/retirement — the ONLY writer of `status`. Returns False if unknown."""
    if status not in ("proposed", "active", "archived"):
        raise ValueError(f"unknown status {status!r}")
    with conn:
        cur = conn.execute(
            "UPDATE objects SET status=?, updated_at=? WHERE id=?", (status, now(), object_id)
        )
    return cur.rowcount > 0


def list_objects(
    conn, *, type_: str | None = None, status: str | None = None, limit: int = 100
) -> list[AgentObject]:
    sql = "SELECT * FROM objects"
    clauses, params = [], []
    if type_:
        clauses.append("type=?")
        params.append(type_)
    if status:
        clauses.append("status=?")
        params.append(status)
    if clauses:
        sql += " WHERE " + " AND ".join(clauses)
    sql += " ORDER BY updated_at DESC, id DESC LIMIT ?"
    params.append(limit)
    return [_row_to_object(r) for r in conn.execute(sql, params)]


# --- links -----------------------------------------------------------------------------------


def add_links(conn, object_id: int, links: Iterable[ObjectLink]) -> int:
    """Attach grounding links to an object (idempotent via the UNIQUE constraint). Returns the
    number newly inserted."""
    added = 0
    with conn:
        for link in links:
            if link.relation not in RELATIONS:
                raise ValueError(f"unknown relation {link.relation!r}")
            cur = conn.execute(
                "INSERT OR IGNORE INTO object_links (object_id, target_kind, target_key, relation) "
                "VALUES (?,?,?,?)",
                (object_id, link.target_kind, link.target_key, link.relation),
            )
            added += cur.rowcount
    return added


def links_for(conn, object_id: int) -> list[ObjectLink]:
    return [
        ObjectLink(r["target_kind"], r["target_key"], r["relation"])
        for r in conn.execute(
            "SELECT target_kind, target_key, relation FROM object_links WHERE object_id=? "
            "ORDER BY id",
            (object_id,),
        )
    ]


def objects_linking_to(conn, target_kind: str, target_key: str) -> list[AgentObject]:
    """Every object grounded in one doc/entity — the reverse lookup the MCP reads use."""
    rows = conn.execute(
        "SELECT o.* FROM objects o JOIN object_links l ON l.object_id=o.id "
        "WHERE l.target_kind=? AND l.target_key=? ORDER BY o.id",
        (target_kind, target_key),
    ).fetchall()
    return [_row_to_object(r) for r in rows]


# --- belief positions ------------------------------------------------------------------------


@dataclass
class Position:
    id: int
    subject_kind: str
    subject_key: str
    stance: str
    source_doc_id: int | None
    dated_at: str
    source_run: int | None = None
    # The note's path — STABLE across re-ingest, unlike source_doc_id (notes_sync replaces a
    # changed note with a NEW document id, orphaning the id). Prefer this for provenance.
    source_uri: str | None = None


def record_position(
    conn,
    *,
    subject_kind: str,
    subject_key: str,
    stance: str,
    dated_at: str,
    source_doc_id: int | None = None,
    source_uri: str | None = None,
    source_run: int | None = None,
    now: Callable[[], str] = _utcnow,
) -> int | None:
    """Append one dated stance. Returns the new id, or None if it was already recorded.

    Dedup is the table's UNIQUE (subject, stance, source_doc) — re-running the proposer over an
    unchanged note must not stack the same position again, or the trajectory reads as a belief
    the owner kept restating."""
    if subject_kind not in ("concept", "project"):
        raise ValueError(f"unknown subject_kind {subject_kind!r}")
    with conn:
        cur = conn.execute(
            "INSERT OR IGNORE INTO belief_positions (subject_kind, subject_key, stance, "
            "source_doc_id, source_uri, source_run, dated_at, created_at) VALUES (?,?,?,?,?,?,?,?)",
            (subject_kind, subject_key, stance, source_doc_id, source_uri, source_run, dated_at,
             now()),
        )
    return int(cur.lastrowid) if cur.rowcount else None


def positions_for(conn, subject_kind: str, subject_key: str) -> list[Position]:
    """The trajectory: one subject's stances oldest-first by the SOURCE date (§6.3)."""
    rows = conn.execute(
        "SELECT id, subject_kind, subject_key, stance, source_doc_id, source_uri, source_run, "
        "dated_at FROM belief_positions WHERE subject_kind=? AND subject_key=? "
        "ORDER BY dated_at, id",
        (subject_kind, subject_key),
    ).fetchall()
    return [
        Position(
            id=r["id"], subject_kind=r["subject_kind"], subject_key=r["subject_key"],
            stance=r["stance"], source_doc_id=r["source_doc_id"], dated_at=r["dated_at"],
            source_run=r["source_run"], source_uri=r["source_uri"],
        )
        for r in rows
    ]


def subjects_with_positions(conn, *, limit: int = 100) -> list[tuple[str, str, int]]:
    """(subject_kind, subject_key, count) for every subject that has a trajectory."""
    return [
        (r["subject_kind"], r["subject_key"], r["n"])
        for r in conn.execute(
            "SELECT subject_kind, subject_key, COUNT(*) AS n FROM belief_positions "
            "GROUP BY subject_kind, subject_key ORDER BY n DESC, subject_key LIMIT ?",
            (limit,),
        )
    ]


# --- acceptance log --------------------------------------------------------------------------


def log_acceptance(
    conn, *, surface: str, candidate_key: str, verdict: str, now: Callable[[], str] = _utcnow
) -> int:
    """Record a keep/reject — the flywheel's free relevance label (§12.1)."""
    if verdict not in ("kept", "rejected"):
        raise ValueError(f"unknown verdict {verdict!r}")
    with conn:
        cur = conn.execute(
            "INSERT INTO acceptance_log (surface, candidate_key, verdict, at) VALUES (?,?,?,?)",
            (surface, candidate_key, verdict, now()),
        )
    return int(cur.lastrowid)


def acceptance_counts(conn, surface: str | None = None) -> dict[str, dict[str, int]]:
    """candidate_key -> {kept: n, rejected: n} — what the flywheel folds into ranking."""
    sql = "SELECT candidate_key, verdict, COUNT(*) AS n FROM acceptance_log"
    params: list = []
    if surface:
        sql += " WHERE surface=?"
        params.append(surface)
    sql += " GROUP BY candidate_key, verdict"
    out: dict[str, dict[str, int]] = {}
    for r in conn.execute(sql, params):
        out.setdefault(r["candidate_key"], {})[r["verdict"]] = r["n"]
    return out
