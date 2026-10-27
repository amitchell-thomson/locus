"""What Locus sent to the tablet (migration 0038), and what his notes on it carry (Slice 0.12).

Capture treats a recorded PDF as Claude's document and an unrecorded one in an area folder as a
source. The two mistakes are not equal: an unrecorded Claude document would be ingested into the
library as a source (invariant 5). So the record is made before the upload, a failed record stops
the push, and pushing a third-party paper opts out explicitly.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from locus.capture.sent import record_sent, sent_hashes
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.reading.deliver_remarkable import deliver_pdf


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "s.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _pdf(tmp_path: Path, body: bytes = b"%PDF-1.7 a plan") -> Path:
    p = tmp_path / "2026-10-27 Plan.pdf"
    p.write_bytes(body)
    return p


class _Rmapi:
    def __init__(self):
        self.calls: list = []

    def __call__(self, args):
        self.calls.append(args)
        return 0, "", ""


def test_record_is_the_sha256_of_the_bytes_and_idempotent(conn, tmp_path):
    pdf = _pdf(tmp_path)
    h = record_sent(conn, pdf, "/Inbox/")
    assert h == hashlib.sha256(pdf.read_bytes()).hexdigest()
    record_sent(conn, pdf, "Inbox")
    rows = conn.execute("SELECT filename, folder, origin FROM device_sent").fetchall()
    assert [tuple(r) for r in rows] == [("2026-10-27 Plan.pdf", "Inbox", "push")]
    assert sent_hashes(conn) == {h}


def test_an_authored_push_is_recorded_before_the_upload(tmp_path):
    order: list = []
    rmapi = _Rmapi()

    def record(pdf, folder):
        order.append(("record", folder, rmapi.calls[:]))

    deliver_pdf(_pdf(tmp_path), remote_folder="Inbox", runner=rmapi, record=record)
    assert order == [("record", "Inbox", [])]            # nothing had gone to rmapi yet
    assert any(c[0] == "put" for c in rmapi.calls)


def test_a_failed_record_stops_the_push(tmp_path):
    rmapi = _Rmapi()

    def record(pdf, folder):
        raise RuntimeError("database is locked")

    with pytest.raises(RuntimeError, match="locked"):
        deliver_pdf(_pdf(tmp_path), remote_folder="Inbox", runner=rmapi, record=record)
    assert rmapi.calls == []


def test_a_third_party_paper_is_not_recorded(tmp_path):
    recorded: list = []
    deliver_pdf(_pdf(tmp_path), remote_folder="Reading/Proposed", runner=_Rmapi(),
                authored=False, record=lambda p, f: recorded.append(p))
    assert recorded == []


def test_an_injected_transport_never_records_into_the_live_database(tmp_path, monkeypatch):
    """The inspector's rule: a caller that injected the transport injected everything it wants,
    so the default recorder (the live DB) is only built when the runner is real."""
    import locus.capture.sent as sent

    monkeypatch.setattr(sent, "default_recorder", lambda: pytest.fail("touched the live DB"))
    deliver_pdf(_pdf(tmp_path), remote_folder="Inbox", runner=_Rmapi())


def test_send_functions_pass_their_recorder_down(tmp_path, monkeypatch):
    """send.py builds its own runner, so it must hand deliver_pdf the recorder explicitly, as it
    does the inspector; otherwise every MCP `to_remarkable` push would go unrecorded."""
    import locus.reading.send as send

    seen: dict = {}

    def fake_deliver(pdf, **kw):
        seen.update(kw)

    monkeypatch.setattr(send, "deliver_pdf", fake_deliver)
    monkeypatch.setattr(send, "default_recorder", lambda: "THE-RECORDER")
    monkeypatch.setattr(send, "_rmapi_inspector", lambda binary: "INSPECTOR")
    monkeypatch.setattr(send, "_subprocess_runner", lambda binary: _Rmapi())
    pdf = _pdf(tmp_path)

    class _Cfg:
        class reading:
            send_folder = "Inbox"
            rmapi_binary = "rmapi"

    send.send_pdf(pdf, folder="Inbox", cfg=_Cfg())
    assert seen["record"] == "THE-RECORDER"


def test_a_margin_note_is_promoted_with_the_line_it_sits_beside(conn, tmp_path):
    """A note in the margin covers no printed words; without the line beside it, "why here?"
    reaches the library as nothing at all."""
    from locus.agent.promote import promote_reading_notes

    with conn:
        conn.execute(
            "INSERT INTO pdf_annotations (source_uri, doc_uuid, pdf_page, kind, bbox_key, "
            "covered_text, line_text, note, captured_at) VALUES (?,?,?,?,?,?,?,?,?)",
            ("/Projects/locus/2026-10-19 Locus plan v3", "u", 4, "margin_note", "k", "",
             "The capture loop keys change detection on the staged render.",
             "why not the cloud?", "2026-10-20T00:00:00+00:00"),
        )
    p = promote_reading_notes(conn, "/Projects/locus/2026-10-19 Locus plan v3",
                              title="2026-10-19 Locus plan v3", notes_dir=tmp_path)
    text = p.path.read_text()
    assert "why not the cloud?" in text
    assert "> The capture loop keys change detection on the staged render." in text
    assert "Reading notes — 2026-10-19 Locus plan v3" in text
