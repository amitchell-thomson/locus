"""Index every source into heuris.db, and say how fresh each is (H1.3).

`locus heuris index [mail calendar chats repos files]` runs on the server every 15 minutes
(`locus-heuris.timer`). Each source is independent: one failing records its error and the rest
still run. `locus heuris status` prints every source with its count, its last good run and how
stale it is; Home's system section reads the same rows.

**Never prune after reading nothing.** mail, repos and calendar are full scans, so they prune
items the scan did not yield. A scan that yields NOTHING while the index holds items is treated
as a failed read (an rsync that has not arrived, an empty mount), recorded as an error, and
prunes nothing; otherwise one bad tick would empty the index and the next would rebuild it,
silently, at the cost of every embedding.
"""

from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import time
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

from locus.heuris import db as hdb
from locus.heuris.index import Stats, index, record_run

SOURCES = ("mail", "calendar", "chats", "repos", "files", "facts", "records")
# How old a source's last good run may be before status calls it stale.
STALE = {"mail": timedelta(hours=1), "calendar": timedelta(hours=1), "chats": timedelta(hours=2),
         "repos": timedelta(hours=2), "files": timedelta(hours=2),
         # H2: the ledger and the records, indexed so search and timeline find them.
         "facts": timedelta(hours=1), "records": timedelta(hours=2)}


def locus_dir() -> Path:
    return Path(os.environ.get("LOCUS_DIR") or Path(__file__).resolve().parents[2])


def inbox() -> Path:
    return locus_dir() / "data" / "heuris-in"


@dataclass
class Outcome:
    source: str
    stats: Stats | None
    error: str | None
    seconds: float


def _full_scan(conn: sqlite3.Connection, source: str, items, embed=None) -> Stats:
    items = list(items)
    have = conn.execute("SELECT COUNT(*) FROM items WHERE source=?", (source,)).fetchone()[0]
    if not items and have:
        raise RuntimeError(f"read nothing while the index holds {have} items; not pruning")
    return index(conn, source, items, embed=embed, prune=True)


def run_source(conn: sqlite3.Connection, name: str, *, embed=None) -> Outcome:
    t0 = time.monotonic()
    st, err, note = None, None, None
    try:
        if name == "mail":
            from locus.heuris.sources import mail

            root = inbox() / "mail"
            st = _full_scan(conn, name, mail.items(root), embed)
            rule = root / "_rule.json"
            if rule.exists():
                note = hashlib.sha256(rule.read_bytes()).hexdigest()[:16]
                old = conn.execute("SELECT note FROM sources WHERE name='mail'").fetchone()
                if not old or old[0] != note:
                    mail.record_rule(root)
        elif name == "repos":
            from locus.heuris.sources import repos

            st = _full_scan(conn, name, repos.items(inbox() / "repos"), embed)
        elif name == "files":
            from locus.heuris.sources import files

            st = _full_scan(conn, name, files.items(inbox() / "files"), embed)
        elif name == "calendar":
            from locus.heuris.sources import calendar

            st = _full_scan(conn, name, calendar.items(), embed)
        elif name == "facts":
            from locus.heuris import facts

            st = index(conn, name, facts.items(), embed=embed, prune=True)
        elif name == "records":
            from locus.heuris import records

            st = index(conn, name, records.items(), embed=embed, prune=True)
        elif name == "chats":
            from locus.heuris.sources import chats

            chats.purge_held_out(conn)
            files = chats.changed_files(conn, locus_dir())
            st = index(conn, name, chats.items(files), embed=embed)
            chats.mark_read(conn, files)
        else:
            raise ValueError(f"unknown source {name!r}; one of {', '.join(SOURCES)}")
    except Exception as exc:  # one source failing never stops the rest; it is recorded
        err = f"{type(exc).__name__}: {exc}"[:300]
    record_run(conn, name, st, err, note)
    return Outcome(name, st, err, time.monotonic() - t0)


def run(names: list[str] | None = None, *, conn: sqlite3.Connection | None = None, embed=None) -> list[Outcome]:
    own = conn is None
    conn = conn or hdb.connect()
    try:
        return [run_source(conn, n, embed=embed) for n in (names or list(SOURCES))]
    finally:
        if own:
            conn.close()


def status(conn: sqlite3.Connection, now: datetime | None = None) -> list[dict]:
    now = now or datetime.now(timezone.utc)
    rows = {r[0]: r for r in conn.execute(
        "SELECT name, last_run, last_ok, items, added, unchanged, removed, error FROM sources")}
    out = []
    for name in SOURCES:
        r = rows.get(name)
        if not r:
            out.append({"source": name, "items": 0, "state": "never run", "stale": True})
            continue
        _, last_run, last_ok, items, added, unchanged, removed, error = r
        ok_at = datetime.fromisoformat(last_ok) if last_ok else None
        stale = ok_at is None or now - ok_at > STALE[name]
        out.append({"source": name, "items": items, "last_run": last_run, "last_ok": last_ok,
                    "added": added, "unchanged": unchanged, "removed": removed, "error": error,
                    "stale": stale, "state": "error" if error else ("stale" if stale else "ok")})
    return out


def _ago(iso: str | None, now: datetime) -> str:
    if not iso:
        return "never"
    s = int((now - datetime.fromisoformat(iso)).total_seconds())
    return f"{s // 60} min ago" if s < 7200 else f"{s // 3600} h ago" if s < 172800 else f"{s // 86400} days ago"


def render(rows: list[dict], now: datetime | None = None) -> str:
    now = now or datetime.now(timezone.utc)
    lines = []
    for r in rows:
        line = f"{r['source']:9} {r['items']:7} items  last good {_ago(r.get('last_ok'), now):12}  {r['state'].upper() if r['state'] != 'ok' else 'ok'}"
        if r.get("last_run"):
            line += f"  (last run +{r['added']} added, {r['unchanged']} unchanged, -{r['removed']} removed)"
        if r.get("error"):
            line += f"\n          {r['error']}"
        lines.append(line)
    return "\n".join(lines)


def status_json(conn: sqlite3.Connection) -> str:
    return json.dumps(status(conn))
