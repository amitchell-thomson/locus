"""Build the vectors a candidate paper is ranked AGAINST — his projects, and his open gaps.

The engine's whole premise is that his work is already embedded, so a harvested abstract and a
project of his can be compared directly. This module makes that concrete: it renders each blessed
project into a short piece of text describing what it IS, embeds it with the same
nomic-embed-text the corpus went through, and stores both.

WHY THE TEXT IS STORED ALONGSIDE THE VECTOR. A profile is a lossy summary of a project, and a
ranking nobody can inspect is a ranking nobody should trust. When a paper is proposed "for the
regime-detection project", the question "what did you think that project was?" has to have a
readable answer, or the grounded-or-silent invariant is decorative.

A subject gets MANY vectors, not one, and a candidate matches on its BEST facet (migration 0019).
The first version embedded one string per project — title, thesis and method, 289 characters for
`regime-ml` against 34,856 available — and it matched relevant work roughly by luck, because that
string is the elevator pitch. Everything specific enough to match a METHOD paper lives below it:
88 section summaries, the `result` and `limitations` fields, and 263 named methods and concepts,
none of which were being used.

Simply concatenating all of it would not work either. nomic truncates around 2k tokens, and
averaging a whole repository into one vector makes every project collapse toward "a machine
learning system with a data pipeline" — generic exactly where it must be specific. Facets keep
each slice sharp: a section about tuning state persistence can match a paper on sticky HMM priors,
while the project pitch never will.

Gaps stay single-facet and weaker by design — a bare concept name carries little, so it is
embedded together with the projects that raised it.
"""

from __future__ import annotations

import json
import sqlite3
import struct
from dataclasses import dataclass, field
from datetime import datetime, timezone

from locus.agent import state

# Per-FACET cap. nomic truncates around 2k tokens, and this is a ceiling on one slice, not on the
# whole project — the project's total representation is now the sum of its facets.
_MAX_CHARS = 4000
# Section summaries embedded per project, longest first. A repo's short summaries are its
# `__init__.py` and setup boilerplate; embedding those buys noise, not coverage.
MAX_SECTION_FACETS = 30
MIN_SECTION_CHARS = 120
# Cap on the method/concept vocabulary bag (regime-ml alone names 263).
MAX_CONCEPT_NAMES = 120
# An open thread shorter than this is a stub, not a problem statement.
MIN_THREAD_CHARS = 25


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
    # Which slice of the subject this row holds: 'synthesis' | 'section:<n>' | 'concepts'.
    # One subject has many facets and a candidate matches on its BEST one (migration 0019).
    facet: str = "synthesis"


def _clean(text: str, limit: int = _MAX_CHARS) -> str:
    return " ".join((text or "").split())[:limit]


def _project_facets(
    conn: sqlite3.Connection, object_id: int, title: str, prefixes: tuple[str, ...] = (),
    *, max_sections: int = MAX_SECTION_FACETS,
) -> tuple[list[tuple[str, str]], list[int]]:
    """`[(facet, text), ...]` for one project, plus the documents they came from.

    THREE KINDS OF FACET, because a project is not one topic and one vector cannot say what it is:

      `synthesis`   the whole-project pitch — title, thesis, method, result, limitations. This is
                    what the profile used to be, except it was missing `result` and `limitations`
                    entirely, which is where a write-up says what actually HAPPENED.
      `section:<n>` one per section summary. This is the bulk of the signal and none of it was
                    being used: `regime-ml` has 88 section summaries totalling 34,385 characters
                    against the 289 the old profile embedded. A section describing how state
                    persistence is tuned can match a paper on sticky HMM priors; the project-level
                    pitch ("regime-conditioned equity ML trading") never will.
      `concepts`    the method and concept entities the project's documents name, as one bag. A
                    plain vocabulary list, which is often the most direct route to a method paper.

    Sections are ranked by summary length and capped, because a repo's short summaries are its
    `__init__.py` and its setup boilerplate, and embedding those buys nothing but noise.
    """
    from locus.learn.gaps import _doc_ids_for_object

    doc_ids = keep_doc_ids(conn, _doc_ids_for_object(conn, object_id), prefixes)
    if not doc_ids:
        return [], []
    marks = ",".join("?" * len(doc_ids))

    parts = [title]
    for r in conn.execute(
        f"SELECT title, thesis, method, result, limitations FROM documents WHERE id IN ({marks})",
        doc_ids,
    ):
        parts += [
            p for p in (r["title"], r["thesis"], r["method"], r["result"], r["limitations"]) if p
        ]
    facets: list[tuple[str, str]] = [("synthesis", _clean(" ".join(parts)))]

    for i, r in enumerate(conn.execute(
        f"SELECT title, summary FROM sections WHERE doc_id IN ({marks}) "
        f"AND summary IS NOT NULL AND LENGTH(summary) >= ? "
        f"ORDER BY LENGTH(summary) DESC LIMIT ?",
        (*doc_ids, MIN_SECTION_CHARS, max_sections),
    )):
        facets.append((f"section:{i}", _clean(f"{title}. {r['title'] or ''} {r['summary']}")))

    names = [
        r["name"] for r in conn.execute(
            f"SELECT DISTINCT name FROM entities WHERE doc_id IN ({marks}) "
            f"AND type IN ('method','concept') ORDER BY name LIMIT ?",
            (*doc_ids, MAX_CONCEPT_NAMES),
        )
    ]
    if len(names) >= 5:
        facets.append(("concepts", _clean(f"{title}. Methods and concepts: " + ", ".join(names))))

    facets.extend(_open_problem_facets(conn, object_id, title))
    return facets, doc_ids


def _open_problem_facets(
    conn: sqlite3.Connection, object_id: int, title: str
) -> list[tuple[str, str]]:
    """Facets built from what the project has NOT solved — its open threads and learnings.

    THE MOST DISCRIMINATIVE QUERY A PROJECT HAS, and it was going unused. Everything else in a
    profile describes what the project IS, and describing what something is retrieves more
    descriptions of the same thing. An OPEN PROBLEM describes what he needs, which is what a
    method paper supplies:

        "Whether rebalancing heuristic is overfit to training data"
        "Out-of-sample persistence of cascade mean-reversion patterns"
        "Generalize arbitrage to multiple stock pairs simultaneously"

    Each of those is a better query for finding transferable work than "a system that detects
    market regimes" will ever be, and it is the shape of the thing the owner asked this engine to
    do — surface a paper whose METHOD applies to a problem he actually has.

    `learnings` are included for the same reason at one remove: a finding states what did and did
    not work, which is a claim a paper can agree or disagree with.

    Each thread is its own facet rather than one concatenated blob, because a project's threads are
    unrelated to each other and averaging them produces a vector describing none of them.
    """
    row = conn.execute("SELECT body FROM objects WHERE id = ?", (object_id,)).fetchone()
    if not row:
        return []
    try:
        body = json.loads(row["body"] or "{}")
    except (TypeError, ValueError):
        return []

    out: list[tuple[str, str]] = []
    for i, thread in enumerate(body.get("open_threads") or []):
        text = str(thread).strip()
        if len(text) >= MIN_THREAD_CHARS:
            # The title gives the bare thread its domain — "expand market making to exploit idle
            # periods" is ambiguous without knowing it belongs to a trading project.
            out.append((f"thread:{i}", _clean(f"{title}. Open problem: {text}")))

    learnings = [str(x).strip() for x in (body.get("learnings") or []) if str(x).strip()]
    if learnings:
        out.append(("learnings", _clean(f"{title}. Findings: " + " ".join(learnings))))

    approach = " ".join(
        str(body.get(k) or "").strip() for k in ("approach", "why")
    ).strip()
    if len(approach) >= MIN_THREAD_CHARS:
        out.append(("approach", _clean(f"{title}. {approach}")))
    return out


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
        facets, doc_ids = _project_facets(conn, obj.id, obj.title, excluded_uris)
        # Not grounded in anything real, just a title restated.
        if not facets or len(facets[0][1]) <= len(obj.title) + 20:
            continue
        for facet, text in facets:
            out.append(Profile("project", str(obj.id), obj.title, text, doc_ids, facet))

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
                "INSERT INTO discovery_profiles (subject_kind, subject_key, facet, label, "
                "text, doc_ids, built_at) VALUES (?,?,?,?,?,?,?)",
                (prof.subject_kind, prof.subject_key, prof.facet, prof.label, prof.text,
                 json.dumps(prof.doc_ids), _utcnow()),
            )
            conn.execute(
                "INSERT INTO discovery_profile_vectors (profile_id, embedding) VALUES (?,?)",
                (cur.lastrowid, vec_blob(vec)),
            )
    return profiles
