"""Decide what to SEARCH the literature for — the concepts he reads and builds with.

Browsing recent listings by category, which is what harvesting did first, caps the pool at whatever
happened to be posted in the last few weeks. Measured 2026-07-31: a targeted search for
`"regime switching" AND "hidden Markov"` returns work from 2008, 2014 and 2020 — including
*Regime Switching Bandits*, squarely relevant to his regime project and unreachable by any amount
of reranking, because it was never harvested. **Methods are old.** A discovery engine that only
sees new preprints cannot find the canonical treatment of a technique, which is usually the thing
worth reading.

THREE SOURCES, and the first is the one that was missing entirely:

  `reading`  concepts from documents he has ANNOTATED, and from his notes on them. He marked
             these passages by hand; nothing else in the system states his interests as directly.
             `Betting Against Beta`, `Beta compression`, `Crowding`, `Alternative Data` all come
             from one book he read last week.
  `project`  method entities from the documents behind his active projects — what he builds with.
  `gap`      concepts his blessed work uses but never explains.

WHAT GOES OUT, and why it is shaped this way. Queries are short technical PHRASES, never his prose.
That is not a privacy compromise, it is what actually works: search APIs degrade badly on long
text, and his own names for things (`tanker-flow`, `Optibook`) return nothing anywhere because
they are not published terminology. The best query and the least revealing query are the same
string, so there is no quality being traded away.
"""

from __future__ import annotations

import re
import sqlite3
from dataclasses import dataclass

# A term shorter than this, and not multi-word, is too broad to search: `Beta`, `Alpha`, `Country`
# are real entities on his reading but they retrieve the entire field.
_MIN_SINGLE_WORD = 8
_MAX_TERM = 60
# Book-internal and code-internal shapes that name a LOCATION rather than a concept. Measured on
# the first term derivation: `Procedure 6.2 Sizing alphas into positions`, `IS/OOS split at
# 2019-01-01` and `if-elif-else statement` are all real entities and all useless as searches —
# they are how one document refers to itself, not published terminology anyone else uses.
_INTERNAL = re.compile(
    r"(^|\s)(procedure|chapter|section|table|figure|appendix|step|phase)\s*\d"
    r"|[=<>{}]|\bif-|\bself\b|\d{4}-\d{2}-\d{2}",
    re.IGNORECASE,
)
# Terms per project, so one project cannot spend the whole search budget. The first derivation had
# tanker-flow supplying all 24 project terms because it happened to be iterated first.
_PER_PROJECT = 4


@dataclass(frozen=True)
class SearchTerm:
    term: str
    source_kind: str    # 'reading' | 'project' | 'gap'
    source_label: str   # the document title / project / gap that produced it

    @property
    def evidence_key(self) -> str:
        return f"{self.source_kind}:{self.source_label}"


def _usable(term: str, excluded: set[str]) -> bool:
    """Is this entity name worth spending a search on?"""
    t = (term or "").strip()
    if not (4 <= len(t) <= _MAX_TERM) or t.lower() in excluded:
        return False
    if any(tok.isdigit() for tok in t.split()):
        return False                        # a numeric token means an instrument or a venue:
                                            # `CAC 40`, `12 Mo`, `55 liquid futures`
    if "_" in t or re.search(r"[a-z][A-Z]", t):
        return False                        # a code identifier the AST pass typed as a method:
                                            # `get_positions`, `delete_order`, `runMigrations`
    if len(t) <= 6 and t.isupper() and " " not in t:
        return False                        # bare acronym — embeds and searches as noise
    if _INTERNAL.search(t):
        return False                        # names a place in a document, not a concept
    # Single words must be long enough to be specific: `Crowding` yes, `Country` no.
    return " " in t or len(t) >= _MIN_SINGLE_WORD


def reading_terms(conn: sqlite3.Connection, *, limit: int = 40) -> list[SearchTerm]:
    """Concepts from documents he has ANNOTATED, plus his notes about them.

    This is the source the engine was missing, and the one he asked for by name: the concepts he
    has come across READING, as opposed to the ones his code implements. It resolves through
    `reading_targets`, which is why that mapping had to exist (migration 0017) — the annotations
    are keyed by device path and the documents by filesystem path.
    """
    rows = conn.execute(
        """
        SELECT DISTINCT e.name AS name, d.title AS title
        FROM pdf_annotations a
        JOIN reading_targets t
          ON (a.doc_uuid IS NOT NULL AND a.doc_uuid = t.doc_uuid)
          OR (a.doc_uuid IS NULL AND a.source_uri = t.device_path)
        JOIN documents d ON d.source_uri = t.source_uri
        JOIN entities  e ON e.doc_id = d.id
        WHERE e.type IN ('method', 'concept', 'metric')
        ORDER BY LENGTH(e.name) DESC
        """
    ).fetchall()
    excluded = _excluded_names(conn)
    out: list[SearchTerm] = []
    for r in rows:
        if _usable(r["name"], excluded):
            out.append(SearchTerm(r["name"].strip(), "reading", r["title"] or "your reading"))
        if len(out) >= limit:
            break
    return out


def project_terms(conn: sqlite3.Connection, *, limit: int = 30) -> list[SearchTerm]:
    """Method entities from the documents behind his active projects — what he builds with."""
    from locus.agent import state
    from locus.learn.gaps import _doc_ids_for_object

    excluded = _excluded_names(conn)
    out: list[SearchTerm] = []
    for obj in state.list_objects(conn, type_="project", status="active", limit=100):
        doc_ids = _doc_ids_for_object(conn, obj.id)
        if not doc_ids:
            continue
        marks = ",".join("?" * len(doc_ids))
        # Rank by how many documents in the whole corpus name the term, NOT by length. Length
        # selects the most idiosyncratic phrase a summary happens to contain; cross-document
        # frequency selects terminology that is actually shared, which is what a search engine
        # can match. `walk-forward cross-validation` beats `IS/OOS split at 2019-01-01`.
        taken = 0
        for r in conn.execute(
            f"""SELECT e.name AS name, COUNT(DISTINCT e2.doc_id) AS df
                FROM entities e
                JOIN entities e2 ON e2.name = e.name
                WHERE e.doc_id IN ({marks}) AND e.type = 'method'
                GROUP BY e.name ORDER BY df DESC, LENGTH(e.name) ASC LIMIT 40""",
            doc_ids,
        ):
            if taken >= _PER_PROJECT:
                break
            if _usable(r["name"], excluded):
                out.append(SearchTerm(r["name"].strip(), "project", obj.title))
                taken += 1
    return out[:limit]


def gap_terms(conn: sqlite3.Connection, *, limit: int = 20) -> list[SearchTerm]:
    """Concepts his blessed work uses but has never written up."""
    from locus.learn.reread import open_gap_concepts

    excluded = _excluded_names(conn)
    gaps = open_gap_concepts(conn)
    if not gaps:
        return []
    # A gap named in only ONE document is usually an instrument, a venue or a one-off phrase
    # (`55 liquid futures`, `CAC 40`, `European TTF`, `Exchange object`). Requiring it to be
    # attested across documents is the cheap half of the filter tier the plan defers, and it is
    # the difference between searching for a concept and searching for a ticker.
    marks = ",".join("?" * len(gaps))
    shared = {
        r["name"] for r in conn.execute(
            f"SELECT name, COUNT(DISTINCT doc_id) df FROM entities WHERE name IN ({marks}) "
            f"GROUP BY name HAVING df >= 2", tuple(gaps)
        )
    }
    out: list[SearchTerm] = []
    for concept, owners in sorted(gaps.items()):
        if concept in shared and _usable(concept, excluded):
            out.append(SearchTerm(concept, "gap", sorted(owners)[0] if owners else "your work"))
        if len(out) >= limit:
            break
    return out


def _excluded_names(conn: sqlite3.Connection) -> set[str]:
    """Code boilerplate and non-topical surfaces — the predicate the other layers already share."""
    try:
        from locus.link.related import non_topical_names

        return {n.lower() for n in non_topical_names(conn)}
    except Exception:                       # pragma: no cover - defensive only
        return set()


def all_terms(conn: sqlite3.Connection, *, per_source: int = 25) -> list[SearchTerm]:
    """Every search term worth spending a request on, de-duplicated, reading first.

    Reading leads deliberately: a concept he underlined by hand is a stronger statement of interest
    than one his code happens to name, and the ordering decides what gets searched when the request
    budget runs out.
    """
    seen: set[str] = set()
    out: list[SearchTerm] = []
    for group in (
        reading_terms(conn, limit=per_source),
        project_terms(conn, limit=per_source),
        gap_terms(conn, limit=per_source),
    ):
        for t in group:
            key = t.term.casefold()
            if key not in seen:
                seen.add(key)
                out.append(t)
    return out
