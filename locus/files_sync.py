"""The laptop's `~/files` and the tablet keep one folder tree (files-and-chat M1, 9 Nov 2026).

Both trees are `<Area>/<folder>/...`, where `<folder>` is the registry's `folder`, the same name on
both sides. Only folders are kept in step, never file contents (F3: copying content is heuris's
job). The rules, his on 9 Nov ("the shape of the tablet and computer trees are identical, and they
are perfectly linked... We also need a way to handle new folders on both"):

* A folder below a thing's folder (level 3 and deeper), new on one side, is made on the other.
* A thing's folder (level 2) the registry names but one side lacks is made there: the registry is
  the authority on what exists.
* A level-2 folder the registry does NOT name is a question for him, never mirrored: register it
  as a course, project or company, or remove it. Personal has no registry entries, so every level
  below it mirrors freely, and so does `general` in every area: the area's own things, the folder
  kairos calls "Careers (general)".
* A name with a space or a capital is a question too, with the name it would take. Nothing is
  made under a name the other side could not use, since a mirrored copy would carry the bad name
  across and the two trees would then disagree about what the folder is called.
* Nothing is ever deleted or renamed by a sync. A rename shows up as one folder gone and one new,
  and the new one is put to him like any other.

Where it runs: on the LAPTOP, which has `~/files`. The tablet is reached through the server,
which holds the rmapi login: `locus files tablet` there prints the tablet's folders (one cloud
call, which also refreshes capture's tree cache) and `locus files tablet-mkdir` makes some. The
questions are written to `$LOCUS_STATE_DIR/files.json`, which `locus files questions` prints and
the Home chat reads.

A laptop folder that is a git repository or a Python environment is mirrored itself but not
looked inside: the tablet wants `b1-coding-practical-mt24`, not its `__pycache__`.
"""
from __future__ import annotations

import json
import os
import re
import shlex
import subprocess
import time
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

AREAS = ("Studies", "Projects", "Careers", "Personal")
FILES_ROOT = Path("~/files")
REGISTRY_PATH = Path(__file__).resolve().parents[1] / "taxonomy" / "registry.toml"
_NAME = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
# Never looked inside on the laptop: their contents are code and tooling, not things you file.
_OPAQUE_MARKERS = (".git", "pyvenv.cfg")
_SKIP = {"__pycache__", "node_modules", ".venv", "venv", "target", "build", "dist"}
GENERAL = "general"   # in every area: the area's own things, mirrored without a registry entry


def is_folder_name(name: str) -> bool:
    """Lowercase words joined by hyphens: the same rule as the registry's `folder` (F1)."""
    return bool(_NAME.match(name))


def suggest(name: str) -> str:
    """The name a folder would take under F1: `B1 Engineering Computation` -> `b1-engineering-computation`."""
    s = re.sub(r"[^a-z0-9]+", "-", name.casefold()).strip("-")
    return s or "folder"


def registry_folders(path: Path | str = REGISTRY_PATH) -> dict[str, set[str]]:
    """Area -> the registry's folder names in it."""
    data = tomllib.loads(Path(path).read_text())
    out: dict[str, set[str]] = {a: set() for a in AREAS}
    for e in data.get("projects", []):
        if e.get("area") in out and e.get("folder"):
            out[e["area"]].add(e["folder"])
    return out


def done_folders(path: Path | str = REGISTRY_PATH) -> set[str]:
    """`Area/folder` of every finished project (registry `state = "done"`, 10 Nov). Its folder is
    known, so never a question, but it is not made where it does not exist: a finished project
    with no notes would only leave two empty folders behind."""
    data = tomllib.loads(Path(path).read_text())
    return {f"{e['area']}/{e['folder']}" for e in data.get("projects", [])
            if e.get("state") == "done" and e.get("area") and e.get("folder")}


# --- the two trees, as sets of "Area/a/b" paths ---------------------------------------------------

def laptop_folders(root: Path | str = FILES_ROOT) -> set[str]:
    root = Path(root).expanduser()
    out: set[str] = set()
    for area in AREAS:
        base = root / area
        if not base.is_dir():
            continue
        out.add(area)
        for dirpath, dirnames, _ in os.walk(base):
            here = Path(dirpath)
            rel = here.relative_to(root).as_posix()
            out.add(rel)
            if here != base and any((here / m).exists() for m in _OPAQUE_MARKERS):
                dirnames[:] = []
                continue
            dirnames[:] = sorted(d for d in dirnames if not d.startswith(".") and d not in _SKIP)
    return out


def tablet_folders(tree: dict) -> set[str]:
    """Every live folder under the four areas in an rmapi tree cache.

    Raises ValueError on a cache of the wrong shape, as capture does: an unreadable tablet must
    never read as an empty one, or a sync would conclude every folder is new on the laptop.
    """
    docs = tree.get("Docs")
    if not isinstance(docs, list):
        raise ValueError("rmapi tree cache has no 'Docs' list: its format has changed")
    by_id = {d["DocumentID"]: d for d in docs if isinstance(d, dict) and "DocumentID" in d}

    def path_of(doc_id: str) -> str | None:
        names: list[str] = []
        seen: set[str] = set()
        cur: str | None = doc_id
        while cur:
            if cur == "trash" or cur in seen or cur not in by_id:
                return None
            seen.add(cur)
            meta = by_id[cur].get("Metadata") or {}
            if meta.get("deleted"):
                return None
            names.append(meta.get("visibleName") or "")
            cur = meta.get("parent") or None
        return "/".join(reversed(names))

    out: set[str] = set()
    for doc_id, d in by_id.items():
        if (d.get("Metadata") or {}).get("type") != "CollectionType":
            continue
        p = path_of(doc_id)
        if p and p.split("/", 1)[0] in AREAS:
            out.add(p)
    return out


# --- the comparison -------------------------------------------------------------------------------

@dataclass
class Plan:
    to_laptop: list[str] = field(default_factory=list)     # make these in ~/files
    to_tablet: list[str] = field(default_factory=list)     # make these on the tablet
    questions: list[dict] = field(default_factory=list)    # put to him, never acted on
    renames: list[tuple[str, str]] = field(default_factory=list)  # tablet folders awaiting F1's rename


def plan(laptop: set[str], tablet: set[str], registry: dict[str, set[str]],
         renames: list[tuple[str, str]] = (), done: set[str] = frozenset()) -> Plan:
    """What to make where, and what to ask. `renames` are the tablet's level-2 folders still
    under their old names (`rename_plan`): until `locus files tablet-rename` has run, the new
    name is NOT made beside the old one, which would leave two folders for one thing, and the
    old one is reported as a pending rename rather than as a question."""
    out = Plan(renames=list(renames))
    asked: set[str] = set(o for o, _ in renames)
    pending_new = {n for _, n in renames}

    def ask(path: str, side: str, why: str) -> None:
        if path in asked:
            return
        asked.add(path)
        name = path.rsplit("/", 1)[-1]
        q = {"path": path, "on": side, "why": why}
        if why == "name":
            q["suggest"] = path.rsplit("/", 1)[0] + "/" + suggest(name)
        out.questions.append(q)

    def blocked(path: str) -> bool:
        """A folder under a questioned one waits for the answer."""
        return any(path.startswith(a + "/") for a in asked)

    # The same folder under its old name on one side and its new one on the other (the tablet's
    # `Technology Strategy`, the laptop's `technology-strategy`) is ONE folder waiting for a
    # rename, never a reason to make a second one.
    def renamed_twin(p: str, other: set[str]) -> str | None:
        head, _, name = p.rpartition("/")
        twin = f"{head}/{suggest(name)}" if head else suggest(name)
        return twin if twin != p and twin in other else None

    twins = {renamed_twin(p, laptop) for p in tablet - laptop} | {renamed_twin(p, tablet) for p in laptop - tablet}
    twins.discard(None)

    # the four areas, and the registry's own folders, exist on both sides
    for area in registry:
        if area not in laptop:
            out.to_laptop.append(area)
        if area not in tablet:
            out.to_tablet.append(area)
    for area, names in registry.items():
        for f in sorted(names):
            p = f"{area}/{f}"
            if p in done and p not in laptop and p not in tablet:
                continue
            if p not in laptop:
                out.to_laptop.append(p)
            if p not in tablet and p not in pending_new:
                out.to_tablet.append(p)

    for here, there, side, dest in ((laptop, tablet, "laptop", out.to_tablet),
                                    (tablet, laptop, "tablet", out.to_laptop)):
        for p in sorted(here - there, key=lambda s: (s.count("/"), s)):
            parts = p.split("/")
            if len(parts) == 1 or blocked(p) or p in pending_new or p in twins:
                continue
            if not is_folder_name(parts[-1]):
                ask(p, side, "name")
                continue
            if (len(parts) == 2 and parts[0] != "Personal" and parts[1] != GENERAL
                    and parts[1] not in registry.get(parts[0], set())):
                ask(p, side, "unregistered")
                continue
            if p not in dest:
                dest.append(p)
    # A registered folder present on both sides is fine; one with a bad name on BOTH sides (the
    # tablet before its rename) is reported too, so the rename is never forgotten.
    for p in sorted(laptop & tablet):
        parts = p.split("/")
        if len(parts) >= 2 and not is_folder_name(parts[-1]) and not blocked(p):
            ask(p, "both", "name")
    # parents before children, so one pass makes every level
    out.to_laptop.sort(key=lambda s: (s.count("/"), s))
    out.to_tablet.sort(key=lambda s: (s.count("/"), s))
    return out


# --- running it -----------------------------------------------------------------------------------

def server_command(args: list[str]) -> list[str]:
    host = os.environ.get("LOCUS_HOST", "compute-node")
    d = os.environ.get("LOCUS_DIR", "/home/alec/server-projects/locus")
    # uv by its full path: a non-interactive ssh shell does not read the profile that puts
    # ~/.local/bin on PATH ("zsh: command not found: uv", 9 Nov).
    uv = os.environ.get("LOCUS_SERVER_UV", "~/.local/bin/uv")
    remote = f"cd {shlex.quote(d)} && {uv} run --quiet locus files " + " ".join(shlex.quote(a) for a in args)
    return ["ssh", "-o", "ConnectTimeout=10", "-o", "BatchMode=yes", host, remote]


def fetch_tablet() -> set[str]:
    r = subprocess.run(server_command(["tablet"]), capture_output=True, text=True, timeout=120)
    if r.returncode != 0:
        raise RuntimeError(f"the server could not list the tablet: {(r.stderr or r.stdout).strip()[:300]}")
    return set(json.loads(r.stdout))


def make_on_tablet(paths: list[str]) -> None:
    if not paths:
        return
    r = subprocess.run(server_command(["tablet-mkdir", *paths]), capture_output=True, text=True, timeout=600)
    if r.returncode != 0:
        raise RuntimeError(f"the server could not make tablet folders: {(r.stderr or r.stdout).strip()[:300]}")


def state_path() -> Path:
    base = os.environ.get("LOCUS_STATE_DIR") or "~/.local/state/locus"
    return Path(base).expanduser() / "files.json"


def sync(*, dry_run: bool = False, root: Path | str = FILES_ROOT) -> Plan:
    """One sync, laptop side. Writes the questions to `files.json` even on a dry run, since they
    are true either way; a failure to reach the tablet raises and writes nothing."""
    root = Path(root).expanduser()
    tablet = fetch_tablet()
    p = plan(laptop_folders(root), tablet, registry_folders(), rename_plan(tablet), done=done_folders())
    if not dry_run:
        for rel in p.to_laptop:
            (root / rel).mkdir(parents=True, exist_ok=True)
        make_on_tablet(p.to_tablet)
    s = state_path()
    s.parent.mkdir(parents=True, exist_ok=True)
    s.write_text(json.dumps({"at": time.time(), "questions": p.questions,
                             "renames_pending": p.renames,
                             "made_laptop": [] if dry_run else p.to_laptop,
                             "made_tablet": [] if dry_run else p.to_tablet}, indent=1))
    return p


def questions() -> list[dict]:
    try:
        return json.loads(state_path().read_text()).get("questions", [])
    except (OSError, ValueError):
        return []


def describe(q: dict) -> str:
    where = {"laptop": "on the laptop", "tablet": "on the tablet", "both": "on both"}[q["on"]]
    if q["why"] == "name":
        return f"{q['path']} ({where}): not a folder name; rename it {q['suggest']}?"
    return f"{q['path']} ({where}): not in the registry; register it, or remove the folder?"


# --- server side: the tablet ----------------------------------------------------------------------

def tablet_now(runner, *, tree_cache: Path | str = Path("~/.cache/rmapi/tree.cache")) -> set[str]:
    """One `rmapi ls /` (which refreshes the tree cache, as capture's read does), then the
    folders. A failed call raises rather than reading a stale cache as today's tablet."""
    code, out, err = runner(["ls", "/"])
    if code != 0:
        raise RuntimeError(f"rmapi ls / failed: {(err or out).strip()[:300]}")
    return tablet_folders(json.loads(Path(tree_cache).expanduser().read_text()))


def deep_rename_plan(tablet: set[str]) -> list[tuple[str, str]]:
    """Every tablet folder below a thing's folder whose name breaks F1, as (old, new) with the
    suggested name, DEEPEST FIRST: rmapi renames by path, so a child is renamed while its parent
    still has the name the listing gave. A target that already exists is left alone (it would be
    a merge, not a rename) and so is anything under a folder sync is asking about."""
    out: list[tuple[str, str]] = []
    for p in sorted(tablet, key=lambda s: (-s.count("/"), s)):
        parts = p.split("/")
        if len(parts) < 3 or is_folder_name(parts[-1]):
            continue
        new = "/".join(parts[:-1] + [suggest(parts[-1])])
        if new not in tablet:
            out.append((p, new))
    return out


def rename_plan(tablet: set[str], registry_path: Path | str = REGISTRY_PATH) -> list[tuple[str, str]]:
    """The tablet's level-2 folders still under their old names (an entry's `name` or `company`),
    as (old path, new path). Two old folders for one new name are both left alone and reported
    by the caller, since merging folders is not a rename."""
    data = tomllib.loads(Path(registry_path).read_text())
    old_to_new: dict[tuple[str, str], str] = {}
    for e in data.get("projects", []):
        if e.get("kind") == "repo" and e.get("course"):
            continue
        for old in {e.get("name"), e.get("company")} - {None}:
            old_to_new[(e["area"], old.casefold())] = e["folder"]
    out: list[tuple[str, str]] = []
    for p in sorted(tablet):
        parts = p.split("/")
        if len(parts) != 2:
            continue
        new = old_to_new.get((parts[0], parts[1].casefold()))
        if new and new != parts[1] and f"{parts[0]}/{new}" not in tablet:
            out.append((p, f"{parts[0]}/{new}"))
    targets = [n for _, n in out]
    return [(o, n) for o, n in out if targets.count(n) == 1]
