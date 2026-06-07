"""Related documents via shared canonical entities — joins-only, no inference (step 12).

The cross-doc edges deferred from step 7.5: with the alias substrate built (`locus link`),
two documents are related when their entities map to the same canonical `(name, type)`.
Consumed by `locus inspect` and the MCP `inspect_document` tool; the same joins will feed
the Obsidian projection (§14) post-pour.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass

# How many shared canonical names to sample per related document (display context).
_SAMPLE_NAMES = 5


@dataclass(frozen=True)
class RelatedDoc:
    doc_id: int
    title: str
    shared_count: int
    shared_names: tuple[str, ...]  # up to _SAMPLE_NAMES canonical names, most distinctive first


def aliases_built(conn: sqlite3.Connection) -> bool:
    """True when the entity_aliases substrate exists and is populated (post `locus link`)."""
    row = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='entity_aliases'"
    ).fetchone()
    if row is None:
        return False
    return conn.execute("SELECT 1 FROM entity_aliases LIMIT 1").fetchone() is not None


def format_related(
    conn: sqlite3.Connection,
    doc_id: int,
    *,
    top_n: int = 5,
    stop_doc_freq: int | None = None,
) -> list[str]:
    """Render the RELATED DOCUMENTS block (shared by `locus inspect` and MCP inspect)."""
    if not aliases_built(conn):
        return ["RELATED DOCUMENTS: (run `locus link` to build the alias substrate)"]
    related = related_documents(conn, doc_id, top_n=top_n, stop_doc_freq=stop_doc_freq)
    if not related:
        return ["RELATED DOCUMENTS: (none — no shared entities with other documents)"]
    lines = [f"RELATED DOCUMENTS (shared canonical entities, top {len(related)}):"]
    for r in related:
        names = ", ".join(r.shared_names)
        lines.append(f"  [{r.doc_id}] {r.title}  ({r.shared_count} shared: {names})")
    return lines


def related_documents(
    conn: sqlite3.Connection,
    doc_id: int,
    *,
    top_n: int = 5,
    stop_doc_freq: int | None = None,
) -> list[RelatedDoc]:
    """Top documents sharing the most canonical entities with `doc_id`.

    `stop_doc_freq`: exclude canonicals appearing in more than this many documents —
    stop-entities ("model", "system") link everything and say nothing. Off (None) at
    small corpus scale; the pour runbook enables it (~0.4 x doc count) post-pour.
    Returns [] when the alias substrate has not been built yet.
    """
    stop_clause = ""
    params: list = [doc_id, doc_id]  # my.doc_id, cd.doc_id != — SQL text order
    if stop_doc_freq is not None:
        stop_clause = (
            " AND (cd.canonical_name, cd.canonical_type) NOT IN ("
            "   SELECT a2.canonical_name, a2.canonical_type"
            "   FROM entities e2 JOIN entity_aliases a2"
            "     ON a2.variant_name = e2.name AND a2.variant_type = e2.type"
            "   GROUP BY a2.canonical_name, a2.canonical_type"
            "   HAVING COUNT(DISTINCT e2.doc_id) > ?)"
        )
        params.append(stop_doc_freq)

    rows = conn.execute(
        f"""
        WITH canon_docs AS (
            SELECT DISTINCT a.canonical_name, a.canonical_type, e.doc_id
            FROM entities e
            JOIN entity_aliases a
              ON a.variant_name = e.name AND a.variant_type = e.type
        ),
        my AS (SELECT canonical_name, canonical_type FROM canon_docs WHERE doc_id = ?)
        SELECT cd.doc_id, d.title, COUNT(*) AS shared
        FROM canon_docs cd
        JOIN my ON my.canonical_name = cd.canonical_name
              AND my.canonical_type = cd.canonical_type
        JOIN documents d ON d.id = cd.doc_id
        WHERE cd.doc_id != ?{stop_clause}
        GROUP BY cd.doc_id
        ORDER BY shared DESC, cd.doc_id
        LIMIT {int(top_n)}
        """,
        params,
    ).fetchall()

    out: list[RelatedDoc] = []
    for r in rows:
        # Same stop-entity filter as the ranking query: a stop-entity must neither count
        # nor show up in the displayed sample. Most distinctive (lowest spread) first.
        name_params: list = [doc_id, r["doc_id"]]
        spread_clause = ""
        if stop_doc_freq is not None:
            spread_clause = " AND spread <= ?"
            name_params.append(stop_doc_freq)
        name_params.append(_SAMPLE_NAMES)
        names = [
            n["canonical_name"]
            for n in conn.execute(
                f"""
                WITH canon_docs AS (
                    SELECT DISTINCT a.canonical_name, a.canonical_type, e.doc_id
                    FROM entities e
                    JOIN entity_aliases a
                      ON a.variant_name = e.name AND a.variant_type = e.type
                ),
                shared AS (
                    SELECT c1.canonical_name, c1.canonical_type,
                           (SELECT COUNT(DISTINCT c3.doc_id) FROM canon_docs c3
                            WHERE c3.canonical_name = c1.canonical_name
                              AND c3.canonical_type = c1.canonical_type) AS spread
                    FROM canon_docs c1
                    JOIN canon_docs c2
                      ON c2.canonical_name = c1.canonical_name
                     AND c2.canonical_type = c1.canonical_type
                    WHERE c1.doc_id = ? AND c2.doc_id = ?
                )
                SELECT canonical_name FROM shared
                WHERE 1=1{spread_clause}
                ORDER BY spread ASC, canonical_name
                LIMIT ?
                """,
                name_params,
            )
        ]
        out.append(RelatedDoc(r["doc_id"], r["title"] or "(untitled)", r["shared"], tuple(names)))
    return out
