"""Loop B on the capture timer — reading annotations captured, mapped, and transcribed.

Model-free: rmapi, rmdoc fetch/parse, and transcription are injected. What is asserted is the
part that failed silently in live use: the ink→answer chain's one manual link (transcription)
and the device→corpus mapping (marks keyed to a path that joins to nothing are invisible to
every downstream pass).
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path

from locus.capture.loop_b import annotate_sync
from locus.db.connection import get_connection
from locus.db.migrate import migrate

PDF_BYTES = b"%PDF-1.4 fake paper bytes"
PDF_HASH = hashlib.sha256(PDF_BYTES).hexdigest()


@dataclass
class _FakeDoc:
    pdf_bytes: bytes = PDF_BYTES
    doc_uuid: str = "uuid-1"


@dataclass
class _FakeMark:
    pdf_page: int = 0
    kind: str = "margin_note"
    bbox: tuple = (1.0, 2.0, 3.0, 4.0)
    covered_text: str = "the passage"
    line_text: str = "the full line"
    in_margin: bool = True
    stroke_count: int = 20
    point_count: int = 200


def _conn(tmp_path: Path):
    db = tmp_path / "loopb.db"
    migrate(db)
    return get_connection(db)


def _staged(tmp_path: Path, uuid: str = "uuid-1") -> Path:
    staging = tmp_path / "staging"
    staging.mkdir()
    (staging / f"{uuid}.pdf").write_bytes(b"staged render bytes")
    return staging


def _fake_transcriber(calls: list):
    def transcribe(conn, marks, *, source_uri, limit):
        calls.append((source_uri, limit))
        return min(1, limit)
    return transcribe


def _run(conn, staging, *, manifest=None, transcribe_fn=None, limit=8):
    return annotate_sync(
        conn,
        staging_dir=staging,
        manifest=manifest if manifest is not None else {},
        index_fn=lambda: {"uuid-1": ("/Reading/Finished/A Paper", "A Paper")},
        fetch_fn=lambda path, dest: Path(dest) / "a.rmdoc",
        read_fn=lambda p: _FakeDoc(),
        marks_fn=lambda doc: [_FakeMark()],
        transcribe_fn=transcribe_fn or _fake_transcriber([]),
        transcribe_limit=limit,
    )


def test_marks_key_to_the_corpus_document_by_content_hash(tmp_path):
    """The live failure (2026-08-06): marks keyed by device path joined to no document.

    The `.rmdoc` bundle carries the original PDF bytes and `content_hash` is sha256 of exactly
    those bytes, so the mapping is computed, never remembered.
    """
    conn = _conn(tmp_path)
    with conn:
        conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
            "ingest_model) VALUES (?,'pdf','vault/incoming/paper/a.pdf','r','A Paper','t')",
            (PDF_HASH,),
        )
    calls: list = []
    r = _run(conn, _staged(tmp_path), transcribe_fn=_fake_transcriber(calls))
    assert [o.status for o in r.outcomes] == ["annotated"]
    assert r.outcomes[0].hash_mapped
    assert r.outcomes[0].transcribed == 1
    assert calls == [("vault/incoming/paper/a.pdf", 8)]   # transcription keyed the same way
    row = conn.execute("SELECT source_uri, source_run FROM pdf_annotations").fetchone()
    assert row["source_uri"] == "vault/incoming/paper/a.pdf"
    assert row["source_run"] is not None            # provenance: which run stored this ink


def test_an_uningested_document_keys_by_device_path_and_says_so(tmp_path):
    conn = _conn(tmp_path)
    r = _run(conn, _staged(tmp_path))
    assert r.outcomes[0].status == "annotated"
    assert not r.outcomes[0].hash_mapped
    row = conn.execute("SELECT source_uri FROM pdf_annotations").fetchone()
    assert row["source_uri"] == "/Reading/Finished/A Paper"


def test_an_unchanged_render_costs_nothing(tmp_path):
    """The device pushes every changed doc; an unchanged staged render means no new ink."""
    conn = _conn(tmp_path)
    staging = _staged(tmp_path)
    manifest: dict = {}
    fetches: list = []

    def fetch(path, dest):
        fetches.append(path)
        return Path(dest) / "a.rmdoc"

    annotate_sync(
        conn, staging_dir=staging, manifest=manifest,
        index_fn=lambda: {"uuid-1": ("/Reading/Finished/A Paper", "A Paper")},
        fetch_fn=fetch, read_fn=lambda p: _FakeDoc(), marks_fn=lambda d: [_FakeMark()],
        transcribe_limit=0,
    )
    r2 = annotate_sync(
        conn, staging_dir=staging, manifest=manifest,
        index_fn=lambda: {"uuid-1": ("/Reading/Finished/A Paper", "A Paper")},
        fetch_fn=fetch, read_fn=lambda p: _FakeDoc(), marks_fn=lambda d: [_FakeMark()],
        transcribe_limit=0,
    )
    assert len(fetches) == 1                        # second run downloaded nothing
    assert [o.status for o in r2.outcomes] == ["unchanged"]


def test_one_failing_document_never_aborts_the_batch(tmp_path):
    conn = _conn(tmp_path)
    staging = tmp_path / "staging"
    staging.mkdir()
    (staging / "bad.pdf").write_bytes(b"x")
    (staging / "good.pdf").write_bytes(b"y")

    def read(p):
        return _FakeDoc()

    def fetch(path, dest):
        if "Bad" in path:
            raise RuntimeError("cloud hiccup")
        return Path(dest) / "a.rmdoc"

    r = annotate_sync(
        conn, staging_dir=staging, manifest={},
        index_fn=lambda: {
            "bad": ("/Reading/Bad Doc", "Bad Doc"),
            "good": ("/Reading/Good Doc", "Good Doc"),
        },
        fetch_fn=fetch, read_fn=read, marks_fn=lambda d: [_FakeMark()],
        transcribe_limit=0,
    )
    by = {o.name: o.status for o in r.outcomes}
    assert by == {"Bad Doc": "failed", "Good Doc": "annotated"}
