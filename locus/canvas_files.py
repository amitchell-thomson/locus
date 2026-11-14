"""The mirror's two outputs for this year's courses (spec P, step P3; P-D2, P-D5, P-D6).

1. **The library, straight from the mirror** (`queue_library`, on the server after each pull).
   The spec first had Canvas PDFs reach the library through the tablet, because capture reads
   every source PDF in `Studies/`. That route has two holes: solutions never go to the tablet
   (P-D3), so they would never be read in; and a PDF locus pushes to the tablet is recorded in
   `device_sent`, which capture treats as Claude's own document and never ingests. So every PDF,
   PowerPoint and Word file of an active course is queued from the mirror into
   `data/incoming/studies/<course folder>/<module>/<file>`, where `locus-watch` ingests it as
   coursework; the course folder in its path is what ties it to its course (`doc_thing`). A file
   whose bytes the library already has (his hand uploads, 14 Nov: B14, B15, B18) is a no-op by
   content hash. Each queued sha256 is remembered in the manifest, so it is queued once; a file
   Canvas changes is queued again as its new version. Past years stay out (his choice, 14 Nov):
   their syllabus and his notes already say what he knows.

2. **`~/files`, on the laptop** (`to_files`, run by `locus files sync`). Each file of an active
   course is copied to `~/files/Studies/<course folder>/<module>/<file>`, the folder the
   syllabus is already in. Never deletes, and never overwrites his work:
   - a file he deleted after it arrived is not brought back;
   - a file Canvas changed replaces the laptop copy only if that copy is still the bytes locus
     delivered; if he edited it, the new version lands beside it as `<name>-v2`.
   What was delivered is kept in `data/state/canvas-files.json` on the laptop.
"""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from locus.canvas_pull import MIRROR, load_manifest

LIBRARY_TYPES = (".pdf", ".pptx", ".docx")


def active_folders(courses: dict[str, dict]) -> dict[str, str]:
    """Registry id -> course folder, for this year's courses with a Canvas id."""
    return {rid: c["folder"] for rid, c in courses.items()
            if c.get("canvas") and c.get("folder") and c.get("active", True) is not False}


# ── 1. the library (server) ────────────────────────────────────────────────────────────────────

@dataclass
class Queued:
    queued: list[str] = field(default_factory=list)
    in_library: int = 0
    waiting: int = 0
    held: list[str] = field(default_factory=list)


def _held(incoming: Path, rel: str) -> bool:
    """Set aside by locus-watch before (quarantined or unsupported): never queued again, or a bad
    file would loop every half hour (capture's rule, loop_b._held_back)."""
    name = rel.rsplit("/", 1)[-1]
    for d in (".quarantine", ".unsupported"):
        held = incoming / d
        if held.exists() and any(str(p).endswith(rel) for p in held.rglob(name)):
            return True
    return False


def queue_library(courses: dict[str, dict], library_hashes: set[str], *, incoming: Path,
                  root: Path = MIRROR, dry_run: bool = False) -> Queued:
    man = load_manifest(root)
    live = active_folders(courses)
    out = Queued()
    for e in man["files"].values():
        if e["course"] not in live or e["status"] != "ok" or not e["path"].lower().endswith(LIBRARY_TYPES):
            continue
        if e.get("library") == e["sha256"]:
            continue
        if e["sha256"] in library_hashes:
            out.in_library += 1
            e["library"] = e["sha256"]
            continue
        dest = incoming / "studies" / e["path"]
        if _held(incoming, e["path"]):
            out.held.append(e["path"])
            continue
        if dest.exists():
            out.waiting += 1
            continue
        out.queued.append(e["path"])
        if dry_run:
            continue
        dest.parent.mkdir(parents=True, exist_ok=True)
        tmp = dest.with_name(dest.name + ".part")
        shutil.copyfile(root / e["path"], tmp)
        os.replace(tmp, dest)
        e["library"] = e["sha256"]
    if not dry_run:
        tmp = root / "manifest.json.part"
        tmp.write_text(json.dumps(man, indent=1, sort_keys=True))
        os.replace(tmp, root / "manifest.json")
    return out


def library_hashes(db_path: Path) -> set[str]:
    import sqlite3

    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        return {r[0] for r in conn.execute("SELECT content_hash FROM documents")}
    finally:
        conn.close()


# ── 2. ~/files (laptop) ────────────────────────────────────────────────────────────────────────

STATE = Path(__file__).resolve().parents[1] / "data" / "state" / "canvas-files.json"


@dataclass
class Copy:
    src: str       # path in the mirror
    dest: Path     # where it goes under ~/files
    why: str       # new, replaced, beside


@dataclass
class FilesPlan:
    copies: list[Copy] = field(default_factory=list)
    same: int = 0
    gone: list[str] = field(default_factory=list)


def _sha(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def _beside(dest: Path, sha: str) -> Path | None:
    """`<name>-v2` (then -v3...) for a new Canvas version of a file he edited; None when that
    version is already there."""
    n = 2
    while True:
        cand = dest.with_name(f"{dest.stem}-v{n}{dest.suffix}")
        if not cand.exists():
            return cand
        if _sha(cand) == sha:
            return None
        n += 1


def plan_files(manifest: dict, courses: dict[str, dict], state: dict[str, str], studies: Path) -> FilesPlan:
    live = active_folders(courses)
    p = FilesPlan()
    for e in manifest["files"].values():
        if e["course"] not in live or e["status"] != "ok":
            continue
        rel, sha = e["path"], e["sha256"]
        dest = studies / rel
        delivered = state.get(rel)
        if not dest.exists():
            if delivered:
                p.gone.append(rel)              # he deleted it: not brought back
            else:
                p.copies.append(Copy(rel, dest, "new"))
            continue
        now = _sha(dest)
        if now == sha:
            p.same += 1
            state[rel] = sha
        elif delivered and now == delivered:
            p.copies.append(Copy(rel, dest, "replaced"))
        elif (b := _beside(dest, sha)) is not None:
            p.copies.append(Copy(rel, b, "beside"))
        else:
            p.same += 1
    return p


def _server() -> tuple[str, str]:
    return (os.environ.get("LOCUS_HOST", "compute-node"),
            os.environ.get("LOCUS_DIR", "/home/alec/server-projects/locus"))


def fetch_manifest() -> dict:
    host, d = _server()
    r = subprocess.run(["ssh", "-o", "ConnectTimeout=10", "-o", "BatchMode=yes", host,
                        f"cat {shlex.quote(d)}/data/canvas/manifest.json"],
                       capture_output=True, text=True, timeout=120)
    if r.returncode != 0:
        raise RuntimeError(f"could not read the server's Canvas manifest: {r.stderr.strip()[:300]}")
    return json.loads(r.stdout)


def _rsync(pairs: list[tuple[str, Path]]) -> None:
    """Copy mirror files down: one rsync for those landing at their own path, one each for the
    rest. Copy-only: never `--delete`."""
    host, d = _server()
    src_root = f"{host}:{d}/data/canvas/"
    straight = [(s, t) for s, t in pairs if str(t).endswith(s)]
    if straight:
        dest_root = Path(str(straight[0][1])[: -len(straight[0][0])])
        lst = "\n".join(s for s, _ in straight) + "\n"
        subprocess.run(["rsync", "-rt", "--files-from=-", src_root, str(dest_root)],
                       input=lst, text=True, check=True, timeout=3600)
    for s, t in pairs:
        if not str(t).endswith(s):
            t.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(["rsync", "-t", src_root + s, str(t)], check=True, timeout=600)


def to_files(courses: dict[str, dict], *, studies: Path, dry_run: bool = False,
             manifest: dict | None = None, copy=None, state_path: Path = STATE) -> FilesPlan:
    man = manifest if manifest is not None else fetch_manifest()
    state = json.loads(state_path.read_text()) if state_path.exists() else {}
    p = plan_files(man, courses, state, studies)
    if dry_run:
        return p
    shas = {e["path"]: e["sha256"] for e in man["files"].values()}
    if p.copies:
        (copy or _rsync)([(c.src, c.dest) for c in p.copies])
    for c in p.copies:
        if c.why != "beside":
            state[c.src] = shas[c.src]
    state_path.parent.mkdir(parents=True, exist_ok=True)
    state_path.write_text(json.dumps(state, indent=1, sort_keys=True))
    return p
