"""Which of his projects is a piece of his own writing about?

THE PROBLEM THIS EXISTS FOR. An idea he jots in a margin, or a question he writes on the daily
page, is only worth storing as an object if it lands beside the work it bears on. The first cut
asked `reading.relevance.best_subject` — a cosine against `discovery_profiles` — and it was wrong
more often than right. Measured over the first seven real items (2026-08-03):

  * three of four mark-born ideas linked to a project they were not about. "read next on
    alt-data?" linked to `OxAI`, an exam-question generator, and retrieval then stated that to
    Claude as fact: `part of: OxAI`.
  * every question written on the daily page linked to NOTHING, because `discovery_profiles`
    holds `gap` rows as well as `project` rows and a concept label ("AIS capture", "Hierarchical
    Attention Network") routinely won the unrestricted search. The caller then looked that label
    up in `objects` as a project title, found nothing, and silently dropped the link.

So the ordering is deterministic-first, exactly like `link/aliases.py`: a fact that can be checked
by reading beats a number that cannot.

  TIER 1  he NAMED it. Every distinctive token of the project's title appears in his text.
          `tanker-flow` -> {tanker}, so "in the tanker project" matches. Requiring ALL tokens is
          what stops `Swaps Momentum Strategy` from firing on the bare word "strategy", which it
          did on his systematic-strategy idea. Tier 1 may return SEVERAL projects, and that is
          correct: "a macro regime predictor in the tanker project" is genuinely about both
          `regime-ml` and `tanker-flow`.

  TIER 2  nothing was named, so fall back to the nearest PROJECT profile, and only above
          `[capture].idea_project_fit_floor`. On the same seven items the one correct cosine link
          scored 0.756 and every wrong one 0.637-0.673; a runner-up margin test does not separate
          them (the correct link beat its runner-up by 0.008, exactly like a wrong one).

Returning nothing is a valid and common answer. An unlinked idea is still an idea; a confidently
wrong link is worse than none, because every consumer downstream reads it as a statement of fact.
"""

from __future__ import annotations

import re
import sqlite3

# Title words too generic to identify a project on their own. Without this, "Alpha Fund" fires on
# any mention of alpha and "Quant Data Ingestion Pipeline" on any mention of data — the words are
# his whole vocabulary, so they carry no evidence about WHICH project he meant.
_GENERIC_TITLE_TOKENS = frozenset({
    "fund", "data", "analysis", "trading", "python", "solutions", "reference", "pipeline",
    "system", "project", "projects", "algorithms", "alpha", "quant", "prediction", "generation",
})

# Below this a token is an abbreviation or a connective ("ml", "flow", "oxai"), not a name that
# can carry the match on its own.
_MIN_TOKEN_CHARS = 5

_WORD = re.compile(r"[a-z0-9]+")


def _tokens(text: str) -> list[str]:
    return _WORD.findall((text or "").lower())


def _distinctive(title: str) -> list[str]:
    return [
        t for t in _tokens(title)
        if len(t) >= _MIN_TOKEN_CHARS and t not in _GENERIC_TITLE_TOKENS
    ]


def _live_projects(conn: sqlite3.Connection) -> list[tuple[int, str]]:
    rows = conn.execute(
        "SELECT id, title FROM objects WHERE type='project' AND status != 'archived' ORDER BY id"
    ).fetchall()
    return [(r["id"], r["title"] or "") for r in rows]


def named_projects(conn: sqlite3.Connection, text: str) -> list[tuple[int, str]]:
    """Tier 1: projects he named outright. Every distinctive title token must be present."""
    words = set(_tokens(text))
    if not words:
        return []
    out = []
    for pid, title in _live_projects(conn):
        distinctive = _distinctive(title)
        if distinctive and all(tok in words for tok in distinctive):
            out.append((pid, title))
    return out


def projects_for(
    conn: sqlite3.Connection, text: str, *, floor: float | None = None
) -> list[tuple[int, str]]:
    """The project objects this text is about, as [(object_id, title)] — possibly empty.

    `floor` overrides `[capture].idea_project_fit_floor`, which is what the tests vary.
    """
    if not (text or "").strip():
        return []

    named = named_projects(conn, text)
    if named:
        return named

    from locus.config import load
    from locus.reading.relevance import best_project

    cutoff = load().capture.idea_project_fit_floor if floor is None else floor
    label, fit = best_project(conn, text)
    if not label or fit < cutoff:
        return []
    # A link must name a row that exists: the profile label is resolved back to the actual object,
    # because storing the label where an object id belongs is how the first cut of this pointed
    # every idea->project link at nothing resolvable.
    row = conn.execute(
        "SELECT id, title FROM objects WHERE type='project' AND lower(title)=lower(?) "
        "AND status != 'archived' LIMIT 1",
        (label,),
    ).fetchone()
    return [(row["id"], row["title"])] if row else []
