"""Loop A orchestrator (agent-layer §8.1; capture from the cloud since Slice 0.12). Every model
call and the cloud fetch are injected, so this runs offline on a real migrated DB: it asserts the
note is filed with the right frontmatter, the run is journaled, idempotency keys on the INK (not
on a render, and not on where the notebook sits), the backlog is bounded per run, and per-doc
failures don't abort the batch."""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.capture.device import DeviceDoc, Placement
from locus.capture.fillin import FillResult
from locus.capture.loop_a import capture_sync
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


def _notebook(uuid: str, name: str, folders: tuple[str, ...], *, ink: str = "ink-1",
              pages: int = 2, modified: str | None = None) -> DeviceDoc:
    return DeviceDoc(uuid=uuid, name=name, folders=folders, is_notebook=True, pdf_hash=None,
                     ink_key=ink, ink_pages=pages, modified=modified)


def _transcript(text: str) -> Transcript:
    t = Transcript(input_tokens=1000, output_tokens=200)
    t.pages.append(PageTranscript(page=1, markdown=text, illegible=text.count("[illegible]"),
                                  uncertain=text.count("[?]")))
    return t


class _Fakes:
    """The cloud fetch, the render and the three model passes, all counting their calls."""

    def __init__(self, fail: set[str] = frozenset(), empty: set[str] = frozenset()):
        self.fetched: list[str] = []
        self.transcribed: list[str] = []
        self.enriched: list[str] = []
        self.fail = fail
        self.empty = empty
        self.staging = Path("/tmp")

    def fetch(self, doc):
        self.fetched.append(doc.uuid)
        return Path(f"/nowhere/{doc.uuid}.rmdoc")

    def notebook(self, doc, bundle):
        """Stands in for render + per-page transcription; the real one is tested in
        test_capture_notebook.py."""
        if doc.uuid in self.fail:
            raise RuntimeError("vision API 500")
        if doc.uuid in self.empty:
            return None
        self.transcribed.append(doc.uuid)
        out = self.staging / f"{doc.uuid}.pdf"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(b"%PDF-1.7\n" + doc.uuid.encode())
        t = _transcript("# Notes\n\nSwap [illegible] resets on SOFR[?].")
        return t, out, len(t.pages)

    def fillin(self, md):
        return FillResult(markdown=md.replace("[illegible]", "⟦leg⟧"), filled=1, total_gaps=2)

    def enrich(self, note_path, note_text, *, conn, run_id):
        self.enriched.append(str(note_path))
        return EnrichResult(related=2, wrote_block=True)


def _run(conn, tmp_path, placed, fakes, **kw):
    kw.setdefault("max_notebooks", 10)
    fakes.staging = tmp_path / "stage"
    return capture_sync(
        conn, placed=placed, staging_dir=tmp_path / "stage", notes_dir=tmp_path / "notes",
        manifest_path=tmp_path / "m.json", ingest=False, fetch_fn=fakes.fetch,
        notebook_fn=fakes.notebook, fillin_fn=fakes.fillin,
        enrich_fn=fakes.enrich, now="2026-10-27T00:00:00+00:00", **kw,
    )


def test_capture_files_note_with_provenance_and_journals(tmp_path, conn):
    doc = _notebook("81ae50ba-x", "Jargon Sheet", ("Careers", "Brevan Howard"))
    where = Placement("notebook", area="Careers", thing="brevan-howard-2026",
                      thing_folder="Brevan Howard")
    fakes = _Fakes()
    r = _run(conn, tmp_path, [(doc, where)], fakes)

    assert r.captured == 1
    note = tmp_path / "notes" / "jargon-sheet-81ae50ba.md"
    text = note.read_text()
    # the area decides the category; the path and the registry thing are kept as provenance
    assert "category: career" in text and "maturity: rough" in text
    assert "remarkable_folder: /Careers/Brevan Howard" in text
    assert "remarkable_uuid: 81ae50ba-x" in text
    assert "area: Careers" in text and "thing: brevan-howard-2026" in text
    assert "⟦leg⟧" in text
    assert fakes.enriched == [str(note)]
    row = conn.execute("SELECT kind, status, stats FROM agent_runs").fetchone()
    assert row["kind"] == "capture" and row["status"] == "ok"
    assert '"captured": 1' in row["stats"] and '"cost_usd"' in row["stats"]


def test_jot_at_the_root_is_a_note_with_no_area(tmp_path, conn):
    fakes = _Fakes()
    _run(conn, tmp_path, [(_notebook("jot-1", "Jot", ()), Placement("notebook"))], fakes)
    text = next((tmp_path / "notes").glob("*.md")).read_text()
    assert "category: note" in text
    assert "area:" not in text and "thing:" not in text   # empty fields are not written


def test_unchanged_ink_costs_nothing_even_after_a_move(tmp_path, conn):
    """Keyed on the ink, so filing a notebook into a new folder does not re-bill a transcription
    (moving one changes its metadata, and so its document hash, but not a stroke)."""
    fakes = _Fakes()
    before = _notebook("u1", "Doc", ("Projects", "quast"))
    _run(conn, tmp_path, [(before, Placement("notebook", area="Projects"))], fakes)
    moved = _notebook("u1", "Doc", ("Studies", "B15 Control Systems"))
    r2 = _run(conn, tmp_path, [(moved, Placement("notebook", area="Studies"))], fakes)
    assert fakes.fetched == ["u1"] and fakes.transcribed == ["u1"]
    assert [o.status for o in r2.outcomes] == ["unchanged"]


def test_new_ink_is_captured_again(tmp_path, conn):
    fakes = _Fakes()
    _run(conn, tmp_path, [(_notebook("u1", "Doc", ("Projects",), ink="a"), Placement("notebook"))],
         fakes)
    _run(conn, tmp_path, [(_notebook("u1", "Doc", ("Projects",), ink="b"), Placement("notebook"))],
         fakes)
    assert fakes.transcribed == ["u1", "u1"]


def test_only_notebooks_are_loop_a(tmp_path, conn):
    """A PDF is never transcribed as his writing, whoever made it (invariant 5)."""
    fakes = _Fakes()
    pdf = DeviceDoc("p1", "Lecture 4", ("Studies", "B14 Information Engineering Systems"),
                    is_notebook=False, pdf_hash="h", ink_key="k", ink_pages=3, modified=None)
    r = _run(conn, tmp_path, [(pdf, Placement("source", area="Studies")),
                              (pdf, Placement("sent", area="Studies"))], fakes)
    assert r.outcomes == [] and fakes.fetched == []


def test_an_empty_notebook_is_recorded_without_a_fetch(tmp_path, conn):
    fakes = _Fakes()
    r = _run(conn, tmp_path, [(_notebook("e1", "New", (), pages=0), Placement("notebook"))], fakes)
    assert fakes.fetched == [] and [o.status for o in r.outcomes] == ["unchanged"]


def test_a_notebook_the_tablet_agent_captured_is_seeded_not_rebilled(tmp_path, conn):
    """Transcribed from the agent's render before it stopped: recorded as current on first sight
    rather than re-transcribed to find out whether anything moved."""
    import json

    (tmp_path / "stage").mkdir()
    (tmp_path / "stage" / "old-1.pdf").write_bytes(b"%PDF device render")
    (tmp_path / "m.json").write_text(json.dumps({"captures": {"old-1": "rasterhash"}}))
    fakes = _Fakes()
    doc = (_notebook("old-1", "Swaps", ("Careers", "Brevan Howard")), Placement("notebook"))
    r = _run(conn, tmp_path, [doc], fakes)
    assert [o.status for o in r.outcomes] == ["seeded"] and fakes.fetched == []
    r2 = _run(conn, tmp_path, [doc], fakes)
    assert [o.status for o in r2.outcomes] == ["unchanged"]


def test_recapture_overrides_the_seed(tmp_path, conn):
    import json

    (tmp_path / "stage").mkdir()
    (tmp_path / "stage" / "old-1.pdf").write_bytes(b"%PDF device render")
    (tmp_path / "m.json").write_text(json.dumps({"captures": {"old-1": "rasterhash"}}))
    fakes = _Fakes()
    _run(conn, tmp_path, [(_notebook("old-1", "Swaps", ()), Placement("notebook"))], fakes,
         recapture=True)
    assert fakes.transcribed == ["old-1"]


def test_the_backlog_is_bounded_per_run_and_resumes(tmp_path, conn):
    """Twelve weeks of notebooks arrive at once after the fix; transcription is billed, so a run
    takes `max_notebooks` and the rest wait for the next tick rather than being dropped."""
    fakes = _Fakes()
    placed = [(_notebook(f"n{i}", f"Notebook {i}", ()), Placement("notebook")) for i in range(3)]
    r1 = _run(conn, tmp_path, placed, fakes, max_notebooks=2)
    assert [o.status for o in r1.outcomes] == ["captured", "captured", "deferred"]
    r2 = _run(conn, tmp_path, placed, fakes, max_notebooks=2)
    assert [o.status for o in r2.outcomes] == ["unchanged", "unchanged", "captured"]
    stats = conn.execute("SELECT stats FROM agent_runs ORDER BY id LIMIT 1").fetchone()["stats"]
    assert '"deferred": 1' in stats


def test_one_doc_failure_does_not_abort_batch(tmp_path, conn):
    fakes = _Fakes(fail={"bad"})
    placed = [(_notebook("good", "Good", ()), Placement("notebook")),
              (_notebook("bad", "Bad", ()), Placement("notebook"))]
    r = _run(conn, tmp_path, placed, fakes)
    by = {o.name: o for o in r.outcomes}
    assert by["Good"].status == "captured"
    assert by["Bad"].status == "failed" and "500" in by["Bad"].error
    assert conn.execute("SELECT status FROM agent_runs").fetchone()["status"] == "ok"
    # a failed notebook is tried again next run: its ink key was not recorded
    fakes.fail = set()
    r2 = _run(conn, tmp_path, placed, fakes)
    assert {o.name: o.status for o in r2.outcomes} == {"Good": "unchanged", "Bad": "captured"}


def test_unmatched_registry_folders_are_journaled(tmp_path, conn):
    """`locus status` names them from here (see test_status)."""
    fakes = _Fakes()
    placed = [(_notebook("u", "x", ("Studies", "B99 Typo")),
               Placement("notebook", area="Studies", thing=None, thing_folder="B99 Typo"))]
    r = _run(conn, tmp_path, placed, fakes)
    assert r.unmatched_folders == ["Studies/B99 Typo"]
    assert "B99 Typo" in conn.execute("SELECT stats FROM agent_runs").fetchone()["stats"]


def test_note_is_dated_by_the_device_not_by_capture_time(tmp_path, conn):
    """A note's `date:` frontmatter must be when the owner last wrote in the notebook.

    extract/textdoc reads it into documents.source_date, which becomes
    belief_positions.dated_at. The first live 12-document capture dated every note "today",
    collapsing the understanding-evolution trajectory to a single point.
    """
    doc = _notebook("u-dated", "Rates Foundations", (), modified="2026-07-10T13:13:56+00:00")
    _run(conn, tmp_path, [(doc, Placement("notebook"))], _Fakes())
    text = next((tmp_path / "notes").glob("*.md")).read_text()
    assert "date: 2026-07-10" in text
    assert "date: 2026-10-27" not in text


def test_note_without_a_device_date_falls_back_to_capture_time(tmp_path, conn):
    """No invented dates: a document the device reports no timestamp for uses capture time."""
    _run(conn, tmp_path, [(_notebook("u-undated", "Scribble", ()), Placement("notebook"))],
         _Fakes())
    assert "date: 2026-10-27" in next((tmp_path / "notes").glob("*.md")).read_text()


def test_a_notebook_whose_ink_was_erased_is_recorded_not_failed_every_run(tmp_path, conn):
    """G-Research, 28 Oct: a stroke layer with no strokes; composing it raised "cannot save with
    zero pages" on every run. Nothing to read is recorded as read, and not fetched again."""
    fakes = _Fakes(empty={"g1"})
    doc = (_notebook("g1", "G-Research", ("Careers", "G-Research")), Placement("notebook"))
    r = _run(conn, tmp_path, [doc], fakes)
    assert [o.status for o in r.outcomes] == ["unchanged"] and fakes.transcribed == []
    r2 = _run(conn, tmp_path, [doc], fakes)
    assert [o.status for o in r2.outcomes] == ["unchanged"] and fakes.fetched == ["g1"]
