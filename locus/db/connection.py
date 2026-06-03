"""Open SQLite connections with the sqlite-vec extension loaded.

sqlite-vec provides the vec0 virtual tables used for brute-force KNN vector search.
Loading it requires Python's sqlite3 to be built with extension-loading support; if this
machine's interpreter lacks it, get_connection() raises a clear, actionable error.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path

import sqlite_vec


def get_connection(db_path: Path | str) -> sqlite3.Connection:
    """Open a Locus DB connection: sqlite-vec loaded, foreign keys enforced, rows as dicts.

    Raises RuntimeError with guidance if the interpreter cannot load SQLite extensions.
    """
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row

    try:
        conn.enable_load_extension(True)
    except AttributeError as exc:  # pragma: no cover - interpreter-dependent
        conn.close()
        raise RuntimeError(
            "This Python's sqlite3 was built without extension-loading support, which "
            "sqlite-vec requires. Use a Python build that enables it (e.g. one managed by uv)."
        ) from exc

    sqlite_vec.load(conn)
    conn.enable_load_extension(False)

    # Enforce referential integrity + ON DELETE CASCADE (off by default in SQLite).
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def vec_version(conn: sqlite3.Connection) -> str:
    """Return the loaded sqlite-vec version string (smoke-test helper)."""
    (version,) = conn.execute("SELECT vec_version()").fetchone()
    return version
