"""Attachments and career files (H1.12): what is never read, what is redacted, where it lands."""

from __future__ import annotations

import json
import sqlite3

import pytest

from locus.heuris import documents, files_push
from locus.heuris.mail_push import build
from locus.heuris.sources import files as files_src
from locus.heuris.sources.mail import items


@pytest.fixture(autouse=True)
def _state(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCUS_STATE_DIR", str(tmp_path / "state"))


@pytest.mark.parametrize("name", [
    "AMT UK Passport.pdf", "Driver’s license .pdf", "HMRC - Starter Checklist (P46).pdf",
    "Intern Information Form.pdf", "2026-09-30_Account Statement - Monthly_0667.pdf",
    "Casual Worker Data Collection Form - Feb24.docx", "Aaron Rose CV.pdf", "~$S Cover Paras.docx",
    "photo.jpg"])
def test_identity_bank_and_other_peoples_papers_are_never_read(name):
    assert documents.denied(name)


@pytest.mark.parametrize("name", [
    "Millennium Questionnaire.pdf", "Offer Letter - UK - Intern 2026-10-28.pdf",
    "Thomson, A - BHAMSL- Summer Intern Contract .docx.pdf", "Alec_Mitchell-Thomson_CV_jul_2026.pdf",
    "AMT CV 2025 Dec.docx", "advice.md"])
def test_offers_contracts_forms_and_his_cvs_are_read(name):
    assert documents.denied(name) is None


def test_each_identifier_kind_is_redacted():
    text = ("NI number: AB 12 34 56 C. Passport No: 123456789.\n"
            "P<GBRMITCHELL<THOMSON<<ALEC<<<<<<<<<<<<<<<<<<<\n"
            "IBAN GB29 NWBK 6016 1331 9268 19. Sort code: 60-16-13 Account number: 31926819.\n"
            "Card 4111 1111 1111 1111. Salary £1,000 per week from 21 June 2027; ref 20260628.")
    out, counts = documents.scrub_identifiers(text)
    for secret in ("AB 12 34 56 C", "123456789", "MITCHELL<THOMSON", "NWBK", "60-16-13", "31926819",
                   "4111 1111"):
        assert secret not in out, secret
    assert set(counts) >= {"ni-number", "passport-no", "mrz", "iban", "bank-account", "card"}
    assert "£1,000 per week from 21 June 2027" in out and "20260628" in out   # terms survive


def _keryx(tmp_path, attachments, raw_ref):
    db = tmp_path / "keryx.db"
    c = sqlite3.connect(db)
    c.executescript("""
      CREATE TABLE messages (id INTEGER, account TEXT, thread_id TEXT, message_id TEXT, from_json TEXT,
        to_json TEXT, cc_json TEXT, subject TEXT, date INTEGER, in_inbox INTEGER, read INTEGER,
        flagged INTEGER, list_headers_json TEXT, body_text TEXT, attachments_json TEXT, raw_ref TEXT);
      CREATE TABLE tags (account TEXT, thread_id TEXT, kind TEXT, reason TEXT);
      CREATE TABLE links (account TEXT, thread_id TEXT, app TEXT, kind TEXT, target TEXT, title TEXT);
    """)
    c.execute("INSERT INTO messages VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
              (7, "personal", "t1", "<a>", '{"email":"alec.mitchellthomson@gmail.com","name":"Alec"}', "[]", "[]",
               "Meet Millennium Questionnaire", 1782600000, 1, 1, 0, "[]", "Please find attached.",
               json.dumps(attachments), str(raw_ref)))
    c.execute("INSERT INTO links VALUES ('personal','t1','locus','item','millennium','Millennium')")
    c.commit()
    c.close()
    return db


def test_an_attachment_is_read_from_mails_store_redacted_and_indexed_beside_its_message(tmp_path):
    msgs = tmp_path / "mbox" / "Data" / "Messages"
    msgs.mkdir(parents=True)
    raw = msgs / "10738.partial.emlx"
    raw.write_text("partial")
    adir = tmp_path / "mbox" / "Data" / "Attachments" / "10738"
    (adir / "2").mkdir(parents=True)
    (adir / "2" / "answers.txt").write_text("Preferred track: Quant Research first, then QD. NI: AB123456C")
    (adir / "3").mkdir(parents=True)
    (adir / "3" / "AMT UK Passport.pdf").write_bytes(b"%PDF never opened")
    db = _keryx(tmp_path, [{"id": 2, "name": "answers.txt"}, {"id": 3, "name": "AMT UK Passport.pdf"},
                           {"id": 4, "name": "image001.png"}], raw)
    stage = tmp_path / "stage"
    r = build(db, stage)
    assert r.attachments_read == 1 and r.attachments_skipped["an identity, tax or bank document"] == 1
    got = list(items(stage))
    att = [i for i in got if i.kind == "attachment"]
    assert len(att) == 1 and "Quant Research first" in att[0].text and "AB123456C" not in att[0].text
    assert att[0].thing == "millennium" and att[0].parent == "mail:personal/t1"
    assert att[0].meta["message"] == "mail:personal/t1/7"


def test_career_files_skip_identity_papers_and_send_duplicates_once(tmp_path):
    career = tmp_path / "Career"
    (career / "applications" / "Jane Street").mkdir(parents=True)
    (career / "applications" / "Jane Street" / "notes.md").write_text("Jane Street onsite: two probability questions.")
    (career / "Career Documents").mkdir()
    (career / "Career Documents" / "AMT UK Passport.pdf").write_bytes(b"%PDF")
    (career / "alpha-fund" / ".venv").mkdir(parents=True)
    (career / "alpha-fund" / ".venv" / "LICENSE.txt").write_text("MIT")
    dl = tmp_path / "Downloads"
    dl.mkdir()
    (dl / "Offer Letter - UK - Intern.txt").write_text("Start date 21 June 2027.")
    (dl / "Offer Letter - UK - Intern-1.txt").write_text("Start date 21 June 2027.")
    (dl / "B14 lecture.txt").write_text("not a career file")
    stage = tmp_path / "files"
    r = files_push.build(stage, roots=[("career", career, None), ("downloads", dl, files_push.DOWNLOADS_KEEP)])
    assert (r.read, r.duplicates) == (2, 1) and r.skipped["an identity, tax or bank document"] == 1
    got = {i.ref: i for i in files_src.items(stage)}
    assert set(got) == {"file:career/applications/Jane Street/notes.md", "file:downloads/Offer Letter - UK - Intern-1.txt"} \
        or set(got) == {"file:career/applications/Jane Street/notes.md", "file:downloads/Offer Letter - UK - Intern.txt"}
    assert got["file:career/applications/Jane Street/notes.md"].thing == "jane-street"


def test_a_partial_read_sends_nothing(tmp_path, monkeypatch):
    sent = []
    monkeypatch.setattr(files_push.subprocess, "run", lambda *a, **k: sent.append(a))
    bad = files_push.Report()
    bad.skipped["unreadable"] = 3
    monkeypatch.setattr(files_push, "build", lambda stage: bad)
    r = files_push.push()
    assert r.error and "nothing sent" in r.error and not sent


def test_cv_books_and_mandates_are_not_read():
    assert documents.denied("oqts-cvs-drinks-dinner-jump-trading-2026-10-23.pdf")
    assert documents.denied("New College JCR Mandate form Signed Alec and Max.pdf")


def test_a_cross_drawn_over_a_checkbox_is_read_as_the_chosen_option():  # Y2
    import pymupdf

    doc = pymupdf.open()
    page = doc.new_page()
    page.insert_text((40, 60), "Question 1")
    page.insert_text((60, 100), "A. Building fast systems")
    page.insert_text((300, 100), "B. Finding patterns in data")
    for x in (240, 500):
        page.draw_rect(pymupdf.Rect(x, 88, x + 18, 106), color=None, fill=(0.9, 0.9, 0.9))
    page.draw_line((502, 90), (516, 104))   # the cross, over B's box only
    page.draw_line((516, 90), (502, 104))
    text = documents.extract("form.pdf", doc.tobytes())
    marked = text[text.index("Marked by hand"):]
    assert "B. Finding patterns in data" in marked and "A. Building" not in marked
