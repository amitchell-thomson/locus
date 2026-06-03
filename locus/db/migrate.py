"""Programmatic Alembic driver: upgrade a Locus DB to head and inspect its revision.

Wraps Alembic so the app and tests can migrate a DB in-process without shelling out.
Version tracking is owned by Alembic's `alembic_version` table (this replaces the previously
hand-rolled `schema_version` table).
"""

from __future__ import annotations

from pathlib import Path

from alembic import command
from alembic.config import Config as AlembicConfig
from alembic.script import ScriptDirectory

from locus.config import PROJECT_ROOT
from locus.db.connection import get_connection

ALEMBIC_INI = PROJECT_ROOT / "alembic.ini"


def _config(db_path: Path | str) -> AlembicConfig:
    cfg = AlembicConfig(str(ALEMBIC_INI))
    cfg.set_main_option("sqlalchemy.url", f"sqlite:///{db_path}")
    return cfg


def migrate(db_path: Path | str) -> None:
    """Upgrade the DB at db_path to the latest revision (idempotent)."""
    command.upgrade(_config(db_path), "head")


def current_revision(db_path: Path | str) -> str | None:
    """Revision the DB is currently at, or None if no migrations have been applied.

    Reads Alembic's own `alembic_version` table directly (our runtime uses raw sqlite3,
    not a SQLAlchemy connection, so MigrationContext.configure can't be used here).
    """
    conn = get_connection(db_path)
    try:
        exists = conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='alembic_version'"
        ).fetchone()
        if not exists:
            return None
        row = conn.execute("SELECT version_num FROM alembic_version").fetchone()
        return row["version_num"] if row else None
    finally:
        conn.close()


def head_revision(db_path: Path | str) -> str:
    """The latest revision defined in the migration scripts."""
    return ScriptDirectory.from_config(_config(db_path)).get_current_head()
