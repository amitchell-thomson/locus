"""Loop B on the capture timer: the ink on PDFs captured, mapped, and transcribed.

Model-free: the cloud fetch, rmdoc parse, and transcription are injected. What is asserted is the
part that failed silently in live use: the ink→answer chain's one manual link (transcription),
the device→corpus mapping (marks keyed to a path that joins to nothing are invisible to every
downstream pass), and since Slice 0.12 the three kinds of PDF: reading, a document Locus sent,
and a source that has to reach the library before its marks can join it.
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path

from locus.capture.device import DeviceDoc, Placement
from locus.capture.loop_b import _queue_source, annotate_sync
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


def _pdf(path: str, *, uuid: str = "uuid-1", ink: str = "ink-1", pages: int = 2) -> DeviceDoc:
    parts = path.strip("/").split("/")
    return DeviceDoc(uuid=uuid, name=parts[-1], folders=tuple(parts[:-1]), is_notebook=False,
                     pdf_hash=PDF_HASH, ink_key=ink, ink_pages=pages, modified=None)


def _reading(path: str = "/Reading/Finished/A Paper", **kw):
    return (_pdf(path, **kw), Placement("reading"))


def _fake_transcriber(calls: list):
    def transcribe(conn, marks, *, source_uri, limit):
        calls.append((source_uri, limit))
        return min(1, limit)
    return transcribe


def _no_queue(doc, where, pdf_bytes):
    raise AssertionError("nothing in this test should reach the drop folder")


def _run(conn, placed, *, manifest=None, transcribe_fn=None, limit=8, fetch_fn=None,
         ingest_fn=None, promote_fn=None, queue_fn=None, max_fetch=20):
    # Every injectable that would otherwise resolve the LIVE config is passed: the real ingest
    # once wrote these fake bytes into the server's data/incoming/paper/ (found 2026-10-19), and
    # the real promote and queue write into data/notes/ and data/incoming/.
    return annotate_sync(
        conn,
        placed=placed,
        manifest=manifest if manifest is not None else {},
        fetch_fn=fetch_fn or (lambda doc: Path("/nowhere/a.rmdoc")),
        read_fn=lambda p: _FakeDoc(),
        marks_fn=lambda doc: [_FakeMark()],
        transcribe_fn=transcribe_fn or _fake_transcriber([]),
        ingest_fn=ingest_fn or (lambda conn, doc, name: None),
        promote_fn=promote_fn or (lambda conn, uri, **kw: None),
        queue_fn=queue_fn or _no_queue,
        transcribe_limit=limit,
        max_fetch=max_fetch,
    )


def _library_row(conn, uri="data/incoming/paper/a.pdf"):
    with conn:
        conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
            "ingest_model) VALUES (?,'pdf',?,'r','A Paper','t')",
            (PDF_HASH, uri),
        )


def test_marks_key_to_the_corpus_document_by_content_hash(tmp_path):
    """The live failure (2026-08-06): marks keyed by device path joined to no document.

    The `.rmdoc` bundle carries the original PDF bytes and `content_hash` is sha256 of exactly
    those bytes, so the mapping is computed, never remembered.
    """
    conn = _conn(tmp_path)
    _library_row(conn)
    calls: list = []
    r = _run(conn, [_reading()], transcribe_fn=_fake_transcriber(calls))
    assert [o.status for o in r.outcomes] == ["annotated"]
    assert r.outcomes[0].hash_mapped
    assert r.outcomes[0].transcribed == 1
    assert calls == [("data/incoming/paper/a.pdf", 8)]   # transcription keyed the same way
    row = conn.execute("SELECT source_uri, source_run FROM pdf_annotations").fetchone()
    assert row["source_uri"] == "data/incoming/paper/a.pdf"
    assert row["source_run"] is not None            # provenance: which run stored this ink


def test_a_proposed_document_keys_by_device_path_and_never_ingests(tmp_path):
    """`Proposed` is pre-verdict: most proposals get rejected, so nothing there auto-ingests."""
    conn = _conn(tmp_path)
    ingests: list = []

    def ingest(conn, doc, name):
        ingests.append(name)
        return None

    r = _run(conn, [_reading("/Reading/Proposed/A Paper")], ingest_fn=ingest)
    assert r.outcomes[0].status == "annotated"
    assert not r.outcomes[0].hash_mapped and not r.outcomes[0].ingested
    assert ingests == []                            # his verdict, not the system's
    row = conn.execute("SELECT source_uri FROM pdf_annotations").fetchone()
    assert row["source_uri"] == "/Reading/Proposed/A Paper"


def test_a_chosen_hand_added_book_is_ingested_and_earlier_marks_rekeyed(tmp_path):
    """Moving a document to In-Progress/Finished is the owner's one reading gesture.

    A hand-added book (never a proposal) was the one reading document with no ingest path that
    didn't need a command typed. The move triggers ingest from the bundle's own PDF bytes, and
    marks captured while it sat un-ingested are re-keyed onto the new document.
    """
    conn = _conn(tmp_path)
    with conn:
        conn.execute(
            "INSERT INTO pdf_annotations (source_uri, doc_uuid, pdf_page, kind, bbox_key, "
            "captured_at) VALUES ('/Reading/Proposed/A Paper', 'uuid-1', 3, 'underline', "
            "'old-key', 't')"
        )

    def ingest(conn, doc, name):
        _library_row(conn, "data/incoming/paper/A Paper.pdf")
        return "data/incoming/paper/A Paper.pdf"

    promoted: list = []
    r = _run(conn, [_reading()], ingest_fn=ingest,
             promote_fn=lambda conn, uri, **kw: promoted.append(uri) or None)
    o = r.outcomes[0]
    assert o.status == "annotated" and o.ingested and not o.hash_mapped
    uris = {row["source_uri"] for row in conn.execute("SELECT source_uri FROM pdf_annotations")}
    assert uris == {"data/incoming/paper/A Paper.pdf"}   # old mark re-keyed, new mark joined
    assert promoted == ["data/incoming/paper/A Paper.pdf"]


def test_unchanged_ink_costs_nothing(tmp_path):
    conn = _conn(tmp_path)
    _library_row(conn)                  # handled: in the library, so nothing is left to retry
    manifest: dict = {}
    fetches: list = []

    def fetch(doc):
        fetches.append(doc.uuid)
        return Path("/nowhere/a.rmdoc")

    _run(conn, [_reading()], manifest=manifest, fetch_fn=fetch, limit=0)
    r2 = _run(conn, [_reading()], manifest=manifest, fetch_fn=fetch, limit=0)
    assert fetches == ["uuid-1"]                    # second run downloaded nothing
    assert [o.status for o in r2.outcomes] == ["unchanged"]


def test_one_failing_document_never_aborts_the_batch(tmp_path):
    conn = _conn(tmp_path)

    def fetch(doc):
        if doc.name == "Bad Doc":
            raise RuntimeError("cloud hiccup")
        return Path("/nowhere/a.rmdoc")

    r = _run(conn, [_reading("/Reading/Bad Doc", uuid="bad"),
                    _reading("/Reading/Good Doc", uuid="good")], fetch_fn=fetch, limit=0)
    assert {o.name: o.status for o in r.outcomes} == {"Bad Doc": "failed", "Good Doc": "annotated"}


# --- a document Locus sent ------------------------------------------------------------------------

def test_a_sent_document_is_never_ingested_and_its_notes_are_promoted_with_its_name(tmp_path):
    """Claude's document: the library must not take its prose as a source (invariant 5), and his
    notes on it go in with the passages they were about, titled by the document's own name."""
    conn = _conn(tmp_path)
    ingests: list = []
    promoted: list = []
    sent = (_pdf("/Projects/locus/2026-10-19 Locus plan v3"),
            Placement("sent", area="Projects", thing="locus", thing_folder="locus"))
    r = _run(conn, [sent], ingest_fn=lambda *a: ingests.append(a),
             promote_fn=lambda conn, uri, **kw: promoted.append((uri, kw)) or None)
    o = r.outcomes[0]
    assert o.status == "annotated" and o.role == "sent" and not o.ingested
    assert ingests == []
    assert promoted == [("/Projects/locus/2026-10-19 Locus plan v3",
                         {"title": "2026-10-19 Locus plan v3"})]
    stats = conn.execute("SELECT stats FROM agent_runs WHERE kind='annotate'").fetchone()["stats"]
    assert '"unmapped_to_corpus": 0' in stats     # keyed on the device by design, not a gap


def test_marks_follow_a_document_he_moved(tmp_path):
    """He refiled almost everything on 23 Oct. Marks stored under the old path move with the
    document, found by its uuid, instead of splitting into two sets."""
    conn = _conn(tmp_path)
    with conn:
        conn.execute(
            "INSERT INTO pdf_annotations (source_uri, doc_uuid, pdf_page, kind, bbox_key, "
            "note, captured_at) VALUES ('/Inbox/2026-10-19 Locus plan v3', 'uuid-1', 5, "
            "'margin_note', 'k5', 'why two columns?', 't')"
        )
    sent = (_pdf("/Projects/locus/2026-10-19 Locus plan v3"), Placement("sent", area="Projects"))
    _run(conn, [sent])
    rows = conn.execute("SELECT DISTINCT source_uri FROM pdf_annotations").fetchall()
    assert [r["source_uri"] for r in rows] == ["/Projects/locus/2026-10-19 Locus plan v3"]
    kept = conn.execute("SELECT note FROM pdf_annotations WHERE bbox_key='k5'").fetchone()
    assert kept["note"] == "why two columns?"     # the transcription moved with it


def test_a_sent_document_with_no_ink_is_not_downloaded(tmp_path):
    conn = _conn(tmp_path)
    fetches: list = []
    r = _run(conn, [(_pdf("/Inbox/2026-10-26 S3", pages=0), Placement("sent"))],
             fetch_fn=lambda doc: fetches.append(doc) or Path("/x"))
    assert fetches == [] and [o.status for o in r.outcomes] == ["unchanged"]


# --- a source -----------------------------------------------------------------------------------

def test_a_source_goes_to_the_drop_folder_first_and_its_marks_wait(tmp_path):
    """A lecture PDF in a course folder: not in the library yet, so it is queued under its area
    for locus-watch, and its ink is NOT read until the library has it (the key stays unset)."""
    conn = _conn(tmp_path)
    manifest: dict = {}
    queued: list = []
    src = (_pdf("/Studies/B14 Information Engineering Systems/B14_IP_MT26_L2.pdf"),
           Placement("source", area="Studies", thing="b14"))

    r1 = _run(conn, [src], manifest=manifest,
              queue_fn=lambda doc, where, b: queued.append((doc.name, where.area, b)) or "queued")
    assert [o.status for o in r1.outcomes] == ["queued"]
    assert queued == [("B14_IP_MT26_L2.pdf", "Studies", PDF_BYTES)]
    assert conn.execute("SELECT COUNT(*) FROM pdf_annotations").fetchone()[0] == 0
    assert "annotate:uuid-1" not in manifest

    _library_row(conn, "data/incoming/studies/B14_IP_MT26_L2.pdf")   # locus-watch ingested it
    promoted: list = []
    r2 = _run(conn, [src], manifest=manifest,
              promote_fn=lambda conn, uri, **kw: promoted.append(uri) or None)
    assert [o.status for o in r2.outcomes] == ["annotated"]
    row = conn.execute("SELECT source_uri FROM pdf_annotations").fetchone()
    assert row["source_uri"] == "data/incoming/studies/B14_IP_MT26_L2.pdf"
    assert promoted == ["data/incoming/studies/B14_IP_MT26_L2.pdf"]


def test_a_source_in_the_library_with_no_ink_costs_nothing(tmp_path):
    conn = _conn(tmp_path)
    _library_row(conn)
    fetches: list = []
    r = _run(conn, [(_pdf("/Projects/quast/lectures.pdf", pages=0), Placement("source",
                     area="Projects"))], fetch_fn=lambda d: fetches.append(d) or Path("/x"))
    assert fetches == [] and [o.status for o in r.outcomes] == ["unchanged"]


def test_queue_source_names_writes_and_never_requeues_a_quarantined_copy(tmp_path):
    incoming = tmp_path / "incoming"
    doc = _pdf("/Studies/B14 Information Engineering Systems/Lecture 4")
    where = Placement("source", area="Studies")
    assert _queue_source(doc, where, PDF_BYTES, incoming=incoming) == "queued"
    dest = incoming / "studies" / "Lecture 4.pdf"
    assert dest.read_bytes() == PDF_BYTES
    assert _queue_source(doc, where, PDF_BYTES, incoming=incoming) == "waiting"
    # locus-watch failed it: re-queuing would quarantine it again every half hour, for ever
    (incoming / ".quarantine").mkdir()
    dest.rename(incoming / ".quarantine" / "Lecture 4.pdf")
    assert _queue_source(doc, where, PDF_BYTES, incoming=incoming) == "quarantined"
    assert not dest.exists()


def test_fetches_are_bounded_per_run(tmp_path):
    """The first run after the fix meets every PDF since August; downloads are capped so one
    tick does not trip the cloud's rate limit, and the rest wait rather than vanish."""
    conn = _conn(tmp_path)
    manifest: dict = {}
    placed = [_reading(f"/Reading/Doc {i}", uuid=f"u{i}") for i in range(3)]
    r1 = _run(conn, placed, manifest=manifest, max_fetch=2, limit=0)
    assert [o.status for o in r1.outcomes] == ["annotated", "annotated", "deferred"]
    r2 = _run(conn, placed, manifest=manifest, max_fetch=2, limit=0)
    assert [o.status for o in r2.outcomes] == ["unchanged", "unchanged", "annotated"]


def test_a_chosen_book_whose_ingest_failed_is_tried_again(tmp_path):
    """27 Oct: the API credit ran out mid-run and a book in In-Progress failed to ingest. Recorded
    as handled, it would never have been tried again until he next wrote on it."""
    conn = _conn(tmp_path)
    manifest: dict = {}
    attempts: list = []

    def ingest(conn, doc, name):
        attempts.append(name)
        return None

    _run(conn, [_reading("/Reading/In-Progress/A Paper")], manifest=manifest, ingest_fn=ingest)
    _run(conn, [_reading("/Reading/In-Progress/A Paper")], manifest=manifest, ingest_fn=ingest)
    assert attempts == ["A Paper", "A Paper"]
    assert "annotate:uuid-1" not in manifest
