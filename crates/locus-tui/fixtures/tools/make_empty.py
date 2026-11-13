"""Write the `empty` set from the default one: every section empty (the registry and taxonomy are
the shared ones in taxonomy/ at the repository root, read by every set).

    python3 make_empty.py

Every screen must draw with nothing in it (spec 1.3).
"""

import json
import shutil
import tomllib
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
    write("home.json", {"mail": {}})
    write("work.json", {"week": [], "blocks_kept": 0, "blocks_planned": 0, "streak_days": 0,
                        "chart": {"from": "2026-08-30", "series": []}})
    write("system.json", {"running": [], "recent": [], "done_today": 0, "failed_today": 0})
    write("usage.json", {"week_pct": 0.0, "cap_pct": 15.0, "projected_pct": 0.0,
                         "window": {"you_pct": 0.0, "locus_pct": 0.0, "stop_at_pct": 60.0,
                                    "resets": "2026-10-24T23:00:00+01:00"}})
    write("approve.json", {"groups": []})
    write("autolog.json", {"entries": []})
    write("learning.json", {"goals": [], "tracks": [], "sheets": []})
    write("tasks.json", {"tasks": []})
    registry = tomllib.loads((HERE.parent.parent.parent / "taxonomy" / "registry.toml").read_text())
    for p in registry["projects"]:
        if not p["active"]:
            continue
        pid = p["id"]
        write(f"projects/{pid}/view.json", {"project": pid})


if __name__ == "__main__":
    main()
