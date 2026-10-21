"""Fill each lane's change records in a project fixture from the real git history, once.

    python3 lane_records.py <fixture view.json> <repo> [--trunk main]

Until change records exist (Slice 2), a lane's records are its commits: date and subject, oldest
first. The range is found the way the branch was finished: a merge on the trunk whose subject
names the branch (or is the lane's own merge subject) gives M^1..M^2; a branch fast-forwarded
into the trunk is read from its reflog's creation point; an open branch is trunk..branch.
"""

import argparse
import json
import re
import subprocess


def git(repo: str, *args: str) -> str:
    r = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else ""


def commits(repo: str, rng: str) -> list[dict]:
    out = git(repo, "log", "--reverse", "--format=%cs%x09%s", rng)
    return [{"date": d, "text": s} for d, s in (line.split("\t", 1) for line in out.splitlines() if line)]


def find_range(repo: str, trunk: str, branch: str) -> str | None:
    merges = git(repo, "log", "--first-parent", "--merges", "--format=%H%x09%s", trunk).splitlines()
    short = branch.removeprefix("worktree-")
    for line in merges:
        sha, subject = line.split("\t", 1)
        names = re.findall(r"from [^/\s]+/(\S+)|Merge (?:remote-tracking )?branch '([^']+)'", subject)
        named = {n for pair in names for n in pair if n}
        if branch in named or short in named or subject == branch:
            return f"{sha}^1..{sha}^2"
    for ref in (branch, f"origin/{branch}"):
        if git(repo, "rev-parse", "--verify", "-q", ref).strip():
            if git(repo, "merge-base", "--is-ancestor", ref, trunk) == "" and subprocess.run(
                ["git", "-C", repo, "merge-base", "--is-ancestor", ref, trunk]
            ).returncode != 0:
                return f"{trunk}..{ref}"
            created = git(repo, "reflog", "show", "--format=%H %gs", ref).splitlines()
            base = [l.split()[0] for l in created if "Created from" in l]
            if base:
                return f"{base[-1]}..{ref}"
    return None


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("view")
    ap.add_argument("repo")
    ap.add_argument("--trunk", default="main")
    a = ap.parse_args()
    v = json.load(open(a.view))
    found = 0
    for lane in v["graph"]["lanes"]:
        rng = find_range(a.repo, a.trunk, lane["branch"])
        lane["records"] = commits(a.repo, rng)[-40:] if rng else []
        found += bool(lane["records"])
    with open(a.view, "w") as f:
        json.dump(v, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"{a.view}: records for {found} of {len(v['graph']['lanes'])} lanes")


if __name__ == "__main__":
    main()
