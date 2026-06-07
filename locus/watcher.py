"""Folder watcher (§10 slice 3): auto-ingest files dropped into vault/incoming/.

Closes the loop with the laptop outbox — a file rsync'd into incoming is ingested without a
manual command. Uses simple polling (not filesystem events): ingest is unbounded-time (§2.4),
polling is dependency-free, survives restarts (it rescans the backlog), and a settle window
avoids grabbing a file mid-transfer.

Disposition per file (incoming is a disposable drop zone; the raw copy is kept in vault/raw):
  ingested / skipped   -> removed from incoming
  unsupported / quarantined -> moved to incoming/.quarantine/<drop subpath> so it is not
                               retried forever (subpath preserved for category provenance)

Repo drops (step 10): a DIRECTORY directly under incoming/projects/ is one repo unit (the
§15.4 convention — "a repo under projects/ is category='project', source_type='code'"),
ingested whole via ingest_repo as a snapshot. Its tree is tarballed into vault/raw (the
source is deleted after ingest, so git-is-the-raw-store doesn't apply), keyed by a stable
`locusdrop:<name>` source_uri so a re-drop replaces the prior document. Tree settle uses
ctime, not mtime — rsync -t preserves source mtimes, so mtime would always look settled.

The watch loop also runs the tracked-repo sync (locus/sync.py) every
config [repos].check_interval seconds, and holds the ingest lock per tick so a manual
`locus ingest`/`locus sync` can never race it on the GPU.
"""

from __future__ import annotations

import logging
import shutil
import tarfile
import time
from pathlib import Path

from locus.config import load
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.extract.code import collect_repo
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.ingest_pipeline import IngestResult, ingest_file, ingest_repo
from locus.sync import sync_repos

log = logging.getLogger(__name__)

QUARANTINE_DIRNAME = ".quarantine"
# Directories under this category folder are repo drops (one unit), not file organization.
REPO_DROP_CATEGORY = "projects"
# Settle window for repo trees: newest ctime in the tree must be at least this old. Longer
# than the per-file window because the outbox rsyncs on a 60 s interval and a large tree
# can arrive across runs.
REPO_SETTLE_SECONDS = 30.0


def _candidates(incoming: Path, settle: float) -> list[Path]:
    """Files ready to ingest: real files anywhere under incoming, stable for `settle` seconds.

    Recurses into subfolders — the drop-folder category convention (ingest_pipeline._category:
    `incoming/papers/x.pdf` -> category 'paper') means the laptop outbox rsyncs *subfolders*,
    so a flat scan would never see categorized drops. Dotfiles and anything inside a dotted
    directory (.quarantine) are skipped.
    """
    now = time.time()
    out: list[Path] = []
    for p in sorted(incoming.rglob("*")):
        if not p.is_file():
            continue
        rel = p.relative_to(incoming)
        if any(part.startswith(".") for part in rel.parts):
            continue  # dotfiles (.gitkeep) and dotted subtrees (.quarantine)
        if len(rel.parts) >= 3 and rel.parts[0] == REPO_DROP_CATEGORY:
            continue  # inside a repo-drop directory — claimed whole by _repo_drops
        try:
            if now - p.stat().st_mtime < settle:
                continue  # still being written / just landed — wait for it to settle
        except FileNotFoundError:
            continue
        out.append(p)
    return out


def _repo_drops(incoming: Path) -> list[Path]:
    """Settled repo-drop directories (directly under incoming/projects/).

    A tree is settled when its newest file *ctime* is at least REPO_SETTLE_SECONDS old —
    ctime is stamped locally on arrival, unlike mtime, which rsync -t preserves from the
    source. An empty directory is not a candidate (rsync creates dirs before files).
    """
    root = incoming / REPO_DROP_CATEGORY
    if not root.is_dir():
        return []
    now = time.time()
    out: list[Path] = []
    for d in sorted(root.iterdir()):
        if not d.is_dir() or d.name.startswith("."):
            continue
        ctimes = [f.stat().st_ctime for f in d.rglob("*") if f.is_file()]
        if not ctimes:
            continue  # empty so far — tree still arriving (or junk; retried next tick)
        if now - max(ctimes) >= REPO_SETTLE_SECONDS:
            out.append(d)
    return out


def _tarball_tree(tree: Path, dest: Path) -> None:
    """Archive the dropped tree into the raw store (its source is deleted after ingest)."""
    if dest.exists():
        return  # same content re-dropped — hash-named, already archived
    dest.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(dest, "w:gz") as tar:
        tar.add(tree, arcname=tree.name)


def _process_repo_drop(conn, tree: Path, quarantine: Path) -> IngestResult:
    """Ingest one dropped repo tree as a snapshot; dispose of the tree per the result."""
    try:
        snap = collect_repo(tree)  # raises ValueError when nothing is ingestible
        tar_name = f"{snap.content_hash}.tar.gz"
        _tarball_tree(tree, load().paths.raw_store / tar_name)
        result = ingest_repo(
            tree, conn, source_uri=f"locusdrop:{tree.name}", raw_name=tar_name
        )
    except Exception as exc:
        result = IngestResult(str(tree), "quarantined", error=str(exc))
    if result.status in ("ingested", "skipped"):
        shutil.rmtree(tree, ignore_errors=True)  # archived in vault/raw; drop zone is disposable
        _prune_empty_parents(tree.parent, quarantine.parent)  # quarantine.parent == incoming
    else:
        dest = quarantine / REPO_DROP_CATEGORY / tree.name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(tree), str(dest))
    log.info("watch: %s/ -> %s", tree.name, result.status)
    return result


def _prune_empty_parents(start: Path, root: Path) -> None:
    """Remove now-empty ancestor dirs of an ingested drop, climbing up to (not incl.) root.

    A drained nested subfolder (incoming/career/career-documents/) is a remnant, not a drop
    target — the laptop outbox recreates category folders on the next rsync, so an emptied
    one is clutter. `rmdir` is self-guarding: a folder still holding a settle-pending sibling
    file simply isn't empty, so it survives to be pruned on a later tick once that file
    ingests. Never touches `root` itself (the incoming/.gitkeep anchor lives there) nor any
    dotted subtree (.quarantine).
    """
    d = start
    while d != root and root in d.parents:
        if any(part.startswith(".") for part in d.relative_to(root).parts):
            break  # don't climb into/prune .quarantine and friends
        try:
            d.rmdir()  # succeeds only when the directory is empty
        except OSError:
            break  # non-empty (pending siblings) or already gone — stop climbing
        d = d.parent


def process_once(conn, *, incoming: Path | None = None, settle: float = 3.0) -> list[IngestResult]:
    """Ingest every settled file/repo-drop in incoming once; dispose per result."""
    incoming = incoming or load().paths.incoming
    quarantine = incoming / QUARANTINE_DIRNAME
    results: list[IngestResult] = []
    for tree in _repo_drops(incoming):
        results.append(_process_repo_drop(conn, tree, quarantine))
    for path in _candidates(incoming, settle):
        result = ingest_file(path, conn)
        results.append(result)
        rel = path.relative_to(incoming)
        if result.status in ("ingested", "skipped"):
            path.unlink(missing_ok=True)  # canonical copy is already in vault/raw
            _prune_empty_parents(path.parent, incoming)  # don't leave drained subfolders behind
        else:  # unsupported / quarantined — set aside so it is not retried every tick
            # Preserve the drop subpath: keeps category provenance visible and avoids
            # same-name collisions across category folders.
            dest = quarantine / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(path), str(dest))
        log.info("watch: %s -> %s", rel, result.status)
    return results


def watch(*, interval: float = 5.0, settle: float = 3.0, once: bool = False) -> None:
    """Poll vault/incoming/ and ingest new files until interrupted (or one pass if `once`).

    Also runs the tracked-repo sync every config [repos].check_interval seconds (and once
    on `--once`, so that flag means "process everything now"). Each tick holds the ingest
    lock; if a manual `locus ingest`/`locus sync` holds it, the tick is skipped and
    retried on the next interval.
    """
    cfg = load()
    migrate(cfg.paths.db)  # ensure the schema is current before ingesting
    conn = get_connection(cfg.paths.db)
    repos = [Path(p) for p in cfg.repos.paths]
    last_sync = float("-inf")  # sync on the first tick (cheap: HEAD checks only)
    log.info(
        "watching %s (interval %.0fs; %d tracked repo(s), sync every %.0fs)",
        cfg.paths.incoming, interval, len(repos), cfg.repos.check_interval,
    )
    try:
        while True:
            try:
                with ingest_lock():
                    process_once(conn, incoming=cfg.paths.incoming, settle=settle)
                    if repos and (once or time.monotonic() - last_sync >= cfg.repos.check_interval):
                        sync_repos(conn, repos)
                        last_sync = time.monotonic()
            except IngestLockHeld as exc:
                log.info("tick skipped: %s", exc)
            if once:
                break
            time.sleep(interval)
    finally:
        conn.close()
