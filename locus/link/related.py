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

import math
import sqlite3
import struct
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

# Code-symbol segregation (Phase 2, §1.2 Link). A repo's AST identifiers (functions ->
# 'method', classes -> 'concept'; extract/code.py) are the exact function/class names, not the
# DOMAIN concepts that bridge a project to its papers. The round-6 `_BARE_IDENT_FILTER` above
# only catches short/private/test names; identifiers like `lifespan`, `Candidate`, `build_signals`
# survive and, being rare, earn high IDF weight — a shared framework identifier (`lifespan` in two
# FastAPI repos) then outranks a genuinely shared concept (`mean reversion`). So we CLASSIFY
# code-symbols and score them separately: concepts drive ranking for ANY pair; a code-symbol
# contributes only a small tiebreak weight and ONLY when BOTH docs are code (never code<->paper).
# This keeps a distinctive shared function (`implied_vol_from_mid`) visible as a code<->code link
# without letting boilerplate displace concept links.
#
# Classifier (joins-only, no new column — the substrate stays regenerable): a canonical is a
# code-symbol iff it is single-token (no space — every real bridging concept the pass emits is
# multi-word, e.g. 'mean reversion', or is attested in narrative) AND every one of its corpus
# occurrences is a code-file section of a code doc. A concept anchored to a repo's `.md` doc, or
# any surface appearing in a paper/coursework doc, therefore never classifies as a code-symbol.
_NARRATIVE_SECTION = (
    "(s.file_path IS NULL"
    " OR lower(s.file_path) LIKE '%.md'"
    " OR lower(s.file_path) LIKE '%.markdown'"
    " OR lower(s.file_path) LIKE '%.rst'"
    " OR lower(s.file_path) LIKE '%.txt')"
)
_NARRATIVE_OCCURRENCE = f"(d.source_type != 'code' OR {_NARRATIVE_SECTION})"

# The (canonical_name, doc_id) substrate + corpus-wide doc frequency per canonical name + the
# code-symbol set, shared by every related-docs query (the candidate scoring, the per-doc vector
# norms, and the shared-name sampling). Factored to one place so the boilerplate filter, frequency
# definition, and code-symbol classifier cannot drift between them.
_CANON_CTE = f"""
    canon_docs AS (
        SELECT DISTINCT a.canonical_name, e.doc_id
        FROM entities e
        JOIN entity_aliases a
          ON a.variant_name = e.name AND a.variant_type = e.type
        WHERE {_BARE_IDENT_FILTER}
          -- A DOCUMENT TITLE IS NOT A SHARED CONCEPT. "Both develop Advanced Portfolio
          -- Management" is vacuous between a book and the notes written inside it: it says only
          -- that one came from the other. §21 established this for thread linking ("all four
          -- ideas from one book formed a complete graph asserting only that he read it") and the
          -- fix went into `link/threads.py` alone, so this surface reproduced it exactly — on the
          -- first real daily page (2026-08-03) FOUR of the top five connections were a mark-born
          -- idea paired with the book it was written in. Excluded inside `canon_docs` so RANKING
          -- and DISPLAY agree: filtering only the rendered names would leave the vacuous pair
          -- ranked first with its reason removed, which is worse.
          AND LOWER(TRIM(a.canonical_name)) NOT IN (
              SELECT LOWER(TRIM(title)) FROM documents
              WHERE TRIM(COALESCE(title, '')) != ''
          )
    ),
    name_freq AS (
        SELECT canonical_name, COUNT(DISTINCT doc_id) AS doc_freq
        FROM canon_docs GROUP BY canonical_name
    ),
    code_symbols AS (
        SELECT a.canonical_name
        FROM entities e
        JOIN entity_aliases a
          ON a.variant_name = e.name AND a.variant_type = e.type
        JOIN sections s  ON s.id = e.section_id
        JOIN documents d ON d.id = e.doc_id
        WHERE a.canonical_name NOT LIKE '% %'
        GROUP BY a.canonical_name
        HAVING SUM(CASE WHEN {_NARRATIVE_OCCURRENCE} THEN 1 ELSE 0 END) = 0
    )
"""

# A code-symbol's tiebreak weight, applied ONLY to code<->code pairs. Small (0.1) so any shared
# domain concept outranks any amount of shared boilerplate, yet non-zero so a distinctive shared
# function still surfaces as a code<->code neighbour when no concept is shared.
_CODE_SYMBOL_WEIGHT = 0.1


def non_topical_names(conn: sqlite3.Connection) -> set[str]:
    """Canonical names too generic to carry topical meaning — boilerplate + code symbols.

    Exposed because the structured-object proposer needs exactly this judgement (a concept too
    generic to justify a related-doc link is too generic to become a Concept object), and a second
    implementation would drift from this one. Round 6/7 tuned these predicates against the live
    corpus; a 2026-07-28 proposal dry-run without them offered `state` and `ingest` as domain
    concepts for the tanker-flow repo, which is the same failure in a new surface.

    Returns lowercase names. Empty when the alias substrate has not been built."""
    names: set[str] = set()
    # Bare identifiers: privates/dunders, test scaffolding, short lowercase single tokens, and
    # the named framework boilerplate — the complement of _BARE_IDENT_FILTER.
    for r in conn.execute(
        "SELECT DISTINCT a.canonical_name AS n FROM entity_aliases a "
        f"WHERE NOT ({_BARE_IDENT_FILTER})"
    ):
        names.add(r["n"].lower())
    # Code symbols: single-token canonicals whose every corpus occurrence is a code-file section.
    for r in conn.execute(f"WITH {_CANON_CTE} SELECT canonical_name AS n FROM code_symbols"):
        names.add(r["n"].lower())
    # DOCUMENT TITLES ARE NOT CONCEPTS. A canonical equal to some document's title says only
    # "these two texts are about that document", which is vacuous between a book and the notes
    # written inside it. §21 established this for thread linking — "all four ideas from one book
    # formed a complete graph asserting only that he read it" — but the fix went into
    # `link/threads.py` alone, so the CONNECTIONS surface reproduced it exactly: on the first real
    # page (2026-08-03) FOUR of the top five connections were a mark-born idea paired with
    # *Advanced Portfolio Management*, sharing the "concept" Advanced Portfolio Management.
    for r in conn.execute(
        "SELECT DISTINCT TRIM(title) AS n FROM documents WHERE TRIM(COALESCE(title,'')) != ''"
    ):
        names.add(r["n"].lower())
    return names


# Cross-domain ranking (round-8). The owner's high-value material — quant/finance PAPERS, code
# PROJECTS, and CAREER/application docs — is a minority of a corpus that is ~80% engineering
# coursework (246/313), a dense topical clique. So a high-value doc's genuinely useful cross-domain
# neighbour (a repo and the paper it implements; a project and the role it targets) ranks just below
# its same-category siblings and the incidental coursework overlap. Boosting a link's IDF weight
# when BOTH endpoints are high-value categories lifts paper<->project, project<->career,
# paper<->career (and same-category project<->project etc.) above coursework noise — WITHOUT
# touching coursework links (factor 1.0; they are legitimate, just not the under-served signal),
# so it cannot demote the coursework pairs the link eval protects. Tunable; pass {} to disable.
_HIGH_VALUE_CATEGORIES = ("paper", "project", "career")
_CROSS_DOMAIN_BOOST = 1.5


def _default_category_affinity() -> dict[tuple[str, str], float]:
    """Symmetric (src_cat, cand_cat) -> weight multiplier; >1 only among high-value categories."""
    return {
        (a, b): _CROSS_DOMAIN_BOOST
        for a in _HIGH_VALUE_CATEGORIES
        for b in _HIGH_VALUE_CATEGORIES
    }


# --- acceptance flywheel (plan §12.1) --------------------------------------------------------
#
# Keep/reject of a surfaced connection is a free labelled relevance judgement. `acceptance_log`
# has been collecting them since Phase 2, and `acceptance_counts()` had NO CALLERS — 32 recorded
# judgements that changed nothing. This is where they finally do something.
#
# Deliberately gentle and bounded. The signal is thin (a handful of judgements per document at
# most), the ranking underneath it is well-tuned, and an over-eager multiplier would let two
# accidental ticks reorder a graph that currently scores links_recall 1.000. So: one step per
# net judgement, hard-clamped, and applied to the SAME slot as category affinity (a multiplier on
# concept weight) rather than as a new arm — it can reorder neighbours, never invent one.
#
# Scoped to the daily-connection surface only. A blessing or a recall judgement says nothing
# about whether two documents belong near each other.
_ACCEPTANCE_SURFACE = "connection"  # the vocabulary migration 0011 fixed in a CHECK constraint
_ACCEPTANCE_STEP = 0.15
_ACCEPTANCE_MIN = 0.6
_ACCEPTANCE_MAX = 1.6


def acceptance_factors(conn: sqlite3.Connection) -> dict[int, float]:
    """doc_id -> ranking multiplier learned from what the owner kept or ignored.

    Keyed through `source_uri`, not a doc row id: acceptance judgements outlive re-ingest, and a
    judgement silently reattaching itself to whatever document inherited an id would be worse
    than losing it (the lesson migration 0012 paid for).

    Returns {} when nothing has been judged, which makes this inert until the loop has actually
    run — no behaviour change on day one.
    """
    from locus.agent.state import acceptance_counts

    counts = acceptance_counts(conn, surface=_ACCEPTANCE_SURFACE)
    if not counts:
        return {}
    by_uri = {
        (r["source_uri"] or ""): r["id"]
        for r in conn.execute("SELECT id, source_uri FROM documents")
    }
    out: dict[int, float] = {}
    for uri, verdicts in counts.items():
        doc_id = by_uri.get(uri)
        if doc_id is None:
            continue  # judged document no longer in the corpus: drop it, never guess a target
        net = verdicts.get("kept", 0) - verdicts.get("rejected", 0)
        if net:
            out[doc_id] = max(
                _ACCEPTANCE_MIN, min(_ACCEPTANCE_MAX, 1.0 + _ACCEPTANCE_STEP * net)
            )
    return out


def _load_category_affinity() -> dict[tuple[str, str], float]:
    """Production affinity matrix. Module default for now; a config override can be wired here
    later (the function boundary keeps `related_documents` agnostic to where the matrix is set)."""
    return _default_category_affinity()


# Semantic arm (round-8). The entity-overlap arm is precise but blind to docs that are clearly
# about the same thing yet share no canonical surface at all. A doc-level embedding (mean-pooled
# section vectors, cosine ~0.91 for same-topic docs) recovers these. It is STRICTLY TAIL-ADDITIVE:
# a semantic-only neighbour (no shared entity) can only fill a slot AFTER every entity/concept
# neighbour, never displace one. (Reciprocal-Rank Fusion was tried and reverted — with k=60 it
# flattened entity rank 1 vs 7, so genre similarity, all the owner's repos embed alike, displaced
# genuine concept links; measured: it demoted tanker-flow's concept-shared downside-risk neighbour
# below its boilerplate-identifier ones. Post code-concept extraction the concept arm subsumes the
# arm's original recall job, so additive-only loses nothing and restores links_recall to 1.000.)
# Graceful: with no section vectors the arm is inert.
_SEMANTIC_CANDIDATES = 10  # top-K semantic-only neighbours appended per source doc
_doc_vector_cache: dict[int, dict[int, list[float]]] = {}


def _doc_vectors(conn: sqlite3.Connection) -> dict[int, list[float]]:
    """Per-doc L2-normalised embedding = mean of its section vectors. Cached per connection for
    the process lifetime (the substrate is static during a link/inspect/export run)."""
    cached = _doc_vector_cache.get(id(conn))
    if cached is not None:
        return cached
    acc: dict[int, list[float]] = {}
    cnt: dict[int, int] = {}
    rows = conn.execute(
        "SELECT s.doc_id AS doc_id, sv.embedding AS embedding "
        "FROM section_vectors sv JOIN sections s ON s.id = sv.section_id"
    ).fetchall()
    for r in rows:
        vec = struct.unpack(f"{len(r['embedding']) // 4}f", r["embedding"])
        a = acc.get(r["doc_id"])
        if a is None:
            acc[r["doc_id"]] = list(vec)
        else:
            for i, x in enumerate(vec):
                a[i] += x
        cnt[r["doc_id"]] = cnt.get(r["doc_id"], 0) + 1
    out: dict[int, list[float]] = {}
    for doc_id, a in acc.items():
        n = cnt[doc_id]
        mean = [x / n for x in a]
        norm = math.sqrt(sum(x * x for x in mean)) or 1.0
        out[doc_id] = [x / norm for x in mean]
    _doc_vector_cache[id(conn)] = out
    return out


def _shared_names(
    conn: sqlite3.Connection, doc_a: int, doc_b: int, stop_doc_freq: int | None
) -> list[str]:
    """The shared canonical names between two docs, rarest (most distinctive) first."""
    params: list = [doc_a, doc_b]
    name_stop = ""
    if stop_doc_freq is not None:
        name_stop = " AND nf.doc_freq <= ?"
        params.append(stop_doc_freq)
    params.append(_SAMPLE_NAMES)
    return [
        n["canonical_name"]
        for n in conn.execute(
            f"""
            WITH {_CANON_CTE}
            SELECT c1.canonical_name
            FROM canon_docs c1
            JOIN canon_docs c2 ON c2.canonical_name = c1.canonical_name
            JOIN name_freq nf  ON nf.canonical_name = c1.canonical_name
            LEFT JOIN code_symbols cs ON cs.canonical_name = c1.canonical_name
            WHERE c1.doc_id = ? AND c2.doc_id = ?{name_stop}
            ORDER BY (cs.canonical_name IS NOT NULL), nf.doc_freq ASC, c1.canonical_name
            LIMIT ?
            """,
            params,
        )
    ]


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
    category_affinity: dict[tuple[str, str], float] | None = None,
    acceptance: dict[int, float] | None = None,
) -> list[RelatedDoc]:
    """Top documents sharing canonical entity NAMES with `doc_id`, IDF-weighted.

    Each shared name contributes 1/doc_freq (docs it appears in corpus-wide) to the ranking, so
    corpus-ubiquitous terms cannot dominate. `stop_doc_freq` additionally EXCLUDES names appearing
    in more than that many documents (hard stop-entity cut — off by default at small scale; the
    pour runbook enables it, ~0.4 x doc count). Returns [] when the alias substrate is unbuilt.

    Cross-domain ranking (round-8): the raw IDF-weighted overlap reflects the corpus's makeup —
    246/313 docs are engineering coursework, a dense topical clique — so for the owner's
    high-value docs (quant papers, code projects, career/applications) the genuinely useful
    cross-domain neighbours (a repo and the paper it implements) sit just below same-category
    siblings. `category_affinity` multiplies a pair's weight by a per-(src,cand)-category factor
    so links *among* the high-value categories surface above incidental coursework overlap. It
    is applied AFTER IDF weighting and never changes `shared_count`/`shared_names` (raw display);
    defaults from config (`relate_category_affinity`) when None — pass `{}` to disable.

    Code-symbol segregation (Phase 2): shared canonicals classified as code-symbols (AST
    identifiers — single-token, only ever in code-file sections; `code_symbols` CTE) drive
    ranking ONLY for code<->code pairs and only at a downweighted tiebreak (`_CODE_SYMBOL_WEIGHT`),
    so a shared DOMAIN concept always outranks a shared identifier and a code identifier never
    links a repo to a paper. Domain concepts drive ranking for any pair.

    A **semantic arm** (doc-embedding cosine, mean-pooled section vectors) is tail-additive: it
    appends same-topic neighbours that share NO entity surface after the entity/concept
    neighbours, never displacing one. Inert when section vectors are absent (e.g. seeded tests).

    NB cosine/length-normalised scoring of the ENTITY arm was tried and reverted: it tanked
    `links_recall` (1.000 -> 0.769) by over-rewarding short docs.
    """
    if category_affinity is None:
        category_affinity = _load_category_affinity()
    if acceptance is None:
        acceptance = acceptance_factors(conn)

    src_row = conn.execute(
        "SELECT category, source_type FROM documents WHERE id = ?", (doc_id,)
    ).fetchone()
    src_cat = (src_row["category"] if src_row else None) or ""
    src_is_code = bool(src_row) and src_row["source_type"] == "code"

    stop_clause = ""
    stop_params: list = []
    if stop_doc_freq is not None:
        stop_clause = " AND nf.doc_freq <= ?"
        stop_params = [stop_doc_freq]

    cands = conn.execute(
        f"""
        WITH {_CANON_CTE},
        my AS (SELECT canonical_name FROM canon_docs WHERE doc_id = ?)
        SELECT cd.doc_id, d.title, d.category, d.source_type,
               COUNT(*) AS shared,
               SUM(CASE WHEN cs.canonical_name IS NULL THEN 1 ELSE 0 END) AS concept_shared,
               SUM(CASE WHEN cs.canonical_name IS NULL
                        THEN 1.0 / nf.doc_freq ELSE 0 END) AS concept_weight,
               SUM(CASE WHEN cs.canonical_name IS NOT NULL
                        THEN 1.0 / nf.doc_freq ELSE 0 END) AS code_weight
        FROM canon_docs cd
        JOIN my        ON my.canonical_name = cd.canonical_name
        JOIN name_freq nf ON nf.canonical_name = cd.canonical_name
        JOIN documents d  ON d.id = cd.doc_id
        LEFT JOIN code_symbols cs ON cs.canonical_name = cd.canonical_name
        WHERE cd.doc_id != ?{stop_clause}
        GROUP BY cd.doc_id
        """,
        [doc_id, doc_id, *stop_params],
    ).fetchall()

    # --- entity arm: concepts drive ranking (any pair); code-symbols add only a downweighted
    # tiebreak, and only for code<->code (never code<->paper). Category-affinity multiplies the
    # concept weight. `shared` for display prefers the shared CONCEPT count so the graph reads as
    # topic links; a pure code<->code neighbour (no shared concept) falls back to its raw count.
    entity_scored = []
    for r in cands:
        both_code = src_is_code and r["source_type"] == "code"
        affinity = category_affinity.get((src_cat, r["category"] or ""), 1.0)
        weight = r["concept_weight"] * affinity * acceptance.get(r["doc_id"], 1.0)
        if both_code:
            weight += _CODE_SYMBOL_WEIGHT * r["code_weight"]
        if weight <= 0:
            continue  # only code-symbols shared across a non-code pair -> not a topical link
        shared = r["concept_shared"] or r["shared"]
        entity_scored.append((weight, shared, r["doc_id"], r["title"]))
    entity_scored.sort(key=lambda t: (-t[0], -t[1], t[2]))
    meta = {cid: (sh, ti) for _w, sh, cid, ti in entity_scored}

    # --- semantic arm: doc-embedding cosine, best-first; recovers entity-less neighbours ---
    sem_order: list[int] = []
    vectors = _doc_vectors(conn)
    src_vec = vectors.get(doc_id)
    if src_vec is not None:
        sims = sorted(
            ((sum(a * b for a, b in zip(src_vec, v)), cid) for cid, v in vectors.items() if cid != doc_id),
            reverse=True,
        )[:_SEMANTIC_CANDIDATES]
        sem_order = [cid for _s, cid in sims]

    # --- tail-additive fusion: entity/concept neighbours first (weight order), then semantic-only
    # docs (no shared entity) fill remaining slots — never displacing a concept neighbour. ---
    ordered = [cid for _w, _sh, cid, _ti in entity_scored]
    seen = set(ordered)
    for cid in sem_order:
        if cid not in seen:
            ordered.append(cid)
            seen.add(cid)

    out: list[RelatedDoc] = []
    for cand_id in ordered[: int(top_n)]:
        shared, title = meta.get(cand_id, (0, ""))
        if not title:
            row = conn.execute("SELECT title FROM documents WHERE id = ?", (cand_id,)).fetchone()
            title = (row["title"] if row else "") or "(untitled)"
        # Only entity-overlap neighbours have shared canonical names to display; a semantic-only
        # neighbour (shared == 0) is a meaning match with no shared surface.
        names = tuple(_shared_names(conn, doc_id, cand_id, stop_doc_freq)) if shared else ()
        out.append(RelatedDoc(cand_id, title or "(untitled)", shared, names))
    return out
