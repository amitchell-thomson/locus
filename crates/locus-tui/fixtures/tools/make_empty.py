"""Write the `empty` set from the default one: the same registry and taxonomy, every section empty.

    python3 make_empty.py

Every screen must draw with nothing in it (spec 1.3). Projects keep their decisions, answered
and changes files, emptied, so an empty list is drawn rather than "this project has no log".
"""

import json
import shutil
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
SRC, DST = HERE / "default", HERE / "empty"
NOW = "2026-10-24T21:40:00+01:00"


def write(rel: str, data: object) -> None:
    path = DST / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")


def main() -> None:
    if DST.exists():
        shutil.rmtree(DST)
    write("manifest.json", {"name": "empty", "now": NOW,
                            "description": "Every section empty: the same registry as default, with nothing in it."})
    for f in ("taxonomy.json", "registry.json"):
        write(f, json.loads((SRC / f).read_text()))
    write("home.json", {"chores_fit": 0, "mail": {"need_reply": 0, "deadlines_proposed": 0}})
    write("work.json", {"week": [], "blocks_kept": 0, "blocks_planned": 0, "streak_days": 0,
                        "chart": {"from": "2026-08-30", "series": []}})
    write("system.json", {"running": [], "recent": [], "done_today": 0, "failed_today": 0})
    write("usage.json", {"week_pct": 0.0, "cap_pct": 15.0, "projected_pct": 0.0,
                         "window": {"you_pct": 0.0, "locus_pct": 0.0, "stop_at_pct": 60.0,
                                    "resets": "2026-10-24T23:00:00+01:00"}})
    write("explore.json", {"queue": [], "queue_cap": 7, "ideas": [], "reading": [], "reading_cap": 5})
    write("approve.json", {"groups": []})
    write("autolog.json", {"entries": []})
    write("search.json", {"query": "", "scope": "everything", "hits": []})
    write("learning.json", {"goals": [], "tracks": [], "sheets": []})
    write("review.json", {"from": "2026-10-19", "to": "2026-10-25", "steps": ["todo"] * 5,
                          "week": {"blocks_kept": 0, "blocks_planned": 0, "budget_pct": 0.0},
                          "explore": {"left": 0}, "locus": {}})
    registry = json.loads((SRC / "registry.json").read_text())
    for p in registry["projects"]:
        if not p["active"]:
            continue
        pid = p["id"]
        write(f"projects/{pid}/view.json", {"project": pid})
        write(f"projects/{pid}/decisions.json", {"project": pid, "log": "docs/decisions/", "items": []})
        write(f"projects/{pid}/answered.json", {"project": pid, "items": []})
        write(f"projects/{pid}/changes.json", {"project": pid, "since": "2026-10-19", "entries": []})


if __name__ == "__main__":
    main()
