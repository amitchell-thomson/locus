"""chunks_fts: FTS5 lexical index over chunk text for hybrid retrieval (§15.2)

Dense embeddings (general-purpose nomic) blur exact symbols, tickers, and specific method
names; an FTS5/BM25 lexical arm catches them. External-content FTS5 table mirrors chunks.raw_text,
kept in sync by triggers (so re-ingest delete/insert stays consistent), with a one-time backfill
of existing chunks. Not re-ingest-bound — derivable from stored data at any time.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0002"
down_revision: Union[str, None] = "0001"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        "CREATE VIRTUAL TABLE chunks_fts USING fts5("
        " raw_text, content='chunks', content_rowid='id')"
    )
    op.execute(
        "CREATE TRIGGER chunks_fts_ai AFTER INSERT ON chunks BEGIN "
        "INSERT INTO chunks_fts(rowid, raw_text) VALUES (new.id, new.raw_text); END"
    )
    op.execute(
        "CREATE TRIGGER chunks_fts_ad AFTER DELETE ON chunks BEGIN "
        "INSERT INTO chunks_fts(chunks_fts, rowid, raw_text) "
        "VALUES ('delete', old.id, old.raw_text); END"
    )
    op.execute(
        "CREATE TRIGGER chunks_fts_au AFTER UPDATE ON chunks BEGIN "
        "INSERT INTO chunks_fts(chunks_fts, rowid, raw_text) "
        "VALUES ('delete', old.id, old.raw_text); "
        "INSERT INTO chunks_fts(rowid, raw_text) VALUES (new.id, new.raw_text); END"
    )
    # Backfill existing chunks.
    op.execute("INSERT INTO chunks_fts(rowid, raw_text) SELECT id, raw_text FROM chunks")


def downgrade() -> None:
    for trig in ("chunks_fts_au", "chunks_fts_ad", "chunks_fts_ai"):
        op.execute(f"DROP TRIGGER IF EXISTS {trig}")
    op.execute("DROP TABLE IF EXISTS chunks_fts")
