"""Loop A orchestrator (agent-layer §8.1, Phase 1 ④). Every model call is injected, so this runs
offline on a real migrated DB: it asserts the note is filed with the right frontmatter, the run is
journaled, idempotency skips an unchanged raster, and per-doc failures don't abort the batch."""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.capture.fillin import FillResult
from locus.capture.loop_a import capture_sync
from locus.capture.remarkable import CaptureItem
from locus.capture.transcribe import PageTranscript, Transcript
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.enrich.related import EnrichResult


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "c.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _item(staging: Path, uuid: str, name: str, folder: str, category: str) -> CaptureItem:
    pdf = staging / f"{uuid}.pdf"
    pdf.write_bytes(b"%PDF-1.7\n" + uuid.encode())  # distinct content per uuid
    return CaptureItem(uuid=uuid, pdf_path=pdf, name=name, folder=folder, category=category)


def _transcript(text: str) -> Transcript:
    t = Transcript(input_tokens=1000, output_tokens=200)
    t.pages.append(PageTranscript(page=1, markdown=text, illegible=text.count("[illegible]"),
                                  uncertain=text.count("[?]")))
    return t


def _fakes(staging, items, enrich_calls):
    def identify(_s):
        return items, ["excluded-uuid"]

    def transcribe(pdf_path):
        return _transcript("# Notes\n\nSwap [illegible] resets on SOFR[?].")

    def fillin(md):
        return FillResult(markdown=md.replace("[illegible]", "⟦leg⟧"), filled=1, total_gaps=2)

    def enrich(note_path, note_text, *, conn, run_id):
        enrich_calls.append((str(note_path), run_id))
        return EnrichResult(related=2, wrote_block=True)

    return identify, transcribe, fillin, enrich


def test_capture_files_note_with_provenance_and_journals(tmp_path, conn):
    staging, notes = tmp_path / "stage", tmp_path / "notes"
    staging.mkdir(); notes.mkdir()
    items = [_item(staging, "81ae50ba-x", "Jargon Sheet", "brevan_howard", "note")]
    enrich_calls: list = []
    identify, transcribe, fillin, enrich = _fakes(staging, items, enrich_calls)

    r = capture_sync(conn, staging_dir=staging, notes_dir=notes, manifest_path=tmp_path / "m.json",
                     ingest=False, identify_fn=identify, transcribe_fn=transcribe,
                     fillin_fn=fillin, enrich_fn=enrich, now="2026-07-28T00:00:00+00:00")

    assert r.captured == 1 and r.unmapped == ["excluded-uuid"]
    note = notes / "jargon-sheet-81ae50ba.md"
    assert note.exists()
    text = note.read_text()
    # frontmatter provenance: category (note), rough, folder kept, uuid, source raster
    assert "category: note" in text and "maturity: rough" in text
    assert "remarkable_folder: brevan_howard" in text
    assert "remarkable_uuid: 81ae50ba-x" in text
    assert "⟦leg⟧" in text  # filled body written
    assert enrich_calls and enrich_calls[0][0] == str(note)  # enrich ran on the note
    # run journaled with stats
    row = conn.execute("SELECT kind, status, stats FROM agent_runs").fetchone()
    assert row["kind"] == "capture" and row["status"] == "ok"
    assert '"captured": 1' in row["stats"] and '"cost_usd"' in row["stats"]


def test_unchanged_raster_is_skipped_no_retranscribe(tmp_path, conn):
    staging, notes = tmp_path / "stage", tmp_path / "notes"
    staging.mkdir(); notes.mkdir()
    items = [_item(staging, "u1", "Doc", "rough_notes", "note")]
    calls = {"transcribe": 0}
    identify, _t, fillin, enrich = _fakes(staging, items, [])

    def counting_transcribe(pdf):
        calls["transcribe"] += 1
        return _transcript("body")

    kw = dict(staging_dir=staging, notes_dir=notes, manifest_path=tmp_path / "m.json", ingest=False,
              identify_fn=identify, transcribe_fn=counting_transcribe, fillin_fn=fillin, enrich_fn=enrich)
    capture_sync(conn, **kw)
    r2 = capture_sync(conn, **kw)  # nothing changed on disk
    assert calls["transcribe"] == 1  # NOT re-transcribed (manifest gate)
    assert r2.outcomes[0].status == "unchanged"


def test_one_doc_failure_does_not_abort_batch(tmp_path, conn):
    staging, notes = tmp_path / "stage", tmp_path / "notes"
    staging.mkdir(); notes.mkdir()
    items = [_item(staging, "good", "Good", "rough_notes", "note"),
             _item(staging, "bad", "Bad", "rough_notes", "note")]
    identify, transcribe, fillin, enrich = _fakes(staging, items, [])

    def flaky_transcribe(pdf):
        if pdf.stem == "bad":
            raise RuntimeError("vision API 500")
        return _transcript("ok")

    r = capture_sync(conn, staging_dir=staging, notes_dir=notes, manifest_path=tmp_path / "m.json",
                     ingest=False, identify_fn=identify, transcribe_fn=flaky_transcribe,
                     fillin_fn=fillin, enrich_fn=enrich)
    by = {o.name: o for o in r.outcomes}
    assert by["Good"].status == "captured"
    assert by["Bad"].status == "failed" and "500" in by["Bad"].error
    assert conn.execute("SELECT status FROM agent_runs").fetchone()["status"] == "ok"
