"""Canvas deadlines as task proposals in Approve (spec P, step P6; P-D7).

Each assessed assignment of this year's courses with a due date still ahead, and nothing handed
in, becomes a kairos task that WAITS for him: posted with `x-kairos-review`, which only raises a
change to approval (kairosd `Engine::review`), so it sits in Approve with the reason "proposed for
your approval: Canvas deadline". Approved, it is a task like any other: its title is the
assignment's, its locus the course, its due date Canvas's, 5 h by default (a sheet, the
scheduler's default) for him to change. Refused, it never exists, and it is not proposed again.

Not proposed:
- a due date already past, or work already submitted (B1's Numerical Algorithms practical, 5 Nov);
- an assignment of a B3 project he is not on (`taxonomy/canvas-skip.toml`: Canvas shows every
  third-year the cultured-meat logbooks, 15 Nov);
- an assignment with no due date (B15's lab answers, 15 Nov);
- one kairos already has: a task for the same course due the same day (the EngExams mail, or him);
- one proposed before, whatever he decided: `data/canvas/deadlines.json`, keyed by assignment.

A tutor's sheet deadline from mail stays keryx's job (P-D7).
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import tomllib
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from zoneinfo import ZoneInfo

from locus.canvas_files import active_folders, skipped_modules
from locus.canvas_pull import MIRROR

STATE = MIRROR / "deadlines.json"
ESTIMATE_MIN = 300
LONDON = ZoneInfo("Europe/London")


@dataclass
class Proposal:
    assignment: int
    course: str          # registry id
    title: str
    due: str             # ISO, UTC


@dataclass
class DeadlinePlan:
    propose: list[Proposal] = field(default_factory=list)
    submitted: list[str] = field(default_factory=list)
    past: list[str] = field(default_factory=list)
    not_his: list[str] = field(default_factory=list)
    no_date: list[str] = field(default_factory=list)
    in_kairos: list[str] = field(default_factory=list)
    proposed_before: int = 0


def _slug(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


def _not_his(name: str, skip: set[str]) -> bool:
    """An assignment names its project: the first three words of a skipped module's folder
    ("cultured-meat-production") in the assignment's name mean it is that project's."""
    s = _slug(name)
    return any("-".join(f.split("-")[:3]) in s for f in skip)


def _day(iso: str) -> str:
    return datetime.fromisoformat(iso.replace("Z", "+00:00")).astimezone(LONDON).date().isoformat()


def _due_day(due) -> str | None:
    """A kairos task's due date as a London day: `{"type": "at", "value": "...Z"}` or
    `{"type": "date", "value": "2026-11-19"}`."""
    if not isinstance(due, dict) or not due.get("value"):
        return None
    return _day(due["value"]) if due.get("type") == "at" else str(due["value"])


def plan(assignments: dict[str, list[dict]], tasks: list[dict], state: dict, *, now: datetime,
         skip: dict[str, set[str]] | None = None) -> DeadlinePlan:
    """`assignments`: registry id -> Canvas assignments (with `submission`); `tasks`: kairos's
    tasks, open and done, as `/api/tasks` gives them."""
    skip = skipped_modules() if skip is None else skip
    p = DeadlinePlan()
    have = {(t["locus"], d) for t in tasks if t.get("locus") and (d := _due_day(t.get("due")))}
    for rid, items in sorted(assignments.items()):
        for a in items:
            label = f"{rid}: {a.get('name', '').strip()}"
            if not a.get("published", True):
                continue
            if str(a["id"]) in state:
                p.proposed_before += 1
                continue
            if _not_his(a.get("name", ""), skip.get(rid, set())):
                p.not_his.append(label)
                continue
            if not a.get("due_at"):
                p.no_date.append(label)
                continue
            sub = a.get("submission") or {}
            if sub.get("submitted_at") or sub.get("workflow_state") in ("submitted", "graded", "pending_review"):
                p.submitted.append(label)
                continue
            due = datetime.fromisoformat(a["due_at"].replace("Z", "+00:00"))
            if due <= now:
                p.past.append(label)
                continue
            if (rid, _day(a["due_at"])) in have:
                p.in_kairos.append(label)
                continue
            p.propose.append(Proposal(int(a["id"]), rid, a["name"].strip(), a["due_at"]))
    return p


# ── the live ends ──────────────────────────────────────────────────────────────────────────────

def fetch_assignments(client, courses: dict[str, dict]) -> dict[str, list[dict]]:
    out = {}
    for rid in active_folders(courses):
        out[rid] = client.get(f"courses/{courses[rid]['canvas']}/assignments",
                              [("include[]", "submission"), ("per_page", "100")])
    return out


def _kairos() -> tuple[str, str]:
    cfg = tomllib.loads(Path("~/.config/kairos/config.toml").expanduser().read_text())["daemon"]
    tok = subprocess.run(cfg["token_cmd"], shell=True, capture_output=True, text=True, check=True).stdout.strip()
    return cfg["url"].rstrip("/"), tok


def _call(method: str, path: str, body: dict | None = None, headers: dict | None = None):
    url, tok = _kairos()
    req = urllib.request.Request(url + path, method=method,
                                 data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json",
                                          "x-kairos-author": "claude", **(headers or {})})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read() or b"null")


def kairos_tasks() -> list[dict]:
    got = _call("GET", "/api/tasks?status=all")
    return got if isinstance(got, list) else got.get("tasks", [])


def propose(pr: Proposal, course_name: str) -> dict:
    """One pending kairos changeset creating the task. Returns kairosd's write result."""
    body = {"title": pr.title, "key": "Studies", "locus": pr.course, "estimate_min": ESTIMATE_MIN,
            "due": pr.due, "plan": True,
            "notes": f"From Canvas ({course_name}), assignment {pr.assignment}."}
    resp = _call("POST", "/api/tasks", body, {"x-kairos-review": f"Canvas deadline, {course_name}"})
    return resp["changesets"][0]


def load_state(path: Path = STATE) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


def save_state(state: dict, path: Path = STATE) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".part")
    tmp.write_text(json.dumps(state, indent=1, sort_keys=True))
    os.replace(tmp, path)


def run(client, courses: dict[str, dict], *, dry_run: bool = False, log=print) -> DeadlinePlan:
    state = load_state()
    p = plan(fetch_assignments(client, courses), kairos_tasks(), state, now=datetime.now(timezone.utc))
    if dry_run:
        return p
    for pr in p.propose:
        cs = propose(pr, courses[pr.course].get("name", pr.course))
        state[str(pr.assignment)] = {"course": pr.course, "title": pr.title, "due": pr.due,
                                     "changeset": cs.get("changeset_id"), "status": cs.get("status")}
        save_state(state)
        log(f"proposed  {pr.course}: {pr.title}, due {_day(pr.due)} ({cs.get('status')})")
    return p
