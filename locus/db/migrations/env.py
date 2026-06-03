"""Alembic migration environment for Locus.

Two Locus-specific concerns are handled here:

1. **DB URL resolution.** If no URL was injected (via locus.db.migrate or `-x url=...`),
   fall back to config.toml's paths.db so `alembic` works as a bare CLI too.
2. **sqlite-vec extension.** Creating the vec0 virtual tables requires the sqlite-vec
   extension loaded on the migration connection, so we attach a `connect` listener.

Migrations are hand-authored with op.execute(raw SQL); there is no ORM metadata, so
autogenerate is not used (target_metadata stays None).
"""

from __future__ import annotations

import sqlite_vec
from alembic import context
from sqlalchemy import engine_from_config, event, pool

config = context.config


def _resolve_url() -> str:
    """Use the injected URL if present, else build one from config.toml's db path."""
    url = config.get_main_option("sqlalchemy.url")
    if url:
        return url
    from locus.config import load

    return f"sqlite:///{load().paths.db}"


def _load_sqlite_vec(dbapi_connection, _connection_record) -> None:
    dbapi_connection.enable_load_extension(True)
    sqlite_vec.load(dbapi_connection)
    dbapi_connection.enable_load_extension(False)


def run_migrations_offline() -> None:
    context.configure(
        url=_resolve_url(),
        target_metadata=None,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
    )
    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    section = config.get_section(config.config_ini_section, {})
    section["sqlalchemy.url"] = _resolve_url()
    connectable = engine_from_config(section, prefix="sqlalchemy.", poolclass=pool.NullPool)
    event.listen(connectable, "connect", _load_sqlite_vec)

    with connectable.connect() as connection:
        # render_as_batch lets future ALTERs work under SQLite's limited ALTER support.
        context.configure(connection=connection, target_metadata=None, render_as_batch=True)
        with context.begin_transaction():
            context.run_migrations()
    connectable.dispose()


if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()
