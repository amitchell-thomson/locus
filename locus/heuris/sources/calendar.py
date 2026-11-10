"""Calendar and task items from kairosd's own API (H1.5).

kairosd is the single reader and writer of the calendar (kairos CLAUDE.md), so heuris reads its
`/api/events` and `/api/tasks` rather than parsing the vdir itself: recurrence, overrides and the
Family overlay are already resolved there, and a second iCalendar reader would be a second
opinion about what is on a day.

Measured 10 Nov: 461 occurrences from 2023 to 2027 (Family 231, Studies 123, the rest by area)
and 41 tasks, so everything is indexed, one item per occurrence (a lecture series is many items,
which is what `timeline` needs). The API refuses a window over 400 days, so it is read a year
at a time. Attendees are not in kairosd's API and are not indexed.
"""

from __future__ import annotations

import json
import os
import subprocess
import urllib.request
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Iterator

from locus.heuris import registry
from locus.heuris.index import Item

FIRST_YEAR = 2015


def _base() -> str:
    url = os.environ.get("KAIROS_URL")
    if url:
        return url.rstrip("/")
    ip = subprocess.run(["tailscale", "ip", "-4"], capture_output=True, text=True, timeout=10).stdout.split()
    return f"http://{ip[0] if ip else '127.0.0.1'}:7420"


def _token() -> str:
    return Path(os.environ.get("KAIROS_TOKEN_FILE", "~/.config/kairos/secrets/api-token")).expanduser().read_text().strip()


def _get(base: str, token: str, path: str):
    req = urllib.request.Request(f"{base}{path}", headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def _start(span: dict) -> datetime:
    s = span.get("start")
    if span.get("type") == "all_day" or (isinstance(s, str) and len(s) == 10):
        d = date.fromisoformat(s)
        return datetime(d.year, d.month, d.day, tzinfo=timezone.utc)
    return datetime.fromisoformat(str(s).replace("Z", "+00:00"))


def _when(span: dict) -> str:
    s, e = span.get("start"), span.get("end")
    return f"{s} to {e}" if e else str(s)


# What a Family-calendar event says about itself, in its title and first line (H1 fix 1, 10 Nov):
# Q1, 35 and P1 described Callum's personal training and a family flight as Alec's own plans,
# because nothing in the item said whose it was beyond an area label the answer never read.
FAMILY_NOTE = "On the FAMILY calendar (shared with his family): someone in the family's, not necessarily Alec's."


def event_item(e: dict) -> Item:
    rid = e.get("recurrence_id") or e["span"].get("start")
    family = e.get("source") == "family"
    area = "Family" if family else e.get("key")
    title = e.get("title", "")
    parts = [title, f"When: {_when(e['span'])}"]
    if family:
        title = f"[family] {title}"
        parts.insert(0, FAMILY_NOTE)
    if e.get("location"):
        parts.append(f"Where: {e['location']}")
    if e.get("description"):
        parts.append(e["description"])
    return Item(
        ref=f"event:{e['source']}/{e['uid']}#{rid}",
        kind="event",
        at=_start(e["span"]),
        title=title,
        text="\n".join(parts),
        area=area,
        parent=f"event:{e['source']}/{e['uid']}",
        meta={"source": e.get("source"), "key": e.get("key"), "location": e.get("location"), "family": family,
              "span": e["span"], "recurring": e.get("recurring"), "tentative": e.get("tentative")},
    )


def task_item(t: dict) -> Item:
    due = t.get("due") or {}
    when = str(due.get("value") or t.get("created_at") or "")
    if len(when) == 10:
        at = _start({"start": when})
    elif when:
        # kairos writes nanoseconds ("…20.632843778Z"); Python reads at most microseconds.
        head, _, frac = when.rstrip("Z").partition(".")
        at = datetime.fromisoformat(f"{head}.{frac[:6] or '0'}+00:00")
    else:
        at = datetime.now(timezone.utc)
    thing = t.get("locus") if t.get("locus") in registry.entries() else None
    parts = [t.get("title", ""), f"Status: {t.get('status')}"]
    if due:
        parts.append(f"Due: {due.get('value')}")
    if t.get("notes"):
        parts.append(t["notes"])
    return Item(
        ref=f"task:{t['id']}",
        kind="task",
        at=at,
        title=t.get("title", ""),
        text="\n".join(parts),
        area=registry.area_of(thing) or t.get("key"),
        thing=thing,
        meta={"status": t.get("status"), "due": due.get("value"), "key": t.get("key"),
              "remaining_min": t.get("remaining_min")},
    )


def items(base: str | None = None, token: str | None = None, *, today: date | None = None) -> Iterator[Item]:
    """Every occurrence from FIRST_YEAR to the end of next year, then every task.

    Raises on any failed read, so the caller never prunes after a partial read.
    """
    base, token = base or _base(), token or _token()
    today = today or date.today()
    for y in range(FIRST_YEAR, today.year + 2):
        resp = _get(base, token, f"/api/events?from={y}-01-01&to={y}-12-31")
        for e in resp["events"]:
            yield event_item(e)
        # 12-31 to 01-01 is excluded by both windows: read the last day on its own.
        for e in _get(base, token, f"/api/events?from={y}-12-31&to={y + 1}-01-01")["events"]:
            yield event_item(e)
    for t in _get(base, token, "/api/tasks?status=all"):
        yield task_item(t)

