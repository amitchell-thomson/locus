"""Record what a threshold rejected, so a gate that admits nothing can be seen.

WHY THIS EXISTS. On 2026-08-03 `evolve/trajectory.find_tensions` was found to have been inert
since it was written: `_MAX_DISTANCE` was 0.55, so the only neighbouring claims it ever saw were
paraphrases of the owner's own stance, and a paraphrase never contradicts anything. The code was
correct, the tests passed either side of it, and the documentation called it the headline
capability. The constant simply admitted nothing.

Nothing in the system could have caught that. A test fixture is built to clear the gate. Reading
the code shows a number that looks reasonable. The only evidence of a dead threshold lives in what
it REJECTED — and nothing recorded that. This module does.

THE SHAPE IS DELIBERATE. One aggregate row per (gate, day) with a few verbatim samples, not a row
per rejection: `retrieve.min_rerank_score` alone discards thousands of candidates a day, and a row
each would make this the largest table in the database while making the question harder, not
easier. The question is "over a week, what did this gate throw away, and does that look right?",
and a count plus five examples answers it.

`passed` is recorded next to `rejected` because a rejection count alone cannot tell a gate that is
working hard from one that is rejecting everything — 900/1000 rejected is a retrieval floor doing
its job; 16/16 is a dead gate.

NEVER LOAD-BEARING. No pass reads this table; only `locus gates`. Every entry point swallows its
own errors, because an observability write must never be able to break the thing it observes — a
missing table (migration not yet run) degrades to silence.
"""

from __future__ import annotations

import json
import logging
import sqlite3
from datetime import datetime, timezone

log = logging.getLogger(__name__)

# Verbatim rejected values kept per gate per day. Five is enough to judge a gate by eye and small
# enough that the column stays readable in a terminal.
MAX_SAMPLES = 5

# Truncation for one stored sample: long enough to recognise a passage, short enough that a day's
# samples fit on a screen.
_MAX_SAMPLE_CHARS = 120


def _today() -> str:
    return datetime.now(timezone.utc).date().isoformat()


def record(
    conn: sqlite3.Connection,
    gate: str,
    *,
    rejected: bool,
    value: object = None,
    day: str | None = None,
) -> None:
    """Note one gate decision. Cheap, best-effort, and never raises.

    `value` is what the gate judged — a score, a length, a name. It is stored only for rejections,
    because the whole point is to see what is being discarded.
    """
    try:
        _record(conn, gate, rejected=rejected, value=value, day=day or _today())
    except Exception as exc:                       # observability must never break its subject
        log.debug("gates: could not record %s: %s", gate, exc)


def _record(conn, gate: str, *, rejected: bool, value, day: str) -> None:
    now = datetime.now(timezone.utc).isoformat()
    with conn:
        conn.execute(
            "INSERT INTO gate_log (gate, day, rejected, passed, samples, updated_at) "
            "VALUES (?,?,?,?,'[]',?) ON CONFLICT(gate, day) DO UPDATE SET "
            "rejected = rejected + excluded.rejected, passed = passed + excluded.passed, "
            "updated_at = excluded.updated_at",
            (gate, day, 1 if rejected else 0, 0 if rejected else 1, now),
        )
        if not rejected or value is None:
            return
        row = conn.execute(
            "SELECT samples FROM gate_log WHERE gate=? AND day=?", (gate, day)
        ).fetchone()
        try:
            samples = json.loads(row["samples"]) if row else []
        except (TypeError, ValueError):
            samples = []
        if len(samples) >= MAX_SAMPLES:
            return
        text = str(value)[:_MAX_SAMPLE_CHARS]
        if text in samples:                        # distinct examples teach more than repeats
            return
        samples.append(text)
        conn.execute(
            "UPDATE gate_log SET samples=? WHERE gate=? AND day=?",
            (json.dumps(samples), gate, day),
        )


def report(conn: sqlite3.Connection, *, days: int = 7) -> list[dict]:
    """Per-gate totals over the window, worst reject-rate first — what `locus gates` prints."""
    try:
        rows = conn.execute(
            """
            SELECT gate,
                   SUM(rejected) AS rejected,
                   SUM(passed)   AS passed,
                   MIN(day)      AS first_day,
                   MAX(day)      AS last_day
            FROM gate_log
            WHERE day >= date('now', ?)
            GROUP BY gate
            """,
            (f"-{int(days)} days",),
        ).fetchall()
    except sqlite3.OperationalError:               # table not migrated yet
        return []

    out: list[dict] = []
    for r in rows:
        rejected, passed = r["rejected"] or 0, r["passed"] or 0
        total = rejected + passed
        samples: list[str] = []
        for s in conn.execute(
            "SELECT samples FROM gate_log WHERE gate=? AND day >= date('now', ?) "
            "ORDER BY day DESC",
            (r["gate"], f"-{int(days)} days"),
        ):
            try:
                for item in json.loads(s["samples"]):
                    if item not in samples:
                        samples.append(item)
            except (TypeError, ValueError):
                continue
        out.append(
            {
                "gate": r["gate"],
                "rejected": rejected,
                "passed": passed,
                "total": total,
                "reject_rate": (rejected / total) if total else 0.0,
                "first_day": r["first_day"],
                "last_day": r["last_day"],
                "samples": samples[:MAX_SAMPLES],
            }
        )
    # A gate rejecting everything is the one worth looking at, so it sorts first.
    out.sort(key=lambda d: (-d["reject_rate"], -d["rejected"]))
    return out


def render(rows: list[dict], *, days: int = 7) -> str:
    """The report as text. Says so plainly when a gate has admitted nothing at all."""
    if not rows:
        return (
            f"No gate decisions recorded in the last {days} day(s).\n"
            "Gates record as the passes that use them run; give it a night."
        )
    out = [f"GATE LOG — last {days} day(s)", "=" * 60]
    for r in rows:
        pct = f"{r['reject_rate'] * 100:5.1f}%"
        out.append(f"\n{r['gate']}")
        out.append(
            f"  rejected {r['rejected']:>7,} of {r['total']:>7,}  ({pct})   "
            f"{r['first_day']}..{r['last_day']}"
        )
        # THE HEADLINE THIS EXISTS FOR. A gate that let nothing through is indistinguishable, in
        # every other surface, from a subject with nothing to say.
        if r["total"] and r["passed"] == 0:
            out.append("  ** ADMITTED NOTHING — a dead gate looks exactly like an empty subject")
        for s in r["samples"]:
            out.append(f"    rejected: {s}")
    return "\n".join(out)
