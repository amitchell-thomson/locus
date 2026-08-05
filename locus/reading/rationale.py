"""The reason each proposed paper is on the shelf — delivered to the shelf itself.

WHY IT MOVED HERE. The daily page used to carry a Read section: three proposals with their
written reasons. Two days of real use showed two things about it. It duplicated the
`Reading/Proposed` folder he already browses, and he never used it to decide what to read — the
writing he put on it was project thinking about the papers ("will investigate if we can apply
these methods to optimize the ultimate portfolio construction"), which is the Ideas page's job.

But the reasons themselves are the valuable part: without them the shelf is a list of filenames,
and material he sought out himself arrives with less context than material the ranker chose. So
the rationale is not deleted, it is MOVED to where the decision is actually made. One document,
listing every proposal with its reason and what it links to, sitting in `Reading/Proposed`
alongside the papers.

REFRESHED WHEN THE SHELF CHANGES, not on a schedule. The document is keyed by a fingerprint over
the proposals and their reason-timestamps, so a re-run with an unchanged shelf costs nothing and
does not churn the device. A paper accepted or a reason rewritten changes the fingerprint and the
document is replaced.

Free and local: every reason was composed and stored earlier by `discover/why.py`. This renders.
"""

from __future__ import annotations

import hashlib
import logging
import sqlite3
from dataclasses import dataclass
from datetime import date

log = logging.getLogger(__name__)

# The filename on the device. Stable, so a refresh replaces rather than accumulates — a shelf
# with six "why" documents on it would be worse than none.
DOC_NAME = "Why these papers"


@dataclass
class Rationale:
    fingerprint: str
    markdown: str
    count: int


def _rows(conn: sqlite3.Connection) -> list[sqlite3.Row]:
    try:
        return list(
            conn.execute(
                "SELECT id, title, why, why_long, why_kind, evidence_key, score, proposed_at, "
                "       why_written_at "
                "FROM reading_proposals WHERE status='proposed' "
                "ORDER BY COALESCE(score, 0) DESC, id ASC"
            )
        )
    except sqlite3.OperationalError:                   # discovery tables absent
        return []


def fingerprint(conn: sqlite3.Connection) -> str:
    """Identifies the shelf's CONTENT, so an unchanged shelf is not re-delivered.

    Over ids and reason timestamps rather than rendered bytes: the render includes a build date,
    and hashing the output would make every run look like a change.
    """
    parts = [f"{r['id']}:{r['why_written_at'] or ''}" for r in _rows(conn)]
    return hashlib.sha256("|".join(parts).encode()).hexdigest()


def _subject(row: sqlite3.Row) -> str:
    """What this paper was matched against — the project or gap that pulled it in."""
    key = (row["evidence_key"] if "evidence_key" in row.keys() else "") or ""
    kind = (row["why_kind"] if "why_kind" in row.keys() else "") or ""
    if ":" in key:
        return key.split(":", 1)[1]
    return key or kind


def render(conn: sqlite3.Connection, *, today: date | None = None) -> Rationale:
    """The shelf as a readable document. Empty shelf renders an honest short page."""
    rows = _rows(conn)
    today = today or date.today()
    lines = [
        "# Why these papers",
        "",
        f"*The shelf as of {today.isoformat()}. Each entry is why it was proposed and what it "
        "connects to.*",
        "",
    ]
    if not rows:
        lines += [
            "The shelf is empty — nothing is proposed right now.",
            "",
            "That is a valid state, not a failure: a full folder proposes nothing, and the "
            "harvest only adds when there is room.",
        ]
        return Rationale(fingerprint(conn), "\n".join(lines) + "\n", 0)

    for i, row in enumerate(rows, 1):
        keys = row.keys()
        why_long = (row["why_long"] if "why_long" in keys else None) or ""
        why = " ".join((why_long or row["why"] or "").split())
        subject = _subject(row)
        lines += [f"## {i}. {row['title']}", ""]
        if why:
            lines += [why, ""]
        bits = []
        if subject:
            bits.append(f"links to **{subject}**")
        if row["proposed_at"]:
            bits.append(f"proposed {str(row['proposed_at'])[:10]}")
        if bits:
            lines += [f"*{' · '.join(bits)}*", ""]
        lines += ["***", ""]

    lines += [
        "",
        "*Move a paper out of this folder to accept it — that is the signal that teaches the "
        "ranker what to look for next. Deleting it is a firm no; leaving it here is neither.*",
    ]
    return Rationale(fingerprint(conn), "\n".join(lines).rstrip() + "\n", len(rows))
