"""Extract a project's branch graph from its git history, once, for the Slice 1 fixtures.

Prints JSON in the shape of `locus_contract::BranchGraph` minus the stage labels, which are
written by hand (spec 1.3, D4: change records do not exist yet). The output is pasted into a
fixture and frozen; this script is kept as the record of where the numbers came from.

    python3 branch_graph.py <repo> [--trunk main] [--lanes 8] [--since 2026-08-01]

A lane is a branch, collapsed to its fork, its commit count and its merge (or its tip, while
open). Merged branches are read from the trunk's first-parent merges; a merge subject names the
branch ("Merge pull request #29 from owner/x", "Merge branch 'x'") or, in repos that write their
own merge subjects ("merge: what it did"), the subject is the name. Open branches are local
branches not reachable from the trunk.
"""

import argparse
import json
import re
import subprocess


def git(repo: str, *args: str) -> str:
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True, text=True).stdout


def lane_name(subject: str) -> str:
    m = re.match(r"Merge pull request #\d+ from [^/]+/(\S+)", subject)
    if m:
        return m.group(1)
    m = re.match(r"Merge (?:remote-tracking )?branch '([^']+)'", subject)
    if m:
        return m.group(1).removeprefix("origin/")
    return re.sub(r"^merge:\s*", "", subject)


def commits(repo: str, rng: str) -> list[tuple[str, str, str]]:
    out = git(repo, "log", "--reverse", "--format=%h%x09%cs%x09%s", rng)
    return [tuple(line.split("\t", 2)) for line in out.splitlines() if line]  # type: ignore[misc]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("repo")
    ap.add_argument("--trunk", default="main")
    ap.add_argument("--lanes", type=int, default=8)
    ap.add_argument("--since", default="2026-01-01")
    a = ap.parse_args()

    lanes = []
    merges = git(a.repo, "log", "--first-parent", "--merges", f"--since={a.since}",
                 "--format=%H%x09%cs%x09%s", a.trunk).splitlines()
    for line in merges:
        sha, date, subject = line.split("\t", 2)
        side = commits(a.repo, f"{sha}^1..{sha}^2")
        if not side:
            continue
        lanes.append({"name": lane_name(subject), "forked": side[0][1], "commits": len(side),
                      "merged": date, "last": side[-1][2]})

    for ref in git(a.repo, "for-each-ref", "--format=%(refname:short)", "refs/heads").split():
        if ref == a.trunk:
            continue
        side = commits(a.repo, f"{a.trunk}..{ref}")
        if side and side[-1][1] >= a.since:
            lanes.append({"name": ref.removeprefix("worktree-"), "forked": side[0][1], "commits": len(side),
                          "merged": None, "last": side[-1][2], "tip": side[-1][1]})

    lanes.sort(key=lambda l: l["merged"] or l["tip"])
    first = git(a.repo, "log", "--reverse", "--first-parent", f"--since={a.since}", "--format=%cs", a.trunk).split()
    print(json.dumps({
        "trunk": a.trunk,
        "from": first[0] if first else a.since,
        "trunk_commits": len(first),
        "lanes": lanes[-a.lanes:],
    }, indent=2))


if __name__ == "__main__":
    main()
