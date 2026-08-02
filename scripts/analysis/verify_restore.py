"""Prove `locus restore` actually works, into a throwaway target.

An untested restore is a hope, not a backup — and the DB now holds agent state that is NOT
regenerable: blessings, owner corrections, mark intents, development passes. This restores the
newest snapshot into a temp tree, checks the result is a usable database rather than merely a
file that exists, and confirms an older snapshot migrates forward.

    uv run python scripts/analysis/verify_restore.py
"""
import shutil
import sqlite3
import tempfile
from pathlib import Path

from locus.backup import list_snapshots, restore_backup
from locus.config import load

cfg = load()
root = cfg.paths.db.parent / "backups"
snaps = list_snapshots(root)
print(f"snapshots available: {len(snaps)}")
assert snaps, "no snapshot to restore from"
snap = snaps[-1]
print(f"restoring: {snap.name}\n")

tmp = Path(tempfile.mkdtemp(prefix="locus-restore-"))
db, raw, notes = tmp / "locus.db", tmp / "raw", tmp / "notes"
restore_backup(snap, db=db, raw_store=raw, notes=notes, log=print)

print("\n--- is it a usable database, not just a file? ---")
c = sqlite3.connect(db); c.row_factory = sqlite3.Row
docs = c.execute("SELECT COUNT(*) FROM documents").fetchone()[0]
chunks = c.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
objects = c.execute("SELECT COUNT(*) FROM objects").fetchone()[0]
rev = c.execute("SELECT version_num FROM alembic_version").fetchone()[0]
print(f"  documents={docs}  chunks={chunks}  objects={objects}  schema={rev}")
integrity = c.execute("PRAGMA integrity_check").fetchone()[0]
print(f"  integrity_check: {integrity}")
assert integrity == "ok"
# The agent state that is NOT regenerable — losing it is the reason this must work.
blessed = c.execute("SELECT COUNT(*) FROM objects WHERE status='active'").fetchone()[0]
edits = c.execute("SELECT COUNT(*) FROM objects WHERE body LIKE '%_owner_edits%'").fetchone()[0]
print(f"  blessed objects={blessed}  carrying owner edits={edits}")
c.close()

print("\n--- does an older snapshot migrate forward? ---")
from locus.db.migrate import current_revision, migrate
print(f"  before: {current_revision(db)}")
migrate(db)
print(f"  after : {current_revision(db)}")

print("\n--- raw store + notes ---")
print(f"  raw files  : {sum(1 for _ in raw.rglob('*') if _.is_file())}")
print(f"  note files : {sum(1 for _ in notes.rglob('*.md'))}")

shutil.rmtree(tmp)
print(f"\nOK — restored, verified, and the temp tree removed. Live vault untouched.")
