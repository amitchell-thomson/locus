"""device_sent — the PDFs Locus put on the tablet, by content hash

Revision ID: 0038
Revises: 0037
Create Date: 2026-10-27

Capture reads a PDF on the tablet one of two ways (Slice 0.12): a PDF Locus SENT is Claude's
document, never ingested, with his notes on it promoted together with the passage each was
written against; any other PDF in an area folder is a source and is ingested. Getting that wrong
in one direction puts agent prose into the library as a source; in the other it keeps a lecture
out of it. So "did Locus send this" must be known, not guessed from a name or a PDF producer
string (pdfTeX writes arXiv papers too).

Every push records the sha256 of the bytes it uploads BEFORE the upload, and the rmapi tree
cache carries the same sha256 for each PDF on the device, so the question becomes a set lookup
with no download. `origin` is `push` for a recorded send and `backfill` for a document that was
sent before this table existed and was identified once, by hand-checked rule, on 2026-10-27.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0038"
down_revision: Union[str, None] = "0037"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE device_sent (
            content_hash TEXT PRIMARY KEY,
            filename     TEXT NOT NULL,
            folder       TEXT NOT NULL,
            sent_at      TEXT NOT NULL,
            origin       TEXT NOT NULL DEFAULT 'push'
        )
        """
    )


def downgrade() -> None:
    op.execute("DROP TABLE device_sent")
