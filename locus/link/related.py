"""Related documents via shared canonical entities — joins-only, no inference (step 12).

The cross-doc edges deferred from step 7.5: with the alias substrate built (`locus link`),
two documents are related when their entities map to the same canonical entity. Consumed
by `locus inspect` and the MCP `inspect_document` tool; the same joins will feed the
Obsidian projection (§14) post-pour.

Round-5 audit hardening:
  - Sharing is counted at canonical NAME level, not (name, type): "LLM" stored under
    concept/method/tool is one shared term, not three ("4 shared: F1, LLM, LLM, LLM").
  - Ranking is inverse-doc-frequency weighted: a name shared by half the corpus ("F1",
    "LLM") says almost nothing about *this* pair, so each shared name contributes
    1/doc_freq rather than 1. Generic terms stop displacing genuine neighbours while
    still counting a little. Raw shared count is kept for display.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass

# How many shared canonical names to sample per related document (display context).
_SAMPLE_NAMES = 5

# Code-identifier noise in the RELATED pool (alias table + retrieval untouched). A "related
# document" should mean a shared TOPIC, but code repos were linking on scaffolding identifiers
# that every project of a given stack defines regardless of subject. Excluded in four classes:
#
#   - short bare tokens (round-6): short, all-lowercase, single-token ("main", "run") —
#     every Python project has them.
#   - privates / dunders: a leading underscore ("_cand", "__init__") — implementation detail,
#     never a topical link.
#   - test scaffolding: a "test_" prefix — test function names are framework boilerplate.
#   - framework boilerplate by name (_CODE_BOILERPLATE): the Alembic migration-env API
#     (upgrade/downgrade/run_migrations_*). Round-7 audit: the locus repo's top "related"
#     doc was Tanker-Flow purely because both merely *use* Alembic — `run_migrations_offline`
#     had been kept as "distinctive" (round 6) when it is in fact the most generic possible
#     code link. DISTINCTIVE shared identifiers (a domain function reused across the owner's
#     own projects, e.g. `implied_vol_from_mid`) are deliberately NOT listed — those ARE a
#     real cross-project link, the whole point of the layer.
#
# Anything with case or spaces ('F1', 'Bode plot', a shared library class) is untouched.
_CODE_BOILERPLATE = frozenset(
    {"upgrade", "downgrade", "run_migrations_online", "run_migrations_offline"}  # Alembic env.py
)
_BARE_IDENT_FILTER = (
    "NOT ("
    "  a.canonical_name LIKE '\\_%' ESCAPE '\\'"
    "  OR a.canonical_name LIKE 'test\\_%' ESCAPE '\\'"
    "  OR (a.canonical_name = lower(a.canonical_name)"
    "      AND length(a.canonical_name) < 6"
    "      AND a.canonical_name NOT LIKE '% %')"
    "  OR lower(a.canonical_name) IN ("
    + ",".join(f"'{n}'" for n in sorted(_CODE_BOILERPLATE))
    + ")"
    ")"
)


@dataclass(frozen=True)
class RelatedDoc:
    doc_id: int
    title: str
    shared_count: int  # distinct shared canonical names
    shared_names: tuple[str, ...]  # up to _SAMPLE_NAMES, most distinctive (rarest) first


# The guard only makes sense once the corpus is large enough for "appears in many docs" to
# mean "ubiquitous". Below this it stays off, matching CLAUDE.md §9 ("off at small scale").
_MIN_CORPUS_FOR_STOP = 50


def resolve_stop_doc_freq(conn: sqlite3.Connection, ratio: float | None = None) -> int | None:
    """Absolute stop-entity doc-frequency from the config ratio x current corpus size.

    Returns None (guard off) when the ratio is <= 0 or the corpus is below the small-corpus
    floor; otherwise int(ratio x doc_count) — e.g. 0.4 x 313 docs -> exclude entities in
    more than 125 documents. Scales with the corpus, so no re-tuning as it grows.
    """
    if ratio is None:
        from locus.config import load

        ratio = load().alias.stop_doc_freq_ratio
    if ratio <= 0:
        return None
    doc_count = conn.execute("SELECT COUNT(*) AS n FROM documents").fetchone()["n"]
    if doc_count < _MIN_CORPUS_FOR_STOP:
        return None
    return int(ratio * doc_count)


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
    top_n: int = 10,
    stop_doc_freq: int | None = None,
) -> list[str]:
    """Render the RELATED DOCUMENTS block (shared by `locus inspect` and MCP inspect).

    top_n=10 (not 5): a code project shares its rarest, highest-weight concepts with other
    projects, so the *papers* it is built on rank ~6-15 (they tie on a low-IDF generic concept
    like 'Markov model'). A top-10 view surfaces those papers in `inspect`/MCP without the
    Phase-2 fuzzy-linking machinery. The eval's `score_links` keeps its own explicit top-5 bar.

    `stop_doc_freq=None` resolves the corpus-aware stop-entity guard from config (the
    production default); pass an explicit int to override, or 0/None-via-config to disable.
    """
    if not aliases_built(conn):
        return ["RELATED DOCUMENTS: (run `locus link` to build the alias substrate)"]
    if stop_doc_freq is None:
        stop_doc_freq = resolve_stop_doc_freq(conn)
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
    """Top documents sharing canonical entity NAMES with `doc_id`, IDF-weighted.

    Each shared name contributes 1/doc_freq (docs it appears in corpus-wide) to the
    ranking, so corpus-ubiquitous terms cannot dominate. `stop_doc_freq` additionally
    EXCLUDES names appearing in more than that many documents (hard stop-entity cut —
    off by default at small corpus scale; the pour runbook enables it, ~0.4 x doc count).
    Returns [] when the alias substrate has not been built yet.
    """
    stop_clause = ""
    params: list = [doc_id, doc_id]
    if stop_doc_freq is not None:
        stop_clause = " AND nf.doc_freq <= ?"
        params.append(stop_doc_freq)

    rows = conn.execute(
        f"""
        WITH canon_docs AS (
            SELECT DISTINCT a.canonical_name, e.doc_id
            FROM entities e
            JOIN entity_aliases a
              ON a.variant_name = e.name AND a.variant_type = e.type
            WHERE {_BARE_IDENT_FILTER}
        ),
        name_freq AS (
            SELECT canonical_name, COUNT(DISTINCT doc_id) AS doc_freq
            FROM canon_docs GROUP BY canonical_name
        ),
        my AS (SELECT canonical_name FROM canon_docs WHERE doc_id = ?)
        SELECT cd.doc_id, d.title,
               COUNT(*)                  AS shared,
               SUM(1.0 / nf.doc_freq)    AS weight
        FROM canon_docs cd
        JOIN my        ON my.canonical_name = cd.canonical_name
        JOIN name_freq nf ON nf.canonical_name = cd.canonical_name
        JOIN documents d  ON d.id = cd.doc_id
        WHERE cd.doc_id != ?{stop_clause}
        GROUP BY cd.doc_id
        ORDER BY weight DESC, shared DESC, cd.doc_id
        LIMIT {int(top_n)}
        """,
        params,
    ).fetchall()

    out: list[RelatedDoc] = []
    for r in rows:
        name_params: list = [doc_id, r["doc_id"]]
        name_stop = ""
        if stop_doc_freq is not None:
            name_stop = " AND nf.doc_freq <= ?"
            name_params.append(stop_doc_freq)
        name_params.append(_SAMPLE_NAMES)
        names = [
            n["canonical_name"]
            for n in conn.execute(
                f"""
                WITH canon_docs AS (
                    SELECT DISTINCT a.canonical_name, e.doc_id
                    FROM entities e
                    JOIN entity_aliases a
                      ON a.variant_name = e.name AND a.variant_type = e.type
                    WHERE {_BARE_IDENT_FILTER}
                ),
                name_freq AS (
                    SELECT canonical_name, COUNT(DISTINCT doc_id) AS doc_freq
                    FROM canon_docs GROUP BY canonical_name
                )
                SELECT c1.canonical_name
                FROM canon_docs c1
                JOIN canon_docs c2 ON c2.canonical_name = c1.canonical_name
                JOIN name_freq nf  ON nf.canonical_name = c1.canonical_name
                WHERE c1.doc_id = ? AND c2.doc_id = ?{name_stop}
                ORDER BY nf.doc_freq ASC, c1.canonical_name
                LIMIT ?
                """,
                name_params,
            )
        ]
        out.append(RelatedDoc(r["doc_id"], r["title"] or "(untitled)", r["shared"], tuple(names)))
    return out
