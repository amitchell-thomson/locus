"""Ideas that connect to each other.

THE GAP THIS CLOSES. Ideas were born, linked to the document that provoked them and the project
they named, and then sat in one table not knowing about one another. Two ideas about regime
detection — one from a book margin in May, one jotted in a talk in July — were as unrelated as
any two rows. His words: "why is idea not one of the objects where ideas are linked threads that
connect together and are developed over time." Development was there; connection was not.

WHAT COUNTS AS A CONNECTION, and why it is not similarity. The obvious move is to embed every idea
and link the near neighbours, and it is the wrong one: cosine cannot tell "these are about the
same thing" from "these use the same words", and the corpus already contains a better answer.
`entity_aliases` is the system's own definition of when two surfaces are one concept, built by
`locus link` from deterministic tiers plus adjudicated clusters. So two ideas are connected when
they NAME THE SAME CANONICAL CONCEPT — a fact, checkable by reading both, rather than a distance.

Cheap by construction: an idea is a sentence or two, so matching canonical names against its text
is a scan over short strings, and the whole pass is a few hundred comparisons. No model, no
embeddings, no network — the same joins-only discipline as `link/related.py`.

THE LINK IS SYMMETRIC AND STORED BOTH WAYS, because `object_links` is directed and a thread should
surface from either end: reading an idea should show what else you have thought that touches it,
whichever came first.
"""

from __future__ import annotations

import re
import sqlite3
from collections import defaultdict
from dataclasses import dataclass

from locus.agent import state
from locus.link.related import non_topical_names

# Concepts shorter than this match too much to mean anything — `ML`, `PDE`, `VaR` will appear in
# half the ideas he has ever had, and a link that fires on everything is noise wearing a citation.
_MIN_CONCEPT_CHARS = 5

# An idea that names a great many concepts is usually a paragraph rather than an idea; capping
# keeps one sprawling thread from linking to everything.
_MAX_CONCEPTS_PER_IDEA = 8

RELATION = "relates"
THREAD_TYPES = ("idea", "question")


@dataclass
class ThreadLink:
    source_id: int
    target_id: int
    shared: tuple[str, ...]


def _idea_text(obj: state.AgentObject) -> str:
    """Everything of HIS that the thread carries: the title, the idea, and every development pass.

    Deliberately not the agent's `why`: linking two ideas because the proposer used the same word
    in its rationale twice would be a connection between two pieces of machine prose.
    """
    body = obj.body or {}
    parts = [obj.title, str(body.get("idea") or ""), str(body.get("question") or "")]
    for entry in body.get("development") or []:
        parts.append(str(entry.get("text", "")) if isinstance(entry, dict) else str(entry))
    return " ".join(p for p in parts if p)


def canonical_vocabulary(conn: sqlite3.Connection, *, min_docs: int = 2) -> dict[str, str]:
    """`casefolded canonical name -> canonical name`, for canonicals spanning >= min_docs docs.

    The >= 2 floor is what makes a shared concept meaningful: a name appearing in one document is
    that document's vocabulary, not a thread running through his work. Boilerplate and code
    symbols are removed by `non_topical_names`, the same predicate the related-docs and structure
    layers use — so all three agree on what a concept is.
    """
    try:
        rows = conn.execute(
            """
            SELECT cname, COUNT(DISTINCT doc_id) AS docs FROM (
                SELECT COALESCE(a.canonical_name, e.name) AS cname, e.doc_id AS doc_id
                FROM entities e
                LEFT JOIN entity_aliases a
                       ON a.variant_name = e.name AND a.variant_type = e.type
            )
            GROUP BY cname HAVING docs >= ?
            """,
            (min_docs,),
        ).fetchall()
    except sqlite3.OperationalError:
        return {}

    names = [r["cname"] for r in rows if r["cname"] and len(r["cname"]) >= _MIN_CONCEPT_CHARS]
    # `non_topical_names` takes the CONNECTION and returns the corpus-wide boilerplate set — it
    # is not a filter over a list. Same predicate the related-docs and structure layers use, so
    # all three agree on what counts as a concept.
    excluded = {n.casefold() for n in non_topical_names(conn)}
    return {n.casefold(): n for n in names if n.casefold() not in excluded}


def concepts_in(text: str, vocabulary: dict[str, str]) -> list[str]:
    """Canonical concepts this text names, longest first (so a phrase beats its own head word)."""
    haystack = f" {re.sub(r'[^a-z0-9]+', ' ', text.lower())} "
    found: list[str] = []
    for key in sorted(vocabulary, key=len, reverse=True):
        needle = f" {re.sub(r'[^a-z0-9]+', ' ', key)} "
        if needle in haystack:
            canonical = vocabulary[key]
            # A longer phrase already matched swallows its head word: "factor covariance" having
            # matched, "covariance" adds nothing and would inflate the shared count.
            if not any(canonical.casefold() in f.casefold() for f in found):
                found.append(canonical)
        if len(found) >= _MAX_CONCEPTS_PER_IDEA:
            break
    return found


def link_threads(
    conn: sqlite3.Connection, *, min_shared: int = 1, min_docs: int = 2, limit: int = 500
) -> list[ThreadLink]:
    """Connect every pair of threads that names the same canonical concept. Joins only.

    Idempotent: `add_links` is INSERT OR IGNORE, so re-running adds nothing and a thread he
    develops later simply gains links as its vocabulary grows.
    """
    vocabulary = canonical_vocabulary(conn, min_docs=min_docs)
    if not vocabulary:
        return []

    rows = conn.execute(
        f"SELECT id FROM objects WHERE status='active' AND type IN "
        f"({','.join('?' * len(THREAD_TYPES))}) ORDER BY id LIMIT ?",
        (*THREAD_TYPES, limit),
    ).fetchall()

    by_concept: dict[str, list[int]] = defaultdict(list)
    concepts_of: dict[int, list[str]] = {}
    for row in rows:
        obj = state.get_object(conn, row["id"])
        if obj is None:
            continue
        found = concepts_in(_idea_text(obj), vocabulary)
        concepts_of[obj.id] = found
        for concept in found:
            by_concept[concept].append(obj.id)

    shared_between: dict[tuple[int, int], set[str]] = defaultdict(set)
    for concept, ids in by_concept.items():
        for i, a in enumerate(ids):
            for b in ids[i + 1:]:
                shared_between[(a, b)].add(concept)

    out: list[ThreadLink] = []
    for (a, b), shared in sorted(shared_between.items()):
        if len(shared) < min_shared:
            continue
        names = tuple(sorted(shared))
        # Stored BOTH ways: `object_links` is directed, and a thread should surface from either
        # end regardless of which was written first.
        state.add_links(conn, a, [state.ObjectLink("object", str(b), RELATION)])
        state.add_links(conn, b, [state.ObjectLink("object", str(a), RELATION)])
        out.append(ThreadLink(a, b, names))
    return out


def related_threads(conn: sqlite3.Connection, object_id: int) -> list[state.AgentObject]:
    """Other threads linked to this one — what else he has thought that touches it."""
    out: list[state.AgentObject] = []
    for link in state.links_for(conn, object_id):
        if link.target_kind != "object" or not link.target_key.isdigit():
            continue
        other = state.get_object(conn, int(link.target_key))
        if other is not None and other.type in THREAD_TYPES:
            out.append(other)
    return out
