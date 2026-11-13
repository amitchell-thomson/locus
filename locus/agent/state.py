"""Whose writing a library document is: `owner_authored_sql`, the one definition.

This module held the Phase-2 agent state (objects, links, belief positions, the acceptance log)
for the proposer, the trajectory, the review cards and the daily page. All of those were deleted
in Slice C (C2, 13 Nov 2026); their tables stay (migrations are history), and this predicate,
which heuris search reads, is what remains.
"""

from __future__ import annotations


def owner_authored_sql(alias: str = "d") -> tuple[str, list]:
    """SQL predicate for "this document is HIS writing" — ONE definition, several consumers.

    Provenance first: everything he writes lands under the notes directory whatever category the
    DEVICE FOLDER assigned (`Notes/engineering` -> coursework), so keying on category alone
    silently made his own handwriting not-his. Then category + format, both load-bearing, because
    `project` also holds third-party manuals he keeps for reference.

    heuris search asks it to tell his notes from sources (`note` vs `source` hits).
    """
    from locus.config import load

    cfg = load()
    notes = str(cfg.paths.notes).replace("\\", "/").rstrip("/")
    clause = f"({alias}.source_uri LIKE ? OR {alias}.source_uri LIKE ?"
    params: list = [f"{notes}/%", "%/data/notes/%"]
    cats = list(cfg.structure.belief_source_categories)
    if cats:
        from locus.ingest_pipeline import AREA_FOLDERS

        # Nothing dropped into an area folder is his writing (ingest_pipeline.is_area_drop). LIKE
        # is case-insensitive for ASCII, matching the Python test's lower().
        sub = " AND ".join(
            f"('/' || COALESCE({alias}.source_uri, '')) NOT LIKE ?" for _ in AREA_FOLDERS
        )
        params += [f"%/incoming/{area}/%" for area in AREA_FOLDERS]
        sub += f" AND {alias}.category IN ({','.join('?' * len(cats))})"
        params += cats
        types = list(cfg.structure.belief_source_types)
        if types:
            sub += f" AND {alias}.source_type IN ({','.join('?' * len(types))})"
            params += types
        clause += f" OR ({sub})"
    return clause + ")", params
