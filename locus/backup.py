"""Snapshot + restore of the durable corpus state.

Locus's whole knowledge base lives in three places, and only these are NOT cheaply
regenerable from elsewhere:

  - ``data/locus.db``  — the SQLite DB. This is the expensive layer: every embedding,
    LLM summary/proposition/entity, the alias substrate and the retitled titles. Rebuilding
    it means re-paying many GPU-hours of local ingest AND the *billed* ``link``/``retitle``
    Claude passes. Principle 9 calls derived data "regenerable", but regenerable ≠ free.
  - ``data/raw``       — the raw store: the original bytes of every ingested file. After
    ``data/incoming`` is cleared this is the ONLY copy Locus holds.
  - ``data/notes``     — authored notes. An *input*, not derived; loss is unrecoverable.

``data/incoming`` is deliberately excluded: it is a transient drop folder whose contents
are already in the raw store once ingested.

The DB is snapshotted through SQLite's online backup API rather than a file copy: under WAL
the ``.db`` file alone is mid-write and inconsistent without its ``-wal`` sidecar, and the
backup API produces a single self-contained, checkpointed file even while ``locus watch`` is
writing. The raw store (1 GB+) is copied with rsync ``--link-dest`` against the previous
snapshot, so unchanged files are hardlinked and each snapshot costs only what changed —
without that, full copies would make frequent backups too expensive to actually run.
"""

from __future__ import annotations

import json
import os
import shutil
import sqlite3
import subprocess
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

SNAPSHOT_PREFIX = "locus-backup-"
MANIFEST_NAME = "manifest.json"


@dataclass
class BackupManifest:
    """What a snapshot directory contains — written as manifest.json beside the data."""

    created_at: str
    git_sha: str | None
    db_bytes: int
    db_page_count: int
    doc_count: int
    raw_bytes: int
    raw_files: int
    notes_files: int
    linked_from: str | None = field(default=None)  # prior snapshot rsync hardlinked against

    def as_dict(self) -> dict:
        return {
            "created_at": self.created_at,
            "git_sha": self.git_sha,
            "db_bytes": self.db_bytes,
            "db_page_count": self.db_page_count,
            "doc_count": self.doc_count,
            "raw_bytes": self.raw_bytes,
            "raw_files": self.raw_files,
            "notes_files": self.notes_files,
            "linked_from": self.linked_from,
        }


def _git_sha() -> str | None:
    """Short HEAD sha (+'-dirty'), or None outside a repo — pins code to the snapshot."""
    try:
        sha = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        dirty = subprocess.run(
            ["git", "status", "--porcelain"], capture_output=True, text=True, check=True
        ).stdout.strip()
        return f"{sha}{'-dirty' if dirty else ''}"
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def _backup_db(src_db: Path, dest_db: Path) -> tuple[int, int]:
    """Copy the live DB to dest via SQLite's online backup API (WAL-safe, consistent).

    Returns (byte_size, page_count) of the snapshot. The destination is a standalone DB with
    no WAL sidecar — backup() checkpoints everything into the one file.
    """
    src = sqlite3.connect(str(src_db))
    try:
        dest = sqlite3.connect(str(dest_db))
        try:
            src.backup(dest)
            (pages,) = dest.execute("PRAGMA page_count").fetchone()
        finally:
            dest.close()
    finally:
        src.close()
    return dest_db.stat().st_size, int(pages)


# Learning state is not derived either (spec L, L-D8): goals, topics, their evidence, the pace.
LEDGER_TABLES = ("facts", "fact_log", "records", "briefs", "goals", "topics", "topic_evidence", "pace", "curricula", "goal_tasks", "goal_sync", "observations", "cards", "card_reviews")


def restore_ledger(snapshot: Path, heuris_db: Path) -> dict[str, int]:
    """Put a snapshot's ledger back into heuris.db, adding only rows that are not there (by
    primary key): nothing newer is overwritten, and a table the snapshot lacks is left alone.
    The one way back for facts and learning state, which nothing can rebuild. Returns rows added
    per table."""
    src = snapshot / "heuris-ledger.db"
    if not src.exists():
        raise FileNotFoundError(f"{snapshot.name} has no heuris-ledger.db")
    from locus.heuris import briefs, curriculum, learning, learning_plan, learning_record, records
    from locus.heuris import db as hdb

    conn = hdb.connect(heuris_db)
    for mod in (learning, curriculum, learning_plan, learning_record, records, briefs):
        mod.ensure(conn)
    added: dict[str, int] = {}
    try:
        conn.execute("ATTACH DATABASE ? AS snap", (str(src),))   # only read from
        have = {r[0] for r in conn.execute("SELECT name FROM snap.sqlite_master WHERE type='table'")}
        with conn:
            for t in LEDGER_TABLES:
                if t not in have:
                    continue
                before = conn.execute(f"SELECT COUNT(*) FROM main.{t}").fetchone()[0]
                # By name, never `SELECT *`: a column added later (goals.area, pace.season, L3)
                # sits at the end of an older table and elsewhere in a new one, and a snapshot
                # from before it has no such column at all.
                main_cols = [r[1] for r in conn.execute(f"PRAGMA main.table_info({t})")]
                snap_cols = {r[1] for r in conn.execute(f"PRAGMA snap.table_info({t})")}
                cols = ", ".join(c for c in main_cols if c in snap_cols)
                conn.execute(f"INSERT OR IGNORE INTO main.{t} ({cols}) SELECT {cols} FROM snap.{t}")
                added[t] = conn.execute(f"SELECT COUNT(*) FROM main.{t}").fetchone()[0] - before
        conn.execute("DETACH DATABASE snap")
    finally:
        conn.close()
    return added


def _backup_ledger(heuris_db: Path, dest: Path) -> int:
    """heuris's ledger (H2.1): the one part of heuris.db that is not derived. Facts he approved in
    a chat exist nowhere else; the index beside them is rebuilt by re-indexing, so only the
    ledger's tables are copied (the whole file is ~300 MB of vectors). Returns rows copied."""
    if not heuris_db.exists():
        return 0
    src = sqlite3.connect(f"file:{heuris_db}?mode=ro", uri=True)
    try:
        have = {r[0] for r in src.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        tables = [t for t in LEDGER_TABLES if t in have]
        if not tables:
            return 0
        out = sqlite3.connect(str(dest))
        n = 0
        src.execute("BEGIN")  # one snapshot of every table: facts and fact_log agree
        try:
            for t in tables:
                ddl = src.execute("SELECT sql FROM sqlite_master WHERE name=?", (t,)).fetchone()[0]
                out.execute(ddl)
                rows = src.execute(f"SELECT * FROM {t}").fetchall()
                if rows:
                    out.executemany(f"INSERT INTO {t} VALUES ({','.join('?' * len(rows[0]))})", rows)
                n += len(rows)
            out.commit()
        finally:
            out.close()
        return n
    finally:
        src.close()


def _tree_stats(root: Path) -> tuple[int, int]:
    """(total_bytes, file_count) for a directory tree; (0, 0) if it does not exist."""
    if not root.exists():
        return 0, 0
    total = 0
    count = 0
    for p in root.rglob("*"):
        if p.is_file():
            total += p.stat().st_size
            count += 1
    return total, count


def _copy_tree(src: Path, dest: Path, link_dest: Path | None) -> None:
    """Mirror src -> dest. Prefer rsync (hardlinks unchanged files against link_dest);
    fall back to a plain recursive copy when rsync is unavailable (e.g. test hosts)."""
    dest.mkdir(parents=True, exist_ok=True)
    if not src.exists():
        return
    rsync = shutil.which("rsync")
    if rsync:
        cmd = [rsync, "-a", "--delete"]
        if link_dest and link_dest.exists():
            cmd.append(f"--link-dest={link_dest.resolve()}")
        # Trailing slashes: copy the CONTENTS of src into dest.
        cmd += [f"{src}/", f"{dest}/"]
        subprocess.run(cmd, check=True)
    else:
        for item in src.rglob("*"):
            rel = item.relative_to(src)
            target = dest / rel
            if item.is_dir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(item, target)


def list_snapshots(backup_root: Path) -> list[Path]:
    """Snapshot directories under backup_root, oldest first (lexicographic == chronological:
    the timestamp prefix is ISO-ordered)."""
    if not backup_root.exists():
        return []
    return sorted(p for p in backup_root.iterdir() if p.is_dir() and p.name.startswith(SNAPSHOT_PREFIX))


KEEP = 10


def prune_snapshots(backup_root: Path, keep: int = KEEP, log=lambda _m: None) -> list[Path]:
    """Delete all but the newest `keep` snapshots (16 Nov, his call: "maybe only like 10"; 120
    of them had taken 29 GB, each a full copy of locus.db). Raw files are hardlinked between
    snapshots, so removing an old one frees only what no newer one shares."""
    snaps = list_snapshots(backup_root)
    gone = snaps[: max(0, len(snaps) - keep)]
    for p in gone:
        shutil.rmtree(p, ignore_errors=True)
        log(f"  pruned {p.name}")
    return gone


def create_backup(
    *,
    db: Path,
    raw_store: Path,
    notes: Path,
    backup_root: Path,
    now: datetime | None = None,
    log=lambda _m: None,
) -> Path:
    """Write a consistent snapshot of (db, raw_store, notes) under backup_root.

    The new snapshot hardlinks its raw/ against the most recent prior snapshot's raw/, so
    only changed raw files consume new space. Returns the snapshot directory.
    """
    now = now or datetime.now(timezone.utc)
    stamp = now.strftime("%Y-%m-%dT%H-%M-%SZ")
    backup_root.mkdir(parents=True, exist_ok=True)
    prior = list_snapshots(backup_root)
    snapshot = backup_root / f"{SNAPSHOT_PREFIX}{stamp}"
    snapshot.mkdir()

    log(f"  DB     -> online backup ({db})")
    db_bytes, pages = _backup_db(db, snapshot / "locus.db")
    from locus.heuris import db as hdb

    # heuris.db's own path ($LOCUS_HEURIS_DB honoured: beside locus.db missed it, 16 Nov).
    heuris = hdb.path() if os.environ.get("LOCUS_HEURIS_DB") else db.parent / "heuris.db"
    ledger_rows = _backup_ledger(heuris, snapshot / "heuris-ledger.db")
    log(f"  ledger -> {ledger_rows} rows (facts, records, briefs) from heuris.db")

    link_src = (prior[-1] / "raw") if prior else None
    log(f"  raw    -> {raw_store}" + (f" (hardlink vs {prior[-1].name})" if link_src else ""))
    _copy_tree(raw_store, snapshot / "raw", link_dest=link_src)

    log(f"  notes  -> {notes}")
    _copy_tree(notes, snapshot / "notes", link_dest=None)

    raw_bytes, raw_files = _tree_stats(snapshot / "raw")
    _, notes_files = _tree_stats(snapshot / "notes")
    conn = sqlite3.connect(str(snapshot / "locus.db"))
    try:
        (doc_count,) = conn.execute("SELECT COUNT(*) FROM documents").fetchone()
    finally:
        conn.close()

    manifest = BackupManifest(
        created_at=now.isoformat(),
        git_sha=_git_sha(),
        db_bytes=db_bytes,
        db_page_count=pages,
        doc_count=int(doc_count),
        raw_bytes=raw_bytes,
        raw_files=raw_files,
        notes_files=notes_files,
        linked_from=prior[-1].name if link_src else None,
    )
    (snapshot / MANIFEST_NAME).write_text(json.dumps(manifest.as_dict(), indent=2))
    return snapshot


def read_manifest(snapshot: Path) -> BackupManifest | None:
    path = snapshot / MANIFEST_NAME
    if not path.exists():
        return None
    return BackupManifest(**json.loads(path.read_text()))


def restore_backup(
    snapshot: Path,
    *,
    db: Path,
    raw_store: Path,
    notes: Path,
    log=lambda _m: None,
) -> None:
    """Restore (db, raw_store, notes) from a snapshot, overwriting the live state.

    DESTRUCTIVE: the caller is responsible for ensuring no ingest is running (the CLI checks
    the ingest lock). The live DB's WAL/SHM sidecars are removed before the snapshot's
    self-contained DB is copied in, so no stale WAL frames survive the restore.
    """
    snap_db = snapshot / "locus.db"
    if not snap_db.exists():
        raise FileNotFoundError(f"snapshot has no locus.db: {snapshot}")

    log(f"  DB     <- {snap_db}")
    db.parent.mkdir(parents=True, exist_ok=True)
    for sidecar in (db.with_name(db.name + "-wal"), db.with_name(db.name + "-shm")):
        sidecar.unlink(missing_ok=True)
    shutil.copy2(snap_db, db)

    log(f"  raw    <- {snapshot / 'raw'}")
    _copy_tree(snapshot / "raw", raw_store, link_dest=None)
    log(f"  notes  <- {snapshot / 'notes'}")
    _copy_tree(snapshot / "notes", notes, link_dest=None)
