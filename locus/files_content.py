"""PDFs between the tablet and `~/files`, both ways (spec P, step P5; P-D0, P-D5).

The folder sync (`files_sync.py`) keeps the two trees the same shape; this moves what is in
them, by one rule (P-D0, his amendment of 14 Nov): **files cross only as clean PDFs**. Ink and
notebooks stay on the tablet, where locus reads them; nothing locus makes for itself appears in
`~/files`.

- **Tablet → `~/files`.** A PDF in `Studies/`, `Projects/` or `Careers/` that is not locus's own
  (not in `device_sent`, not a Canvas file P4 pushed) lands in the same folder on the laptop,
  named in F1 form (`B18 Lecture 1` → `b18-lecture-1.pdf`), as the bytes he uploaded: never his
  ink. The bytes come from the library's raw store when capture has ingested it (by content
  hash), else from capture's bundle cache, else one `rmapi get`.
- **`~/files` → tablet.** A PDF under those three areas goes up into its folder under its own
  name, as a source (`authored=False`), so capture reads it like a hand upload. Not: anything
  under `report/`, `figures/`, a hidden or build folder, or inside a git repository or a Python
  environment; a past year's folder (laptop-only, 14 Nov); a solution (P-D3); a Canvas file
  (P4 sends those, by its own rules).

**Never a duplicate, never a deletion.** A file crosses only when its bytes are nowhere on the
other side and no document of the same name is in that folder (a name match is an earlier
version of the same thing, his B14 Image Processing lectures; his call 15 Nov: "dont care about
the newer ones"). What crossed is remembered by sha256 in `data/state/files-content.json` on the
laptop, so a file he deletes on one side is not brought back: it is put to him as a question
(delete the other copy too, or `locus files settle <path>` to keep it on one side only). A
partial `rmapi ls` under-reports, so absence only ever asks; it never acts.

At most `MAX_PER_RUN` files go each way per run, so a first run cannot flood the tablet (the
spec's risk list); the rest wait for the next tick.

Where it runs: the LAPTOP plans (it has `~/files`); the SERVER holds the rmapi login and is
reached over ssh: `locus files tablet-docs` (what is on the tablet), `tablet-stage` (puts the
clean bytes of some documents where the laptop rsyncs them from) and `tablet-push` (sends files
the laptop rsynced up).
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

from locus.canvas_pull import file_name
from locus.canvas_tablet import SOLUTION, name_key

CONTENT_AREAS = ("Studies", "Projects", "Careers")
NOT_SENT_DIRS = {"report", "figures"}
MAX_PER_RUN = 30
STATE = Path(__file__).resolve().parents[1] / "data" / "state" / "files-content.json"
STAGE = Path(__file__).resolve().parents[1] / "data" / "files-stage"


def _sha_file(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


# ── the laptop's side ──────────────────────────────────────────────────────────────────────────

def laptop_pdfs(root: Path, cache: dict[str, list]) -> dict[str, str]:
    """`Area/.../x.pdf` -> sha256 for every PDF under the three areas that may cross. `cache`
    (rel -> [size, mtime_ns, sha]) is updated in place, so a tick hashes only what changed."""
    from locus.files_sync import _OPAQUE_MARKERS, _SKIP

    out: dict[str, str] = {}
    for area in CONTENT_AREAS:
        base = root / area
        if not base.is_dir():
            continue
        for dirpath, dirnames, filenames in os.walk(base):
            here = Path(dirpath)
            if here != base and any((here / m).exists() for m in _OPAQUE_MARKERS):
                dirnames[:] = []
                continue
            dirnames[:] = sorted(d for d in dirnames
                                 if not d.startswith(".") and d not in _SKIP and d not in NOT_SENT_DIRS)
            for fn in filenames:
                if not fn.lower().endswith(".pdf") or fn.startswith("."):
                    continue
                p = here / fn
                rel = p.relative_to(root).as_posix()
                try:
                    st = p.stat()
                except OSError:
                    continue
                c = cache.get(rel)
                if c and c[0] == st.st_size and c[1] == st.st_mtime_ns:
                    out[rel] = c[2]
                else:
                    sha = _sha_file(p)
                    cache[rel] = [st.st_size, st.st_mtime_ns, sha]
                    out[rel] = sha
    for rel in [r for r in cache if r not in out]:
        del cache[rel]
    return out


@dataclass
class Down:
    uuid: str
    sha: str
    dest: str           # Area/.../name.pdf under ~/files
    tablet: str         # the tablet path, for messages


@dataclass
class Up:
    rel: str            # Area/.../name.pdf under ~/files
    sha: str
    folder: str         # tablet folder
    title: str          # tablet document name


@dataclass
class ContentPlan:
    down: list[Down] = field(default_factory=list)
    up: list[Up] = field(default_factory=list)
    questions: list[dict] = field(default_factory=list)
    other_version: list[str] = field(default_factory=list)
    solutions: list[str] = field(default_factory=list)
    waiting_folder: list[str] = field(default_factory=list)
    held_over: int = 0          # over MAX_PER_RUN, for the next tick


def _stem(name: str) -> str:
    return name[:-4] if name.lower().endswith(".pdf") else name


def _under(rel: str, prefixes) -> bool:
    return any(rel == p or rel.startswith(p + "/") for p in prefixes)


def plan(tablet_docs: list[dict], laptop: dict[str, str], manifest: dict, state: dict, *,
         laptop_dirs: set[str], blocked: set[str] = frozenset(), laptop_only: set[str] = frozenset(),
         limit: int = MAX_PER_RUN) -> ContentPlan:
    """What crosses, and what is asked. `tablet_docs` are `tablet-docs` rows (uuid, folders,
    name, sha, notebook, sent); `laptop_dirs` the laptop's folders after the folder sync;
    `blocked` folders the folder sync is asking about (nothing goes into them until answered)."""
    p = ContentPlan()
    down_state: dict[str, str] = state.get("down", {})
    up_state: dict[str, str] = state.get("up", {})
    settled: set[str] = set(state.get("settled", []))
    files = manifest.get("files", {}).values()
    canvas = {e["sha256"] for e in files if e.get("sha256")} | \
             {e["previous_sha256"] for e in files if e.get("previous_sha256")}
    canvas_names = set()
    for e in files:
        if e.get("tablet_name"):
            folder, _, title = e["tablet_name"].rpartition("/")
            canvas_names.add(name_key(folder, title))
    laptop_shas = set(laptop.values())
    laptop_names = {name_key(r.rpartition("/")[0], _stem(r.rpartition("/")[2])) for r in laptop}
    tablet_shas = {d["sha"] for d in tablet_docs if d.get("sha")}
    tablet_names = {name_key("/".join(d["folders"]), d["name"]) for d in tablet_docs}
    asked: set[str] = set()

    def ask(path: str, on: str, sha: str, other: str) -> None:
        if sha in asked:
            return
        asked.add(sha)
        p.questions.append({"path": path, "on": on, "why": "gone", "sha": sha, "other": other})

    # Tablet -> laptop
    seen: set[str] = set()
    for d in sorted(tablet_docs, key=lambda d: ("/".join(d["folders"]), d["name"])):
        sha, folders = d.get("sha"), d["folders"]
        if d.get("notebook") or d.get("sent") or not sha or not folders or folders[0] not in CONTENT_AREAS:
            continue
        folder = "/".join(folders)
        tpath = f"{folder}/{d['name']}"
        key = name_key(folder, d["name"])
        if sha in laptop_shas or sha in settled or sha in seen:
            continue
        if sha in canvas or key in canvas_names:
            continue                                # Canvas's own: P3 puts it in ~/files
        if sha in down_state or sha in up_state:    # crossed before, deleted on the laptop since
            ask(down_state.get(sha) or up_state[sha], "laptop", sha, tpath)
            continue
        if _under(folder, laptop_only):
            continue
        if folder not in laptop_dirs or _under(folder, blocked):
            p.waiting_folder.append(tpath)
            continue
        if key in laptop_names:
            p.other_version.append(tpath)
            continue
        seen.add(sha)
        p.down.append(Down(d["uuid"], sha, f"{folder}/{file_name(_stem(d['name']) + '.pdf')}", tpath))

    # Laptop -> tablet
    seen = set()
    for rel, sha in sorted(laptop.items()):
        folder, _, fn = rel.rpartition("/")
        if sha in tablet_shas or sha in settled or sha in seen or sha in canvas:
            continue
        if _under(folder, laptop_only):
            continue
        if SOLUTION.search(fn):
            p.solutions.append(rel)
            continue
        if sha in up_state or sha in down_state:    # crossed before, deleted on the tablet since
            ask(rel, "tablet", sha, up_state.get(sha) or down_state[sha])
            continue
        if _under(folder, blocked):
            p.waiting_folder.append(rel)
            continue
        title = _stem(fn)
        if name_key(folder, title) in tablet_names:
            p.other_version.append(rel)
            continue
        seen.add(sha)
        p.up.append(Up(rel, sha, folder, title))

    if len(p.down) > limit or len(p.up) > limit:
        p.held_over = max(0, len(p.down) - limit) + max(0, len(p.up) - limit)
        p.down, p.up = p.down[:limit], p.up[:limit]
    return p


def describe(q: dict) -> str:
    if q["on"] == "laptop":
        return (f"{q['path']}: deleted on the laptop, still on the tablet as {q['other']}. "
                f"Delete it there too, or `locus files settle {q['path']}` to keep it on the tablet only?")
    return (f"{q['path']}: deleted on the tablet ({q['other']}), still on the laptop. "
            f"Delete it here too, or `locus files settle {q['path']}` to keep it on the laptop only?")


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


def settle(paths: list[str], state: dict) -> list[str]:
    """Stop asking about these (by the path the question named): the file stays where it is,
    and is never sent across again."""
    out = []
    for q in state.get("questions", []):
        if q["path"] in paths or q.get("other") in paths:
            state.setdefault("settled", [])
            if q["sha"] not in state["settled"]:
                state["settled"].append(q["sha"])
            out.append(q["path"])
    state["questions"] = [q for q in state.get("questions", []) if q["path"] not in out]
    return out


def _ssh_json(args: list[str], *, stdin: str | None = None, timeout: int = 600):
    from locus.files_sync import server_command

    r = subprocess.run(server_command(args), input=stdin, capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError(f"locus files {args[0]} on the server failed: {(r.stderr or r.stdout).strip()[-400:]}")
    return json.loads(r.stdout.strip().splitlines()[-1])


def _remote(sub: str) -> str:
    host = os.environ.get("LOCUS_HOST", "compute-node")
    d = os.environ.get("LOCUS_DIR", "/home/alec/server-projects/locus")
    return f"{host}:{d}/data/files-stage/{sub}/"


def sync(root: Path, *, laptop_dirs: set[str], blocked: set[str], laptop_only: set[str],
         manifest: dict, dry_run: bool = False, state_path: Path = STATE, log=print) -> ContentPlan:
    """One run, laptop side. Questions are written even on a dry run: they are true either way."""
    state = load_state(state_path)
    cache = state.setdefault("hashes", {})
    laptop = laptop_pdfs(root, cache)
    docs = _ssh_json(["tablet-docs"], timeout=180)
    p = plan(docs, laptop, manifest, state, laptop_dirs=laptop_dirs, blocked=blocked, laptop_only=laptop_only)
    state["questions"] = p.questions
    if dry_run:
        save_state(state, state_path)
        return p
    if p.down:
        got = _ssh_json(["tablet-stage", *[d.uuid for d in p.down]], timeout=3600)
        for d in p.down:
            if got.get(d.uuid) != "ok":
                log(f"from the tablet failed  {d.tablet}: {got.get(d.uuid)}")
                continue
            dest = root / d.dest
            dest.parent.mkdir(parents=True, exist_ok=True)
            tmp = dest.with_name(dest.name + ".part")
            subprocess.run(["rsync", "-t", _remote("down") + f"{d.sha}.pdf", str(tmp)], check=True, timeout=600)
            if _sha_file(tmp) != d.sha:
                tmp.unlink()
                log(f"from the tablet failed  {d.tablet}: the bytes changed on the way")
                continue
            os.replace(tmp, dest)
            state.setdefault("down", {})[d.sha] = d.dest
            log(f"from the tablet  {d.tablet} -> ~/files/{d.dest}")
        save_state(state, state_path)
    if p.up:
        with tempfile.TemporaryDirectory() as td:
            for u in p.up:
                os.symlink(root / u.rel, Path(td) / f"{u.sha}.pdf")
            subprocess.run(["rsync", "-tL", f"{td}/", _remote("up")], check=True, timeout=3600)
        items = [{"sha": u.sha, "folder": u.folder, "title": u.title} for u in p.up]
        got = _ssh_json(["tablet-push"], stdin=json.dumps(items), timeout=3600)
        for u in p.up:
            if got.get(u.sha) == "sent":
                state.setdefault("up", {})[u.sha] = f"{u.folder}/{u.title}"
                log(f"to the tablet  ~/files/{u.rel}")
            else:
                log(f"to the tablet failed  ~/files/{u.rel}: {got.get(u.sha)}")
        save_state(state, state_path)
    save_state(state, state_path)
    return p


# ── the server's side ──────────────────────────────────────────────────────────────────────────

def tablet_docs() -> list[dict]:
    """Every document on the tablet, with whether locus sent it (`device_sent`)."""
    from locus.capture.device import read_device
    from locus.capture.remarkable import _subprocess_runner
    from locus.capture.sent import sent_hashes
    from locus.config import load
    from locus.db.connection import get_connection as connect

    cfg = load()
    docs = read_device(_subprocess_runner(cfg.capture.rmapi_binary), tree_cache=cfg.capture.rmapi_tree_cache)
    conn = connect(cfg.paths.db)
    try:
        sent = sent_hashes(conn)
    finally:
        conn.close()
    return [{"uuid": d.uuid, "folders": list(d.folders), "name": d.name, "sha": d.pdf_hash,
             "notebook": d.is_notebook, "sent": bool(d.pdf_hash and d.pdf_hash in sent)} for d in docs]


def clean_bytes(sha: str, *, raw_store: Path, db: Path, cache: Path, fetch) -> bytes:
    """The PDF exactly as uploaded, never with ink: the library's raw copy (capture ingested it
    by this content hash), else the cached bundle's PDF, else a fresh bundle (`fetch()`)."""
    import sqlite3
    import zipfile

    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        row = conn.execute("SELECT raw_path FROM documents WHERE content_hash = ?", (sha,)).fetchone()
    finally:
        conn.close()
    candidates: list[bytes] = []
    if row and row[0]:
        rp = Path(row[0]) if Path(row[0]).is_absolute() else raw_store / row[0]
        if rp.exists():
            candidates.append(rp.read_bytes())

    def from_bundle(b: Path) -> bytes:
        with zipfile.ZipFile(b) as z:
            name = next((n for n in z.namelist() if n.endswith(".pdf")), None)
            return z.read(name) if name else b""

    for b in [c for c in [cache] if c.exists()]:
        candidates.append(from_bundle(b))
    for data in candidates:
        if hashlib.sha256(data).hexdigest() == sha:
            return data
    data = from_bundle(fetch())
    if hashlib.sha256(data).hexdigest() != sha:
        raise RuntimeError("the bundle's PDF does not match the tablet's hash")
    return data


def stage(uuids: list[str], *, stage_dir: Path = STAGE / "down") -> dict[str, str]:
    from locus.capture.device import read_device
    from locus.capture.remarkable import _subprocess_runner
    from locus.capture.rmdoc import fetch_rmdoc
    from locus.config import load

    cfg = load()
    if stage_dir.exists():
        shutil.rmtree(stage_dir)
    stage_dir.mkdir(parents=True)
    docs = {d.uuid: d for d in read_device(_subprocess_runner(cfg.capture.rmapi_binary),
                                           tree_cache=cfg.capture.rmapi_tree_cache)}
    cache_dir = Path(cfg.paths.db).parent / "cache" / "rmdoc"
    out: dict[str, str] = {}
    for u in uuids:
        d = docs.get(u)
        if d is None or not d.pdf_hash:
            out[u] = "not on the tablet"
            continue
        try:
            with tempfile.TemporaryDirectory() as td:
                data = clean_bytes(d.pdf_hash, raw_store=cfg.paths.raw_store, db=Path(cfg.paths.db),
                                   cache=cache_dir / f"{u}.rmdoc",
                                   fetch=lambda: fetch_rmdoc(d.path, td, rmapi_binary=cfg.capture.rmapi_binary,
                                                             timeout=600))
            (stage_dir / f"{d.pdf_hash}.pdf").write_bytes(data)
            out[u] = "ok"
        except Exception as e:  # one document's failure is that document's
            out[u] = f"error: {e}"[:300]
    return out


def push(items: list[dict], *, stage_dir: Path = STAGE / "up", deliver=None, hashes=None,
         pause: float = 10.0, sleep=time.sleep, log=lambda s: None) -> dict[str, str]:
    """Send each staged file to its folder under its title, once: a file whose bytes are on the
    tablet by now is skipped. The cloud's 429s are waited out (60 s, doubling), as in P4."""
    from locus.canvas_tablet import _is_429, live_deliver, tablet_now

    if hashes is None:
        hashes, _ = tablet_now()
    deliver = deliver or live_deliver()
    out: dict[str, str] = {}
    for i, it in enumerate(items):
        sha, src = it["sha"], stage_dir / f"{it['sha']}.pdf"
        if not re.fullmatch(r"[0-9a-f]{64}", sha) or it["folder"].split("/", 1)[0] not in CONTENT_AREAS \
                or ".." in it["folder"].split("/") or "/" in it["title"]:
            out[sha] = "error: refused (not a file under the three areas)"
            continue
        if sha in hashes:
            out[sha] = "sent"           # already there: nothing to do, and nothing to send again
            continue
        if not src.exists() or _sha_file(src) != sha:
            out[sha] = "error: the staged file is missing or changed"
            continue
        with tempfile.TemporaryDirectory() as td:
            target = Path(td) / f"{it['title']}.pdf"
            shutil.copyfile(src, target)
            delay = 60.0
            for attempt in range(4):
                try:
                    deliver(target, it["folder"])
                    out[sha] = "sent"
                    hashes.add(sha)
                    log(f"sent {it['folder']}/{it['title']}")
                    break
                except RuntimeError as err:
                    if not _is_429(err) or attempt == 3:
                        out[sha] = f"error: {err}"[:300]
                        break
                    sleep(delay)
                    delay *= 2
        src.unlink(missing_ok=True)
        if pause and i < len(items) - 1:
            sleep(pause)
    return out
