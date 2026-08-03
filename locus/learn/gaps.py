"""Where the owner's grasp is thin (agent-layer plan §3.6, §8.4, §12.3).

This answers the interview-prep question directly: *"what am I lacking to explain the tanker-flow
project?"* — and it answers it deterministically, from the corpus's own shape, with NO model call.
That is the point. A model asked "what don't you understand?" will produce plausible-sounding
gaps; the corpus knows the real ones, because it records what the owner has written about and
what he has only ever used.

Three signals, in decreasing order of how much they mean:

  1. **EXPLANATION GAP (the strong one).** A concept the project's own documents implement, that
     the owner has NEVER written about in his own words — no owner-authored note mentioning it, no
     recorded belief position. He built with it; he has never had to explain it. That is exactly
     what an interview exposes, and it is invisible to every other view of the corpus.
  2. **THIN COVERAGE.** A concept carried by very few propositions corpus-wide: even the material
     he does have is shallow.
  3. **INGEST-FLAGGED GAPS.** `documents.gap_flags` — what the ingest pass itself flagged as
     missing/unclear in the source, filtered to real knowledge gaps (audit-trail lines excluded).

Signals 1 and 2 are joins; signal 3 is a stored field. Nothing here is inferred, so every gap
points at a specific concept and the specific documents that produced it (invariant 3).
"""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass, field

from locus.agent import state
from locus.agent.state import entity_key, parse_entity_key
from locus.eval.metrics import semantic_gaps

log = logging.getLogger(__name__)

# Concepts under this many corpus-wide propositions read as thin coverage (signal 2). Low on
# purpose: this is a prompt to go deeper, not an accusation, and a noisy version gets ignored.
_THIN_PROPOSITION_COUNT = 3


@dataclass
class Gap:
    """One place the owner's grasp is thin, with the evidence that says so."""

    kind: str  # 'explanation' | 'thin_coverage' | 'flagged'
    subject: str  # the concept (or, for 'flagged', the document)
    detail: str
    sources: list[str] = field(default_factory=list)  # document titles that evidence it

    def render(self) -> str:
        where = f" ({', '.join(self.sources)})" if self.sources else ""
        return f"[{self.kind}] {self.subject}: {self.detail}{where}"


# --- building blocks ---------------------------------------------------------------------------


def _canonical_of(conn, name: str, type_: str) -> tuple[str, str]:
    row = conn.execute(
        "SELECT canonical_name, canonical_type FROM entity_aliases "
        "WHERE variant_name=? AND variant_type=?",
        (name, type_),
    ).fetchone()
    return (row["canonical_name"], row["canonical_type"]) if row else (name, type_)


def _doc_ids_for_object(conn, object_id: int) -> list[int]:
    """The documents an object is grounded in (its `doc` links, resolved through source_uri)."""
    uris = [
        link.target_key for link in state.links_for(conn, object_id) if link.target_kind == "doc"
    ]
    if not uris:
        return []
    placeholders = ",".join("?" * len(uris))
    return [
        r["id"]
        for r in conn.execute(
            f"SELECT id FROM documents WHERE source_uri IN ({placeholders})", uris
        )
    ]


def _excluded_names(conn) -> set[str]:
    """Lowercase canonical names that are not CONCEPTS — shared with the proposer's gate 1b/1c.

    Without this, a code repo's gaps are its AST identifiers: the first live run on tanker-flow
    reported `upgrade`, `run_migrations_offline`, `main` and `gpkg_to_wkb` as things the owner
    "has never written about", burying the one real finding (Kalman filter) under Alembic
    boilerplate. A gap list nobody can act on is worse than no gap list."""
    names: set[str] = set()
    try:
        from locus.link.related import non_topical_names

        names |= non_topical_names(conn)
    except Exception as exc:  # a missing alias substrate must not block gap detection
        log.debug("gaps: generic-name filter unavailable: %s", exc)
    return names


def concepts_of_documents(
    conn, doc_ids: list[int], *, exclude_types: list[str] | None = None
) -> dict[tuple[str, str], list[int]]:
    """canonical (name,type) -> the doc ids among `doc_ids` that carry it.

    Filtered exactly as the proposer filters concepts (generic identifiers out by name, and
    non-idea entity KINDS out by type), so "a concept you have not explained" means the same
    thing in both surfaces."""
    if not doc_ids:
        return {}
    excluded_names = _excluded_names(conn)
    excluded_types = {t.casefold() for t in (exclude_types or [])}
    placeholders = ",".join("?" * len(doc_ids))
    out: dict[tuple[str, str], list[int]] = {}
    for r in conn.execute(
        f"SELECT DISTINCT doc_id, name, type FROM entities WHERE doc_id IN ({placeholders})",
        doc_ids,
    ):
        key = _canonical_of(conn, r["name"], r["type"])
        if key[0].lower() in excluded_names or (key[1] or "").casefold() in excluded_types:
            continue
        out.setdefault(key, [])
        if r["doc_id"] not in out[key]:
            out[key].append(r["doc_id"])
    return out


def _owner_authored_mentions(
    conn, name: str, type_: str, categories: list[str], exclude_doc_ids: tuple[int, ...] = ()
) -> list[str]:
    """Titles of OWNER-AUTHORED documents mentioning this concept — the explanation evidence.

    Matches through the alias substrate (any surface of the canonical counts), so writing about
    "regime detection" covers a project that names it "regime-detection".

    `exclude_doc_ids` is the object's OWN documents, and excluding them is what keeps the signal
    alive. The gap asks "does his project use a concept he has never explained?" — the project
    document is the thing doing the USING, so counting it as evidence of explaining makes the
    question answer itself. That went unnoticed while `belief_source_categories` was `note` alone
    (a repo was never owner-authored); the moment project write-ups became his, which is correct,
    every project concept would have been trivially "explained" by the project itself.
    """
    if not categories:
        return []
    placeholders = ",".join("?" * len(categories))
    excluded = ",".join("?" * len(exclude_doc_ids)) if exclude_doc_ids else ""
    not_own = f"AND d.id NOT IN ({excluded})" if exclude_doc_ids else ""
    rows = conn.execute(
        f"""
        SELECT DISTINCT d.title
        FROM entities e
        JOIN documents d ON d.id = e.doc_id
        WHERE d.category IN ({placeholders})
          {not_own}
          AND COALESCE(
                (SELECT a.canonical_name FROM entity_aliases a
                  WHERE a.variant_name = e.name AND a.variant_type = e.type), e.name
              ) = ?
        """,
        (*categories, *exclude_doc_ids, name),
    ).fetchall()
    return [r["title"] for r in rows]


def _proposition_count(conn, name: str) -> int:
    """Corpus-wide propositions in sections that name this canonical concept (signal 2)."""
    row = conn.execute(
        """
        SELECT COUNT(*) AS n FROM propositions p
        WHERE p.section_id IN (
            SELECT e.section_id FROM entities e
            WHERE COALESCE(
                    (SELECT a.canonical_name FROM entity_aliases a
                      WHERE a.variant_name = e.name AND a.variant_type = e.type), e.name
                  ) = ?
        )
        """,
        (name,),
    ).fetchone()
    return int(row["n"] or 0)


def _flagged_gaps(conn, doc_ids: list[int]) -> list[Gap]:
    if not doc_ids:
        return []
    placeholders = ",".join("?" * len(doc_ids))
    out: list[Gap] = []
    for r in conn.execute(
        f"SELECT title, gap_flags FROM documents WHERE id IN ({placeholders})", doc_ids
    ):
        for g in semantic_gaps(json.loads(r["gap_flags"] or "[]")):
            out.append(Gap(kind="flagged", subject=r["title"], detail=g, sources=[r["title"]]))
    return out


# --- the entry points ---------------------------------------------------------------------------


def gaps_for_object(
    conn, object_id: int, *, owner_categories: list[str] | None = None, limit: int = 10
) -> list[Gap]:
    """Gaps for one project/concept object, strongest signal first.

    `owner_categories` are the categories counting as "written in his own words" — the same
    owner-authored notion the belief-position gate uses, defaulting to `[structure]`'s setting so
    the two cannot drift apart."""
    if owner_categories is None:
        from locus.config import load

        owner_categories = load().structure.belief_source_categories

    obj = state.get_object(conn, object_id)
    if obj is None:
        return []
    doc_ids = _doc_ids_for_object(conn, object_id)
    explanation: list[Gap] = []
    thin: list[Gap] = []

    from locus.config import load as _load

    exclude_types = _load().structure.concept_exclude_entity_types
    for (name, type_), carriers in concepts_of_documents(
        conn, doc_ids, exclude_types=exclude_types
    ).items():
        titles = _titles(conn, carriers)
        mentions = _owner_authored_mentions(
            conn, name, type_, owner_categories, exclude_doc_ids=tuple(doc_ids)
        )
        has_position = bool(state.positions_for(conn, "concept", entity_key(name, type_)))
        if not mentions and not has_position:
            explanation.append(Gap(
                kind="explanation", subject=name,
                detail="used in your work but never written about in your own words",
                sources=titles,
            ))
        elif _proposition_count(conn, name) < _THIN_PROPOSITION_COUNT:
            thin.append(Gap(
                kind="thin_coverage", subject=name,
                detail="the corpus carries very little detail on this",
                sources=titles,
            ))

    ordered = explanation + thin + _flagged_gaps(conn, doc_ids)
    return ordered[:limit]


def gaps_for_concept(conn, name: str, type_: str = "concept", *, limit: int = 5) -> list[Gap]:
    """Gaps for a bare concept (no object needed) — the synthesise surface's "what's thin here"."""
    from locus.config import load

    categories = load().structure.belief_source_categories
    canonical, ctype = _canonical_of(conn, name, type_)
    out: list[Gap] = []
    mentions = _owner_authored_mentions(conn, canonical, ctype, categories)
    if not mentions and not state.positions_for(conn, "concept", entity_key(canonical, ctype)):
        out.append(Gap(kind="explanation", subject=canonical,
                       detail="you have never written about this in your own words"))
    if _proposition_count(conn, canonical) < _THIN_PROPOSITION_COUNT:
        out.append(Gap(kind="thin_coverage", subject=canonical,
                       detail="the corpus carries very little detail on this"))
    return out[:limit]


def gaps_for_concept_key(conn, subject_key: str, *, limit: int = 5) -> list[Gap]:
    """`gaps_for_concept` addressed by a canonical entity key (what object_links store)."""
    name, type_ = parse_entity_key(subject_key)
    return gaps_for_concept(conn, name, type_, limit=limit)


def _titles(conn, doc_ids: list[int]) -> list[str]:
    if not doc_ids:
        return []
    placeholders = ",".join("?" * len(doc_ids))
    return [
        r["title"]
        for r in conn.execute(
            f"SELECT title FROM documents WHERE id IN ({placeholders}) ORDER BY id", doc_ids
        )
    ]
