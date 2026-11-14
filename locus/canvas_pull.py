"""The Canvas pull into a mirror (spec P, step P2; P-D2, P-D4).

`locus canvas pull` walks each mapped course's modules (the Files tab is 403 for students, P1)
and keeps a copy of every file in `data/canvas/<course folder>/<module folder>/<file>`, with a
manifest beside it. The mirror is locus's own copy of Canvas, so the later steps (P3 to
`~/files` and the library, P4 to the tablet, P8 the syllabus links) read the disk, never Canvas
twice. Nothing here writes outside `data/canvas/`, and the client is GET only.

What it found on the real account (14 Nov 2026): 717 files over the 18 mapped courses, almost all
reached as module items of type File; past years' lab courses (P5, A5) also keep files behind
Pages ("Files needed for Lab Work"), so a Page item's body is read for `/files/<id>` links. Links
(ExternalUrl: Panopto playlists, demo videos) are kept in the manifest for `lectures.md` later.

Names (P-D4): the module's folder is `syllabus.folder_name` of its Canvas name, the same folder the
syllabus went into; a course with a single module puts its files straight under the course
folder. A file keeps Canvas's display name in F1 form (`B18 Lecture 1.pdf` -> `b18-lecture-1.pdf`).

Re-running fetches only what changed: a file whose Canvas `updated_at` and size match the manifest
and whose bytes are on disk is not downloaded. An inactive (past-year) course does not change, so
its known files are not even asked about again; a live course's are, every run.

Videos are not mirrored (P-D2: lectures arrive as a link list); they are listed in the manifest as
skipped, as is anything over `MAX_BYTES`.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path

from locus.canvas import CanvasError, Client, modules, name
from locus.config import PROJECT_ROOT
from locus.syllabus import folder_name

MIRROR = PROJECT_ROOT / "data" / "canvas"
MAX_BYTES = 300 * 1024 * 1024


def file_name(display: str) -> str:
    """Canvas's display name in F1 form, extension kept and lower-cased."""
    stem, dot, ext = display.strip().rpartition(".")
    if not dot or not re.fullmatch(r"[A-Za-z0-9]{1,6}", ext):
        stem, ext = display.strip(), ""
    f1 = re.sub(r"[^a-z0-9]+", "-", stem.lower()).strip("-") or "file"
    return f"{f1}.{ext.lower()}" if ext else f1


@dataclass
class Placement:
    """Where a file sits on Canvas: its module, the sub-header above it, and how it was reached."""
    file_id: int
    module: str
    subheader: str = ""
    via: str = "module"       # "module", or "page: <title>"


@dataclass
class Entry:
    id: int
    course: str               # registry id
    module: str               # Canvas module name
    subheader: str
    via: str
    name: str                 # Canvas display name
    path: str                 # relative to the mirror
    content_type: str = ""
    size: int = 0
    updated_at: str = ""
    sha256: str = ""
    status: str = "ok"        # ok, skipped: video, skipped: too large, error: ...
    fetched: str = ""


@dataclass
class Report:
    new: list[str] = field(default_factory=list)
    changed: list[str] = field(default_factory=list)
    unchanged: int = 0
    skipped: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)


def placements(client: Client, course_id: int) -> tuple[list[Placement], list[dict], int]:
    """Every file a course's modules reach, in Canvas's order, its links, and how many modules
    it has."""
    out: list[Placement] = []
    links: list[dict] = []
    ms = modules(client, course_id, items=True)
    for m in ms:
        mod = name(m.get("name") or "")
        sub = ""
        for it in m.get("items") or []:
            kind = it.get("type")
            if kind == "SubHeader":
                sub = name(it.get("title") or "")
            elif kind == "File" and it.get("content_id"):
                out.append(Placement(int(it["content_id"]), mod, sub))
            elif kind in ("ExternalUrl", "ExternalTool") and it.get("external_url"):
                links.append({"module": mod, "title": name(it.get("title") or ""), "url": it["external_url"]})
            elif kind == "Page" and it.get("page_url"):
                try:
                    page = client.get(f"courses/{course_id}/pages/{it['page_url']}")
                except CanvasError:
                    continue
                body = (page or {}).get("body") or "" if isinstance(page, dict) else ""
                for fid in dict.fromkeys(re.findall(r"/files/(\d+)", body)):
                    out.append(Placement(int(fid), mod, sub, f"page: {name(it.get('title') or '')}"))
    return out, links, len(ms)


def _download(client: Client, url: str, dest: Path) -> str:
    """Fetch a file's bytes to `dest` (atomically) and return their sha256. The Authorization
    header goes to Canvas only: Canvas answers a download with a redirect to its file store, and
    that request is made without the token."""

    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None

    opener = urllib.request.build_opener(NoRedirect)
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {client.token}"})
    try:
        resp = opener.open(req, timeout=120)
    except urllib.error.HTTPError as e:
        if e.code not in (301, 302, 303, 307, 308) or not e.headers.get("Location"):
            raise CanvasError(f"download {url.split('?')[0]}: HTTP {e.code}") from e
        resp = urllib.request.urlopen(urllib.request.Request(e.headers["Location"]), timeout=300)
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_name(dest.name + ".part")
    h = hashlib.sha256()
    with resp, tmp.open("wb") as f:
        while chunk := resp.read(1 << 20):
            h.update(chunk)
            f.write(chunk)
    os.replace(tmp, dest)
    return h.hexdigest()


def load_manifest(root: Path = MIRROR) -> dict:
    p = root / "manifest.json"
    return json.loads(p.read_text()) if p.exists() else {"files": {}, "links": {}}


def _save_manifest(man: dict, root: Path) -> None:
    root.mkdir(parents=True, exist_ok=True)
    tmp = root / "manifest.json.part"
    tmp.write_text(json.dumps(man, indent=1, sort_keys=True))
    os.replace(tmp, root / "manifest.json")


def pull(client: Client, courses: dict[str, dict], *, root: Path = MIRROR, only: set[str] | None = None,
         dry_run: bool = False, pause: float = 0.0) -> Report:
    """Bring the mirror up to date with Canvas for `courses` (registry id -> entry with `canvas`
    and `folder`). `only` limits it to some ids. Dry run: say what would be fetched, fetch nothing."""
    man = load_manifest(root)
    files: dict[str, dict] = man["files"]
    rep = Report()
    for rid, c in courses.items():
        if not c.get("canvas") or not c.get("folder") or (only and rid not in only):
            continue
        live = c.get("active", True) is not False
        try:
            places, links, n_modules = placements(client, int(c["canvas"]))
        except CanvasError as e:
            rep.errors.append(f"{rid}: {e}")
            continue
        man["links"][rid] = links
        # One module on Canvas, not one module with files so far: B18 has four, only the first
        # with files on 14 Nov, and its files belong in cellular-physiology/ like its syllabus.
        single = n_modules == 1
        taken: dict[str, int] = {}      # path -> file id, within this run
        for e in files.values():
            if e["course"] == rid:
                taken[e["path"]] = e["id"]
        seen: set[int] = set()
        for p in places:
            if p.file_id in seen:
                continue                 # a file in two modules is mirrored once, at its first
            seen.add(p.file_id)
            key = str(p.file_id)
            old = files.get(key)
            if old and not live and (old["status"].startswith("skipped") or (root / old["path"]).exists()):
                rep.unchanged += 1
                continue
            try:
                meta = client.get(f"courses/{c['canvas']}/files/{p.file_id}")
            except CanvasError as e:
                # A Page can link to a file Canvas has since deleted, or one in a course he is
                # not on (P5's Computing Lab pages, 14 Nov: 9 such links, all 404 or 403). That
                # is the page's dead link, not a failure of the pull; a module's own file is.
                if p.via != "module" and re.search(r"HTTP 40[34]", str(e)):
                    status = "skipped: dead link"
                    if not old or old["status"] != status:
                        rep.skipped.append(f"{rid} {p.module}: file {p.file_id} ({p.via}): dead link")
                    if not dry_run:
                        files[key] = asdict(Entry(p.file_id, rid, p.module, p.subheader, p.via,
                                                  f"file {p.file_id}", "", status=status))
                    continue
                rep.errors.append(f"{rid} {p.module}: file {p.file_id}: {e}")
                continue
            display = meta.get("display_name") or meta.get("filename") or str(p.file_id)
            sub = "" if single else folder_name(p.module)
            fname = file_name(display)
            rel = "/".join(x for x in (c["folder"], sub, fname) if x)
            if taken.get(rel, p.file_id) != p.file_id:     # two files with one name in a folder
                stem, dot, ext = fname.rpartition(".")
                fname = f"{stem}-{p.file_id}.{ext}" if dot else f"{fname}-{p.file_id}"
                rel = "/".join(x for x in (c["folder"], sub, fname) if x)
            if old and old["path"] != rel:
                rel = old["path"]        # keep where it first landed: a rename on Canvas moves nothing
            taken[rel] = p.file_id
            e = Entry(p.file_id, rid, p.module, p.subheader, p.via, display, rel,
                      meta.get("content-type") or "", int(meta.get("size") or 0), meta.get("updated_at") or "")
            if old and old["status"] == "ok" and old["updated_at"] == e.updated_at and old["size"] == e.size \
                    and (root / rel).exists():
                rep.unchanged += 1
                continue
            label = f"{rid}/{rel.split('/', 1)[-1]}"
            if e.content_type.startswith("video/"):
                e.status = "skipped: video"
            elif e.size > MAX_BYTES:
                e.status = f"skipped: too large ({e.size // (1 << 20)} MB)"
            if e.status != "ok":
                if not old or old["status"] != e.status:
                    rep.skipped.append(f"{label} ({e.status})")
                if not dry_run:
                    files[key] = asdict(e)
                continue
            (rep.changed if old and old["status"] == "ok" else rep.new).append(label)
            if dry_run:
                continue
            try:
                e.sha256 = _download(client, meta["url"], root / rel)
            except (CanvasError, urllib.error.URLError, OSError, KeyError) as err:
                (rep.changed if old and old["status"] == "ok" else rep.new).remove(label)
                rep.errors.append(f"{label}: {err}")
                continue
            e.fetched = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
            if old and old.get("sha256") and old["sha256"] != e.sha256:
                e_d = asdict(e)
                e_d["previous_sha256"] = old["sha256"]
                files[key] = e_d
            else:
                files[key] = asdict(e)
            _save_manifest(man, root)    # after each file, so an interrupted run keeps its work
            if pause:
                time.sleep(pause)
    if not dry_run:
        _save_manifest(man, root)
    return rep
