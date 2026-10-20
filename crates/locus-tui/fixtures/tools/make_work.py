"""Write the default set's work.json: this week by hand, the 8-week chart from real commits.

    python3 make_work.py > ../default/work.json

The chart is the real shape of the last eight weeks: commits per repository per day, turned
into hours at half an hour a commit, capped at 9 h (transcripts will replace this in Slice 4;
commits are the one record that exists for every project now). Days after the last real
commit are left as they are. The week's spans are written by hand to show the awkward cases:
a deep block that wandered onto a second project, a late night, a day with nothing.
"""

import datetime as dt
import json
import subprocess
from pathlib import Path

HOME = Path.home() / "local-projects"
REPOS = {"quast": ["quast"], "nabla": ["nabla"], "locus": ["locus"],
         "oqts": ["oqts/oqts-platform", "oqts/oqts-site", "oqts/oxdaq-core"], "kairos": ["kairos"], "keryx": ["keryx"],
         "cv": ["cv"]}
TO = dt.date(2026, 10, 24)
FROM = TO - dt.timedelta(days=55)  # eight weeks, ending on the snapshot's Saturday

# (day, start, end, project, intensity); Monday 19 Oct is day 0.
WEEK = [
    (0, "08:30", "10:00", "quast", 3), (0, "10:00", "12:30", "quast", 4), (0, "14:00", "16:00", "nabla", 2),
    (0, "20:00", "22:30", "locus", 3),
    (1, "09:00", "12:00", "quast", 4), (1, "12:00", "12:30", "nabla", 1), (1, "17:30", "19:00", "jump", 2),
    (1, "21:00", "23:30", "locus", 4),
    (2, "10:00", "13:00", "locus", 4), (2, "13:00", "14:00", "kairos", 2), (2, "19:00", "21:00", "cv", 2),
    (3, "08:00", "09:30", "quast", 3), (3, "15:00", "18:00", "nabla", 3), (3, "18:00", "18:30", "oqts", 1),
    (3, "22:00", "24:00", "locus", 3),
    # Friday: nothing logged (a day with no work is a case the grid must draw).
    (5, "09:30", "13:30", "quast", 4), (5, "16:00", "18:00", "3yp", 2), (5, "20:30", "21:40", "locus", 3),
]


def commits_per_day(repo: str) -> dict[dt.date, int]:
    out: dict[dt.date, int] = {}
    try:
        text = subprocess.run(["git", "-C", str(HOME / repo), "log", "--all", f"--since={FROM}", "--format=%cs"],
                              check=True, capture_output=True, text=True).stdout
    except subprocess.CalledProcessError:
        return out
    for line in text.split():
        d = dt.date.fromisoformat(line)
        out[d] = out.get(d, 0) + 1
    return out


def main() -> None:
    days = [FROM + dt.timedelta(days=i) for i in range((TO - FROM).days + 1)]
    series = []
    for project, repos in REPOS.items():
        per = [commits_per_day(r) for r in repos]
        hours = [min(9.0, 0.5 * sum(p.get(d, 0) for p in per)) for d in days]
        if any(hours):
            series.append({"project": project, "hours": hours})
    monday = dt.date(2026, 10, 19)
    week = []
    for i in range(6):
        spans = [{"start": s, "end": e if e != "24:00" else "23:59", "project": p, "intensity": n}
                 for (d, s, e, p, n) in WEEK if d == i]
        week.append({"date": str(monday + dt.timedelta(days=i)), "spans": spans})
    print(json.dumps({"week": week, "blocks_kept": 8, "blocks_planned": 9, "streak_days": 4,
                      "chart": {"from": str(FROM), "series": series}}, indent=1))


if __name__ == "__main__":
    main()
