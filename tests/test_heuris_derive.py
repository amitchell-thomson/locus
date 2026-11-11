"""Facts from mail and the calendar (H2.4): the quote is verified, authority is read from the
sender, a fact he stated is never overwritten by mail, and a run batches what changed."""

from __future__ import annotations

import json
from datetime import datetime, timezone

import pytest

from locus.agent import claude
from locus.heuris import db as hdb
from locus.heuris import derive as D
from locus.heuris import facts as F
from locus.heuris.index import Item, index
from tests.test_heuris_index import fake_embed


def mail(ref, day, sender, name, subject, text, why="a person (named sender)"):
    acct, thread, _ = ref.split(":")[1].split("/")
    return Item(ref=ref, kind="mail", at=datetime(2026, 11, day, 9, tzinfo=timezone.utc), title=subject,
                text=f"{name}: {text}", parent=f"mail:{acct}/{thread}", people=[(sender, "from")],
                meta={"from": sender, "from_name": name, "why": why})


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    index(c, "mail", [
        mail("mail:oxford/1/11", 9, "isobel.voysey@cs.ox.ac.uk", "Isobel Voysey", "B18 tutorials 25th/26th Nov",
             "Our tutorials will be on Wed 25th Nov at 3pm and 4pm and Thu 26th Nov at 3pm. "
             "The deadline for the B18 examples sheet 1 is Wed 18th 5pm.", why="an Oxford list or office"),
        mail("mail:personal/2/21", 2, "recruiting@mlp.com", "Millennium Recruiting", "Your offer",
             "We are delighted to confirm your internship runs from 21 June 2027 to 27 August 2027.",
             why="a firm or recruiting system"),
        mail("mail:personal/3/31", 5, "alec.mitchellthomson@gmail.com", "", "Re: BH",
             "I have decided to withdraw from the Brevan Howard internship.", why="you sent it"),
        mail("mail:personal/4/41", 6, "news@society.org", "Society", "Weekly",
             "Events this week. Unsubscribe here.", why="a quant or careers society"),
    ], embed=fake_embed)
    yield c
    c.close()


def out(*facts):
    return json.dumps({"facts": [dict(zip(("subject", "slot", "value", "as_of", "quote"), f)) for f in facts]})


def test_a_course_fact_from_oxford_is_written_with_its_quote(conn):
    msgs = D.thread_messages(conn, "mail:oxford/1")
    a = D.apply_facts(conn, msgs, out(
        ("b18", "tutorials:2026-11-25", "Wed 25 Nov 15:00 and 16:00; Thu 26 Nov 15:00", "2026-11-09",
         "Our tutorials will be on Wed 25th Nov at 3pm and 4pm and Thu 26th Nov at 3pm."),
        ("b18", "sheet_due:sheet-1", "2026-11-18 17:00", "2026-11-09",
         "The deadline for the B18 examples sheet 1 is Wed 18th 5pm."),
    ))
    assert a.written == 2
    f = F.current(conn, "b18", "sheet_due")[0]
    assert f.sources == ["mail:oxford/1/11"] and f.route == "mail" and f.quote.startswith("The deadline")
    assert F.automatic_log(conn)[0]["fact"] in {x.id for x in F.current(conn, "b18")}


def test_an_invented_quote_loses_the_fact(conn):
    msgs = D.thread_messages(conn, "mail:oxford/1")
    a = D.apply_facts(conn, msgs, out(("b18", "tutor", "Isobel Voysey", "2026-11-09", "I am your tutor for B18.")))
    assert a.dropped_quote == 1 and not F.current(conn, "b18")
    a = D.apply_facts(conn, msgs, out(("b18", "lecturer", "x", "2026-11-09", "Our tutorials will be on Wed 25th Nov")))
    assert a.dropped_slot == 1


def test_the_firm_writes_its_application_but_never_over_his_own_word(conn):
    msgs = D.thread_messages(conn, "mail:personal/2")
    q = "We are delighted to confirm your internship runs from 21 June 2027 to 27 August 2027."
    a = D.apply_facts(conn, msgs, out(("millennium", "start", "2027-06-21", "2026-11-02", q)))
    assert a.written == 1
    F.write(conn, "millennium", "end", "2027-08-20", sources=["chat"], route="chat", as_of="2026-10-01")
    a = D.apply_facts(conn, msgs, out(("millennium", "end", "2027-08-27", "2026-11-02", q)))
    assert a.proposed == 1 and F.proposed(conn)[0].why == "conflicts with what he said"
    assert F.current(conn, "millennium", "end")[0].value == "2027-08-20"


def test_his_own_mail_and_facts_about_him_wait_in_approve(conn):
    msgs = D.thread_messages(conn, "mail:personal/3")
    a = D.apply_facts(conn, msgs, out(("bh-2027", "stage", "withdrawing", "2026-11-05",
                                       "I have decided to withdraw from the Brevan Howard internship.")))
    assert a.proposed == 1 and F.proposed(conn)[0].why == "his own mail"


def test_an_older_fact_from_mail_is_history_not_a_proposal(conn):
    F.write(conn, "millennium", "stage", "offer accepted", sources=["file"], route="import", as_of="2026-11-10")
    msgs = D.thread_messages(conn, "mail:personal/2")
    q = "We are delighted to confirm your internship runs from 21 June 2027 to 27 August 2027."
    a = D.apply_facts(conn, msgs, out(("millennium", "stage", "offer made", "2026-11-02", q)))
    assert a.history == 1 and not F.proposed(conn)
    assert [f.value for f in F.history(conn, "millennium", "stage")] == ["offer accepted", "offer made"]


def test_newsletters_are_not_read(conn):
    assert not D.worth_reading(D.thread_messages(conn, "mail:personal/4"))
    assert D.worth_reading(D.thread_messages(conn, "mail:oxford/1"))


def test_interviews_come_from_the_calendar(conn):
    index(conn, "calendar", [
        Item(ref="event:careers#1", kind="event", at=datetime(2026, 10, 1, 9, tzinfo=timezone.utc),
             title="Jump onsite interview", thing="jump", area="Careers"),
        Item(ref="event:careers#2", kind="event", at=datetime(2026, 10, 2, 9, tzinfo=timezone.utc),
             title="Coffee", thing="jump", area="Careers"),
    ], embed=fake_embed)
    assert D.calendar_facts(conn, since="2026-06-01") == 1
    assert D.calendar_facts(conn, since="2026-06-01") == 0
    assert F.current(conn, "jump", "interview")[0].slot == "interview:2026-10-01"


def test_a_run_batches_changed_threads_and_applies_them_next_run(conn, monkeypatch):
    sent = {}
    monkeypatch.setattr(claude, "credit_allows", lambda est, **k: (True, "ok"))

    def submit(reqs):
        sent["reqs"] = reqs
        return "b1"

    monkeypatch.setattr(claude, "batch_submit", submit)
    monkeypatch.setattr(claude, "batch_collect", lambda bid, model: None)
    monkeypatch.setattr(claude, "batch_ended", lambda bid: False)
    rep = D.run_facts(conn, since="2026-11-01")
    assert rep.submitted == 3 and rep.skipped_threads == 1
    assert "Isobel Voysey" in "".join(r.user for r in sent["reqs"])
    # Nothing new: nothing submitted.
    assert D.run_facts(conn, since="2026-11-01").submitted == 0
    cid = next(r.custom_id for r in sent["reqs"] if "Isobel" in r.user)
    reply = out(("b18", "sheet_due:sheet-1", "2026-11-18 17:00", "2026-11-09",
                 "The deadline for the B18 examples sheet 1 is Wed 18th 5pm."))
    monkeypatch.setattr(claude, "batch_collect", lambda bid, model: {cid: claude.ClaudeResult(text=reply)})
    monkeypatch.setattr(claude, "batch_ended", lambda bid: True)
    rep = D.run_facts(conn, since="2026-11-01")
    assert rep.collected == 1 and rep.applied.written == 1
    assert D.run_facts(conn, since="2026-11-01").collected == 0


def test_a_refused_credit_leaves_threads_for_the_next_run(conn, monkeypatch):
    monkeypatch.setattr(claude, "credit_allows", lambda est, **k: (False, "pace spent"))
    rep = D.run_facts(conn, since="2026-11-01")
    assert rep.submitted == 0 and "pace spent" in rep.note
    assert len(D.threads_to_read(conn, since="2026-11-01")) == 3


def test_an_interview_event_is_matched_to_its_application_by_its_title():
    assert D.app_for_event("Millennium Interview", "2026-10-16T15:00", None) == "millennium"
    assert D.app_for_event("Call with Millennium QD", "2026-10-29T09:30", None) == "millennium"
    assert D.app_for_event("G-Research AI/ML interview", "2026-10-29T09:30", None) == "gresearch-ml"
    assert D.app_for_event("Brevan Howard onsite", "2025-11-03T09:30", None) == "bh-2026"
    assert D.app_for_event("Quant interview", "2026-10-29T09:30", None) is None
