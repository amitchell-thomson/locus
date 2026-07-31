"""Build the vectors a candidate paper is ranked AGAINST — his projects, and his open gaps.

The engine's whole premise is that his work is already embedded, so a harvested abstract and a
project of his can be compared directly. This module makes that concrete: it renders each blessed
project into a short piece of text describing what it IS, embeds it with the same
nomic-embed-text the corpus went through, and stores both.

WHY THE TEXT IS STORED ALONGSIDE THE VECTOR. A profile is a lossy summary of a project, and a
ranking nobody can inspect is a ranking nobody should trust. When a paper is proposed "for the
regime-detection project", the question "what did you think that project was?" has to have a
readable answer, or the grounded-or-silent invariant is decorative.

A project's text is drawn from the documents it is GROUNDED IN (`object_links`), not from the
object title alone: "regime ml" as a bare string embeds to almost nothing useful, while the thesis
and method of the write-ups behind it describe the actual problem. Gaps are the weaker signal and
are treated as such — a bare concept name carries little, so it is embedded together with the
projects that raised it.
"""

from __future__ import annotations

import json
import sqlite3
import struct
from dataclasses import dataclass, field
from datetime import datetime, timezone

from locus.agent import state

# How much source text to embed per profile. nomic truncates long inputs, and a project's first
# few documents describe it as well as its twentieth does.
_MAX_CHARS = 4000


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


def vec_blob(vec: list[float]) -> bytes:
    """Little-endian float32 blob, the layout sqlite-vec stores (mirrors ingest_pipeline)."""
    return struct.pack(f"{len(vec)}f", *vec)


@dataclass
class Profile:
    subject_kind: str   # 'project' | 'gap'
    subject_key: str
    label: str
    text: str
    # The documents the profile was built from. Ranking excludes these when measuring "do I
    # already have this?" — see migration 0018 and `rank._familiarity`.
    doc_ids: list[int] = field(default_factory=list)


def _project_text(
    conn: sqlite3.Connection, object_id: int, title: str, prefixes: tuple[str, ...] = ()
) -> tuple[str, list[int]]:
    """What a project IS, in the words of the documents behind it, and which those were."""
    from locus.learn.gaps import _doc_ids_for_object

    doc_ids = keep_doc_ids(conn, _doc_ids_for_object(conn, object_id), prefixes)
    parts = [title]
    if doc_ids:
        marks = ",".join("?" * len(doc_ids))
        for r in conn.execute(
            f"SELECT title, thesis, method FROM documents WHERE id IN ({marks})", doc_ids
        ):
            parts += [p for p in (r["title"], r["thesis"], r["method"]) if p]
    return " ".join(" ".join(parts).split())[:_MAX_CHARS], doc_ids


def _excluded_prefixes() -> tuple[str, ...]:
    """`source_uri` prefixes that are not real reading material.

    Reuses `[retrieve].exclude_source_uris` rather than inventing a second list, so the two
    layers cannot drift apart. It exists because Locus has ingested ITSELF, and the first live
    ranking showed exactly why that matters here: the `locus` project profile is generic
    ML-and-systems vocabulary, so it matched almost every machine-learning abstract on arXiv and
    took 8 of the top 12 slots — ocean circulation, building energy, tennis. It is a codebase he
    maintains, not a subject he is researching.
    """
    from locus.config import load

    return tuple(load().retrieve.exclude_source_uris or ())


def keep_doc_ids(
    conn: sqlite3.Connection, doc_ids: list[int], prefixes: tuple[str, ...]
) -> list[int]:
    """Drop grounding documents under an excluded prefix, keeping the rest.

    Filtering the SOURCES rather than dropping the whole profile is the right shape: an object may
    be grounded in a mix (the live `locus` object points at two self-ingested paths and two
    unrelated handouts), and requiring every document to be excluded before acting meant the
    exclusion never fired at all.
    """
    if not doc_ids or not prefixes:
        return list(doc_ids)
    marks = ",".join("?" * len(doc_ids))
    rows = conn.execute(
        f"SELECT id, source_uri FROM documents WHERE id IN ({marks})", doc_ids
    ).fetchall()
    return [
        r["id"] for r in rows
        if not any((r["source_uri"] or "").startswith(p) for p in prefixes)
    ]


def is_bare_acronym(concept: str) -> bool:
    """A short all-caps token with no context — a terrible thing to embed.

    Measured 2026-07-31: the gap concept `AIS` (as in AIS vessel-tracking data) retrieved
    "AI Strategy: How to Choose What AI Product to Implement" as its top match, because a
    three-letter token embeds essentially as "AI". `AEX`, `DAX`, `GMV` and `TTF` are the same
    shape — instrument and venue tickers that the entity pass emits as concepts. They match noise
    rather than nothing, which is worse: noise scores.

    The expanded form is what carries meaning, so a bare acronym is dropped rather than guessed
    at. (Reuniting it with its expansion is `entity_aliases`' job, and the fuller gap filter tier
    the plan defers.)
    """
    token = concept.strip()
    return len(token) <= 6 and token.isupper() and " " not in token


def is_excluded_project(
    conn: sqlite3.Connection, object_id: int, prefixes: tuple[str, ...]
) -> bool:
    """True when the object IS an excluded project — not merely linked to one.

    The test is an exact `source_uri` match against an excluded prefix, i.e. the repo's own
    root document. That is precise enough to be safe: a project that happens to cite a file
    inside another repo is untouched, only the repo itself is dropped.

    Filtering this object's documents was not enough, and made things worse. The live `locus`
    object is grounded in four documents: two self-ingested paths, plus — through a structurer
    mislink — an unrelated syllabus and a quant paper on nonlinear VAR forecasting. Removing the
    two real ones left a profile whose text DESCRIBED A TIME-SERIES FINANCE PAPER, so "locus"
    then attracted every regime-switching abstract in the harvest and mislabelled it. A profile
    built from the leftovers of an excluded project describes nothing anyone chose.
    """
    from locus.learn.gaps import _doc_ids_for_object

    doc_ids = _doc_ids_for_object(conn, object_id)
    if not doc_ids or not prefixes:
        return False
    marks = ",".join("?" * len(doc_ids))
    uris = [
        (r["source_uri"] or "").rstrip("/")
        for r in conn.execute(
            f"SELECT source_uri FROM documents WHERE id IN ({marks})", doc_ids
        )
    ]
    return any(u in {p.rstrip("/") for p in prefixes} for u in uris)


def collect(conn: sqlite3.Connection, *, gap_limit: int = 40) -> list[Profile]:
    """The profiles worth ranking against: every active project, then the open gaps."""
    out: list[Profile] = []

    excluded_uris = _excluded_prefixes()
    for obj in state.list_objects(conn, type_="project", status="active", limit=100):
        if is_excluded_project(conn, obj.id, excluded_uris):
            continue
        text, doc_ids = _project_text(conn, obj.id, obj.title, excluded_uris)
        if len(text) <= len(obj.title) + 20:   # not grounded in anything real, just a title
            continue
        out.append(Profile("project", str(obj.id), obj.title, text, doc_ids))

    # Gaps are the secondary signal. `non_topical_names` is the same predicate the structurer and
    # related-docs use, so the layers keep agreeing what counts as a concept — without it a code
    # repo's gaps are its Alembic boilerplate (learn/gaps._excluded_names).
    from locus.learn.reread import open_gap_concepts
    from locus.link.related import non_topical_names

    excluded = {n.lower() for n in non_topical_names(conn)}
    gaps = open_gap_concepts(conn)
    for concept, owners in sorted(gaps.items())[:gap_limit]:
        if concept.lower() in excluded or len(concept) < 4 or is_bare_acronym(concept):
            continue
        out.append(Profile(
            "gap", concept, concept,
            f"{concept}. Relevant to: {', '.join(sorted(owners)[:3])}",
        ))
    return out


def rebuild(
    conn: sqlite3.Connection, *, embed_fn=None, gap_limit: int = 40
) -> list[Profile]:
    """Recompute every profile and its vector. Derived and regenerable (principle 9)."""
    if embed_fn is None:
        from locus.ingest.embed import embed_texts as embed_fn

    profiles = collect(conn, gap_limit=gap_limit)
    if not profiles:
        return []

    vectors = embed_fn([p.text for p in profiles])
    with conn:
        conn.execute("DELETE FROM discovery_profile_vectors")
        conn.execute("DELETE FROM discovery_profiles")
        for prof, vec in zip(profiles, vectors):
            cur = conn.execute(
                "INSERT INTO discovery_profiles (subject_kind, subject_key, label, text, "
                "doc_ids, built_at) VALUES (?,?,?,?,?,?)",
                (prof.subject_kind, prof.subject_key, prof.label, prof.text,
                 json.dumps(prof.doc_ids), _utcnow()),
            )
            conn.execute(
                "INSERT INTO discovery_profile_vectors (profile_id, embedding) VALUES (?,?)",
                (cur.lastrowid, vec_blob(vec)),
            )
    return profiles
