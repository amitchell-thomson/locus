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

# `idea` (0016): what reading actually produces. A question is something he does not know, a
# concept something that exists, a project something he is building — an IDEA is something he
# MIGHT build, and marginalia is where they come from.
OBJECT_TYPES = ("project", "concept", "question", "reading", "idea")
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


# Reserved body keys recording what the OWNER has authored. They are metadata about authority,
# never content, so an incoming agent body can neither read nor write them (docs/owner-authority-design.md).
OWNER_EDITS_KEY = "_owner_edits"      # field -> {at, source}: the owner authored this field
OWNER_REMOVED_KEY = "_owner_removed"  # field -> [items]: the owner struck these from a list
_RESERVED_KEYS = (OWNER_EDITS_KEY, OWNER_REMOVED_KEY)


def merge_body(existing: dict, incoming: dict) -> dict:
    """Additively merge a proposed (AGENT) body into a stored one (rule 2).

    Lists union preserving order (stored items first, new ones appended, exact duplicates
    dropped); scalars fill only where the stored value is missing/empty; nested dicts recurse.
    The asymmetry is deliberate — an agent may ADD to what the owner is tracking, never silently
    replace it.

    Owner authority (2026-07-30). Emptiness was being used as a proxy for "nobody has decided
    this yet", and it is a leaky one, so an owner edit now leaves an explicit marker that this
    merge honours. Two holes closed, both cases where the additive merge silently overrode an
    owner decision:

      - a field the owner deliberately CLEARED read as empty, so the agent refilled it;
      - a list item the owner REMOVED was re-appended by the next union.

    Both rules only ever make the agent do less; nothing here lets it overwrite anything it
    could not overwrite before. The owner's own write path is `apply_owner_edit`, which is where
    overwriting lives — see docs/owner-authority-design.md for why that split is the right shape.
    """
    merged = dict(existing)
    owner_fields = set(existing.get(OWNER_EDITS_KEY, {}))
    removed = existing.get(OWNER_REMOVED_KEY, {})

    for key, new in incoming.items():
        if key in _RESERVED_KEYS:
            continue  # authority metadata is not the agent's to assert
        if key in owner_fields:
            continue  # the owner authored this field; an empty value is still their decision
        old = merged.get(key)
        if isinstance(old, list) and isinstance(new, list):
            struck = removed.get(key, [])
            merged[key] = old + [v for v in new if v not in old and v not in struck]
        elif isinstance(old, dict) and isinstance(new, dict):
            merged[key] = merge_body(old, new)
        elif old in (None, "", [], {}):
            merged[key] = new
    for key, new in incoming.items():
        if key in _RESERVED_KEYS or key in owner_fields:
            continue
        if key not in merged:
            struck = removed.get(key, [])
            merged[key] = (
                [v for v in new if v not in struck] if isinstance(new, list) and struck else new
            )
    return merged


def apply_owner_edit(
    conn,
    object_id: int,
    edits: dict,
    *,
    source: str,
    remove: dict | None = None,
    now: Callable[[], str] = _utcnow,
) -> bool:
    """The OWNER's write path: replace fields outright and record that they did.

    This is the counterpart to `upsert_object`, not a variant of it. The propose-never-mutate
    invariant constrains the AGENT; the owner is the authority and a hand-written correction on
    the daily page must be able to overwrite agent text. Keeping them as separate verbs states
    that directly instead of leaning on emptiness to imply authority.

    `edits` replaces scalars wholesale. `remove` strikes items from list fields. Both record
    markers (`_owner_edits` / `_owner_removed`) so the additive agent merge cannot undo the edit
    on the next structure run — without them a correction would survive only until tonight, and
    having to re-make it nightly is precisely the chore §9 forbids.

    Never writes `status`: correcting an object and blessing it stay separate, independently
    auditable acts (blessing is `set_status`). Returns False if the object is unknown.
    """
    obj = get_object(conn, object_id)
    if obj is None:
        return False

    body = dict(obj.body)
    stamp = now()
    marks = dict(body.get(OWNER_EDITS_KEY, {}))
    struck = {k: list(v) for k, v in body.get(OWNER_REMOVED_KEY, {}).items()}

    for key, value in (edits or {}).items():
        if key in _RESERVED_KEYS:
            raise ValueError(f"{key!r} is authority metadata, not an editable field")
        body[key] = value
        marks[key] = {"at": stamp, "source": source}

    for key, items in (remove or {}).items():
        if key in _RESERVED_KEYS:
            raise ValueError(f"{key!r} is authority metadata, not an editable field")
        current = body.get(key)
        if isinstance(current, list):
            body[key] = [v for v in current if v not in items]
        struck.setdefault(key, [])
        struck[key] += [v for v in items if v not in struck[key]]
        # NOTE: a removal does NOT mark the field as owner-authored. Striking one item means
        # "not that one", not "this list is closed" — marking the field would freeze it and the
        # agent could never append a genuinely new thread again. The `_owner_removed` tombstone
        # is precisely scoped to the items struck, which is all the durability this needs.

    body[OWNER_EDITS_KEY] = marks
    if struck:
        body[OWNER_REMOVED_KEY] = struck

    with conn:
        conn.execute(
            "UPDATE objects SET body=?, updated_at=? WHERE id=?",
            (json.dumps(body), stamp, object_id),
        )
    return True


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
    """Every object grounded in one doc/entity — the reverse of `links_for`.

    NO PRODUCTION CALLER TODAY (the docstring used to claim the MCP reads used it, and they do
    not). Kept as the reverse accessor the tests assert the link table with; if it is still
    unused when something else needs to change here, delete it rather than re-describing it.
    """
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


def development_positions(conn, subject_kind: str, subject_key: str) -> list[Position]:
    """The owner's own successive passes on a thread, read back as positions.

    THE GAP THIS CLOSES, stated precisely. His thinking over time was recorded in two places:
    `belief_positions`, written by the structurer when it EXTRACTS a stance from a note, and
    `objects.body.development`, appended when he WRITES on a thread on the daily page. Only the
    first had a reader — and `record_position` accepts only `concept` and `project` subjects, so
    a THREAD (an idea or a question) could not have a trajectory at all. The chain he builds by
    hand, pass by pass, on the surface he touches every morning, was the one chain `locus
    evolution` could not show him.

    The two stores are not redundant: they have different provenance, one extracted and one
    authored. So the fix is one READ path rather than one write path — merged here and ordered by
    date they are a single chain, and because nothing is copied neither store can drift from the
    other.

    `subject_kind='object'` addresses a thread by its object id. A canonical concept has no body
    to append to, so there is nothing to merge and this returns nothing.
    """
    if subject_kind not in ("object", "project") or not str(subject_key).isdigit():
        return []
    obj = get_object(conn, int(subject_key))
    if obj is None:
        return []

    out: list[Position] = []
    for entry in (obj.body or {}).get("development") or []:
        if isinstance(entry, dict):
            text, at = str(entry.get("text", "")).strip(), str(entry.get("at", "") or "")
        else:
            text, at = str(entry).strip(), ""
        if not text:
            continue
        out.append(Position(
            id=0, subject_kind=subject_kind, subject_key=str(subject_key), stance=text,
            source_doc_id=None, dated_at=at or (obj.updated_at or "")[:10],
            source_run=None, source_uri=None,
        ))
    return out


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


def owner_authored_sql(alias: str = "d") -> tuple[str, list]:
    """SQL predicate for "this document is HIS writing" — ONE definition, several consumers.

    The same rule `structure.propose._is_owner_authored` applies in Python, expressed for a query.
    Provenance first: everything he writes lands under the notes directory whatever category the
    DEVICE FOLDER assigned (`Notes/engineering` -> coursework), so keying on category alone
    silently made his own handwriting not-his. Then category + format, both load-bearing, because
    `project` also holds third-party manuals he keeps for reference.

    Kept here rather than in `structure/` because three layers ask this question — the proposer,
    the daily page's connection source, and re-read ranking — and they were answering it three
    different ways, all of them `category = 'note'`.
    """
    from locus.config import load

    cfg = load()
    notes = str(cfg.paths.notes).replace("\\", "/").rstrip("/")
    clause = f"({alias}.source_uri LIKE ? OR {alias}.source_uri LIKE ?"
    params: list = [f"{notes}/%", "%/vault/notes/%"]
    cats = list(cfg.structure.belief_source_categories)
    if cats:
        sub = f"{alias}.category IN ({','.join('?' * len(cats))})"
        params += cats
        types = list(cfg.structure.belief_source_types)
        if types:
            sub += f" AND {alias}.source_type IN ({','.join('?' * len(types))})"
            params += types
        clause += f" OR ({sub})"
    return clause + ")", params


def dropped_object_ids(conn) -> set[int]:
    """Objects he REJECTED, as opposed to ones he finished with.

    `status='archived'` conflates two opposite judgements, which is why reading it alone gets
    this wrong in one direction or the other. A cross on the daily page archives an object
    ("a cross means NO"); so does a tick that RESOLVES a question. Live 2026-08-03: objs 55 and
    67 were rejected, while 78 and 79 are archived and `kept` — he answered them.

    The distinguishing fact is his own recorded judgement, so this reads the latest verdict per
    object from `acceptance_log`. Consumers that must not resurface something he threw away
    (thread linking, thread context) subtract this rather than filtering on `archived`.
    """
    latest: dict[int, str] = {}
    try:
        rows = conn.execute(
            "SELECT candidate_key, verdict FROM acceptance_log WHERE surface='object' "
            "ORDER BY at, id"
        ).fetchall()
    except sqlite3.OperationalError:
        return set()
    for row in rows:
        key = str(row["candidate_key"])
        if key.isdigit():
            latest[int(key)] = row["verdict"]
    return {oid for oid, verdict in latest.items() if verdict == "rejected"}


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
