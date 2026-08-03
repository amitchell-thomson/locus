"""Re-read ranking: which document already in the corpus would close the most open gaps.

The daily page's read-next slot was structurally empty. It read blessed `reading` objects, the
structurer has proposed none, and a `reading` proposal must anchor to a document already in the
corpus anyway — so at best it could only ever have said "re-read this", never "go find that".

This module supplies the first of those honestly and deterministically. No model, no proposal:
a gap is "a concept your blessed work uses that you have never written about in your own words"
(learn/gaps), and a re-read candidate is simply the corpus document that covers the most of them.

Two exclusions, both deliberate:

  - the owner's OWN writing (`note`) is never suggested. A gap exists precisely because he has
    not written about the concept; offering his own notes back would be circular, and re-reading
    what you wrote is not how you close a gap in understanding;
  - a document is never offered as the answer to a gap its OWN object raised — otherwise the
    page tells him to re-read the very write-up that failed to explain the concept. It may still
    answer a DIFFERENT object's gap, which is why the rule is per-gap rather than a blanket
    exclusion of everything his blessed work touches.

**Deliberately NOT a discovery engine.** See `docs/reading-discovery-plan.md` — the far more
valuable version proposes material the corpus does NOT yet contain, pushes it to a reMarkable
reading folder, and ingests it when the owner moves it to in-progress/finished. That needs an
outside-the-corpus source and its own transport; this module is the part that can be built
truthfully today, and is written so the daily page's read-next slot has the same shape either
way.
"""

from __future__ import annotations

import math
import sqlite3
from dataclasses import dataclass

from locus.agent import state
from locus.learn.gaps import gaps_for_object


@dataclass
class ReReadCandidate:
    doc_id: int
    source_uri: str
    title: str
    concepts: tuple[str, ...]   # the open gap concepts this document covers, RAREST first
    objects: tuple[str, ...]    # the blessed objects those gaps belong to

    @property
    def reason(self) -> str:
        head = ", ".join(self.concepts[:2])
        more = f" (+{len(self.concepts) - 2} more)" if len(self.concepts) > 2 else ""
        return f"covers {head}{more} — used in your work, never written up"


def _concept_doc_freq(conn: sqlite3.Connection, concepts) -> dict[str, int]:
    """How many documents each canonical concept appears in."""
    if not concepts:
        return {}
    marks = ",".join("?" * len(concepts))
    rows = conn.execute(
        f"""
        SELECT canonical_name AS c, COUNT(DISTINCT doc_id) AS n FROM (
            SELECT e.doc_id AS doc_id,
                   COALESCE(
                     (SELECT a.canonical_name FROM entity_aliases a
                       WHERE a.variant_name = e.name AND a.variant_type = e.type),
                     e.name
                   ) AS canonical_name
            FROM entities e
        ) WHERE canonical_name IN ({marks}) GROUP BY canonical_name
        """,
        tuple(concepts),
    ).fetchall()
    return {r["c"]: r["n"] for r in rows}


def concept_weight(doc_freq: int) -> float:
    """How much covering one gap concept should count towards reading a document.

    RARITY IS THE SIGNAL. Ranking on a raw count of gaps closed handed the read-next slot to
    coursework and kept it there: measured 2026-07-30, the open gaps included `frequency
    response` (18 documents) and `eigenvector` (13), which every signals and control handout
    mentions, while the gaps that actually matter to his work — `market regime detection`,
    `walk-forward cross-validation` — sat at ONE document each. Two generic hits beat one
    decisive one, so the page offered Nyquist plots and Mechanical Vibration.

    A concept spanning eighteen documents tells you almost nothing about which of them to
    read; a concept in one document means that document IS the place. This weighting says so,
    and it is not a thumb on the scale for quant: if he later adds fifty papers that all
    discuss regimes, `regime` becomes generic and stops driving the ranking, exactly as it
    should. Category-based down-weighting would have hard-coded today's corpus instead.
    """
    return 1.0 / math.log2(1 + max(doc_freq, 1))


def open_gap_concepts(conn: sqlite3.Connection, *, limit: int = 100) -> dict[str, set[str]]:
    """gap concept -> the blessed objects that use it but have never explained it."""
    out: dict[str, set[str]] = {}
    for obj in state.list_objects(conn, status="active", limit=limit):
        for gap in gaps_for_object(conn, obj.id, limit=5):
            if gap.kind == "flagged":
                continue  # a doc-level quality flag, not a concept he owes himself
            out.setdefault(gap.subject, set()).add(obj.title)
    return out


def _docs_by_object(conn: sqlite3.Connection, *, limit: int = 100) -> dict[str, set[int]]:
    """object title -> the documents it is grounded in.

    Used to drop the circular case only: a document must not be offered as the answer to a gap
    that its OWN object raised. Blanket-excluding every document linked to any blessed object
    was tried first and was too blunt — it removed the Optibook reference, which is genuinely
    the best place to read up on `Exchange object`, purely because a concept object also points
    at it. Nothing in the schema distinguishes "reference material I read" from "my own
    write-up" (both link via `implements`, both sit in `category='project'`), so the narrow
    per-gap rule is the only one that is actually true.
    """
    from locus.learn.gaps import _doc_ids_for_object

    out: dict[str, set[int]] = {}
    for obj in state.list_objects(conn, status="active", limit=limit):
        out[obj.title] = set(_doc_ids_for_object(conn, obj.id))
    return out


def reread_candidates(
    conn: sqlite3.Connection, *, limit: int = 3, gap_limit: int = 100
) -> list[ReReadCandidate]:
    """Corpus documents ranked by how many open gap concepts each would close.

    Alias-aware: a document counts as covering a concept when any of its entity surfaces
    resolves through `entity_aliases` to that canonical, so "MPT" and "Modern Portfolio Theory"
    are the same gap rather than two.
    """
    gaps = open_gap_concepts(conn, limit=gap_limit)
    if not gaps:
        return []

    from locus.agent.state import owner_authored_sql

    marks = ",".join("?" * len(gaps))
    # "His own writing" by the one shared definition, so a note filed `coursework` by its device
    # folder is still recognised as his and never suggested back to him as reading.
    own_clause, own_params = owner_authored_sql("d")
    rows = conn.execute(
        f"""
        SELECT d.id, d.source_uri, d.title, canon.canonical_name AS concept
        FROM entities e
        JOIN documents d ON d.id = e.doc_id
        JOIN (
            SELECT e2.doc_id AS doc_id,
                   COALESCE(
                     (SELECT a.canonical_name FROM entity_aliases a
                       WHERE a.variant_name = e2.name AND a.variant_type = e2.type),
                     e2.name
                   ) AS canonical_name
            FROM entities e2
        ) canon ON canon.doc_id = e.doc_id AND canon.canonical_name IN ({marks})
        WHERE NOT {own_clause}
        GROUP BY d.id, canon.canonical_name
        """,
        (*gaps.keys(), *own_params),
    ).fetchall()

    docs_by_object = _docs_by_object(conn, limit=gap_limit)
    by_doc: dict[int, dict] = {}
    for r in rows:
        # Count this concept only if some object that owes the gap is NOT this document's own —
        # otherwise the page tells him to read his own write-up to learn what it failed to
        # explain (see _docs_by_object).
        owners = gaps.get(r["concept"], set())
        if not any(r["id"] not in docs_by_object.get(o, set()) for o in owners):
            continue
        entry = by_doc.setdefault(
            r["id"],
            {"uri": r["source_uri"] or str(r["id"]), "title": r["title"] or "", "concepts": set()},
        )
        entry["concepts"].add(r["concept"])

    freq = _concept_doc_freq(conn, list(gaps))
    ranked = sorted(
        by_doc.items(),
        # Rarity-weighted, not a raw count (see concept_weight). Ties broken by doc id so the
        # page is stable day to day.
        key=lambda kv: (
            -sum(concept_weight(freq.get(c, 1)) for c in kv[1]["concepts"]),
            kv[0],
        ),
    )

    out: list[ReReadCandidate] = []
    for doc_id, entry in ranked[:limit]:
        # Rarest first, so the printed reason leads with the concept that actually singles this
        # document out rather than one every handout mentions.
        concepts = sorted(entry["concepts"], key=lambda c: (freq.get(c, 1), c))
        objects = sorted({o for c in concepts for o in gaps.get(c, ())})
        out.append(
            ReReadCandidate(
                doc_id=doc_id,
                source_uri=entry["uri"],
                title=entry["title"],
                concepts=tuple(concepts),
                objects=tuple(objects),
            )
        )
    return out
