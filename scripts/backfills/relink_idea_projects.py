"""Re-derive idea/question -> project links with the two-tier matcher (`link/projects.py`).

WHY A BACKFILL. The links were written once, by a cosine, and never revisited — while the
`discovery_profiles` they were derived from get rebuilt. By 2026-08-03 two of the four stored
links no longer matched what even the OLD matcher would produce, and one of them was plainly
wrong: "read next on alt-data?" was linked to `OxAI`, an exam-question generator, and retrieval
stated it to Claude as fact (`part of: OxAI`).

Only PROJECT links are touched. `raised_by` (the document that provoked the thought) and
thread<->thread links are left exactly as they are: they are evidence, not inference.

    uv run python scripts/backfills/relink_idea_projects.py            # dry run, prints the diff
    uv run python scripts/backfills/relink_idea_projects.py --apply
"""

from __future__ import annotations

import argparse
import json
import sys

from locus.config import load
from locus.db.connection import get_connection
from locus.link.projects import projects_for

THREAD_TYPES = ("idea", "question")


def text_for(conn, obj_id: int, body: dict) -> str:
    """His words, plus the passage a mark was written beside — the same input `intent.py` uses.

    The passage matters: "interesting, can we plot this behavior?" names no concept on its own,
    because the concept is in the paragraph he was reading when he wrote it.
    """
    parts = [str(body.get(k) or "") for k in ("idea", "question")]
    row = conn.execute(
        "SELECT covered_text FROM pdf_annotations WHERE object_id=? LIMIT 1", (obj_id,)
    ).fetchone()
    if row:
        parts.append(row["covered_text"] or "")
    return " ".join(p for p in parts if p).strip()


def current_project_links(conn, obj_id: int) -> dict[int, str]:
    out: dict[int, str] = {}
    for row in conn.execute(
        "SELECT target_key FROM object_links WHERE object_id=? AND target_kind='object' "
        "AND relation='relates'",
        (obj_id,),
    ):
        key = str(row["target_key"])
        if not key.isdigit():
            continue
        target = conn.execute(
            "SELECT id, title, type FROM objects WHERE id=?", (int(key),)
        ).fetchone()
        if target is not None and target["type"] == "project":
            out[target["id"]] = target["title"]
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--apply", action="store_true", help="write the changes (default: dry run)")
    args = ap.parse_args()

    conn = get_connection(load().paths.db)
    changed = added = removed = 0
    try:
        rows = conn.execute(
            f"SELECT id, title, body FROM objects WHERE type IN "
            f"({','.join('?' * len(THREAD_TYPES))}) ORDER BY id",
            THREAD_TYPES,
        ).fetchall()
        for row in rows:
            try:
                body = json.loads(row["body"] or "{}")
            except (TypeError, ValueError):
                body = {}
            text = text_for(conn, row["id"], body)
            if not text:
                continue
            want = dict(projects_for(conn, text))
            have = current_project_links(conn, row["id"])
            if set(want) == set(have):
                continue
            changed += 1
            print(f"obj {row['id']} {(row['title'] or '')[:52]!r}")
            for pid, title in have.items():
                if pid not in want:
                    print(f"    - {title}")
                    removed += 1
            for pid, title in want.items():
                if pid not in have:
                    print(f"    + {title}")
                    added += 1
            if args.apply:
                with conn:
                    for pid in have:
                        if pid not in want:
                            conn.execute(
                                "DELETE FROM object_links WHERE object_id=? AND target_kind=? "
                                "AND target_key=? AND relation='relates'",
                                (row["id"], "object", str(pid)),
                            )
                    for pid in want:
                        if pid not in have:
                            conn.execute(
                                "INSERT OR IGNORE INTO object_links "
                                "(object_id, target_kind, target_key, relation) "
                                "VALUES (?,?,?,'relates')",
                                (row["id"], "object", str(pid)),
                            )
        verb = "applied" if args.apply else "would change"
        print(f"\n{verb}: {changed} object(s) · +{added} link(s) · -{removed} link(s)")
        if not args.apply and changed:
            print("re-run with --apply to write")
    finally:
        conn.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
