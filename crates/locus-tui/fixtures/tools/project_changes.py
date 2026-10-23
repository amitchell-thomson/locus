"""Write a project's Changes timeline from its real git history, once.

    python3 project_changes.py <fixture project dir> <repo> --since 2026-10-19 [--until DATE]
        [--last-seen ISO] [--routine "subject prefix=name" ...]

Until change records exist (Slice 2), an entry is a commit: its subject as the line, its message
body as the detail (trailers such as Co-Authored-By dropped). Answers already in the project's
answered.json join the timeline as answers. A commit whose subject starts with a --routine prefix
is marked routine, so Changes folds it with the others of its name.
"""

import argparse
import json
import subprocess
from pathlib import Path

SEP = "\x1e"


def commits(repo: str, since: str, until: str | None) -> list[dict]:
    args = ["git", "-C", repo, "log", f"--since={since}", f"--format=%cI%x1f%s%x1f%b{SEP}"]
    if until:
        args.insert(4, f"--until={until}")
    out = subprocess.run(args, capture_output=True, text=True, check=True).stdout
    found = []
    for rec in out.split(SEP):
        rec = rec.strip("\n")
        if not rec:
            continue
        at, subject, body = rec.split("\x1f", 2)
        paras = []
        for p in body.strip().split("\n\n"):
            lines = [l for l in p.splitlines() if not l.startswith(("Co-Authored-By:", "Signed-off-by:"))]
            text = " ".join(l.strip() for l in lines).strip()
            if text:
                paras.append(text)
        found.append({"kind": "commits", "text": subject, "at": at, "detail": paras})
    return found


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("project_dir")
    ap.add_argument("repo")
    ap.add_argument("--since", required=True)
    ap.add_argument("--until")
    ap.add_argument("--last-seen")
    ap.add_argument("--routine", action="append", default=[])
    a = ap.parse_args()
    pdir = Path(a.project_dir)
    project = pdir.name
    entries = commits(a.repo, a.since, a.until)
    for spec in a.routine:
        prefix, name = spec.split("=", 1)
        for e in entries:
            if e["text"].startswith(prefix):
                e["routine"] = name
    answered = pdir / "answered.json"
    if answered.exists():
        for x in json.loads(answered.read_text())["items"]:
            entries.append({"kind": "answer", "text": x["title"], "at": x["at"], "detail": x["body"]})
    entries.sort(key=lambda e: e["at"], reverse=True)
    out = {"project": project, "since": a.since}
    if a.last_seen:
        out["last_seen"] = a.last_seen
    out["entries"] = entries
    (pdir / "changes.json").write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n")
    print(f"{pdir}/changes.json: {len(entries)} entries")


if __name__ == "__main__":
    main()
