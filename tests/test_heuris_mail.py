"""The mail rule (H1.2, your answers of 10 Nov) and the quote cleaner (H1.4)."""

from __future__ import annotations

import json
import sqlite3

from locus.heuris.mail_push import Ctx, build, keep
from locus.heuris.sources.mail import clean, items

CTX = Ctx(replied={("oxford", "77")}, wrote_to={"friend@example.org"})


def m(email, name="", subject="hello", account="personal", thread="1", headers=(), flagged=0):
    return {"account": account, "thread_id": thread, "subject": subject, "flagged": flagged,
            "from_json": json.dumps({"email": email, "name": name}),
            "list_headers_json": json.dumps([list(h) for h in headers])}


def kept(msg):
    return keep(msg, CTX)[0]


LIST = [("list-id", "<x.lists.example.com>"), ("list-unsubscribe", "<mailto:u@x>")]


def test_oqts_shared_never_leaves_the_laptop():
    assert kept(m("someone@gmail.com", "A Person", account="oqts-shared")) is None


def test_what_you_sent_and_replied_to_is_kept():
    assert kept(m("newc7370@ox.ac.uk"))
    assert kept(m("noreply@x.com", account="oxford", thread="77", headers=LIST))


def test_an_offer_from_an_hr_system_is_kept():  # test question 44
    assert kept(m("brevanhoward@myworkday.com", headers=LIST, subject="Your offer"))
    assert kept(m("no-reply@careers.mlp.com", headers=LIST))


def test_every_oxford_list_is_kept_but_the_daily_job_digest_is_not():
    assert kept(m("student.administration@eng.ox.ac.uk", "EngSci Student Administration", headers=LIST))
    assert kept(m("someone@new.ox.ac.uk", "New College Choir", headers=LIST))
    assert kept(m("x@gmail.com", "Society", headers=[("list-id", "<choir.lists.ox.ac.uk>")]))
    assert kept(m("careerconnect@careers.ox.ac.uk", subject="Daily Opportunities Alert from the Careers Service")) is False


def test_travel_bookings_are_kept_and_shop_receipts_are_not():
    assert kept(m("BA.e-ticket@email.ba.com", "British Airways e-ticket", subject="Your e-ticket receipt VWGSYU"))
    assert kept(m("bookings@info.easyjet.com", subject="Alec, booking ref K3Z814B"))
    assert not kept(m("customerservices@messages.ba.com", subject="Essential information about power banks"))
    assert not kept(m("auto-confirm@amazon.co.uk", "Amazon", subject="Your order", headers=LIST))


def test_quant_societies_are_kept_but_not_finance_newsletters():
    assert kept(m("oxfordalphafund@gmail.com", "Oxford Alpha Fund", headers=LIST))
    assert kept(m("oqts@oqts.org", "Oxford Quantitative Trading Society", headers=LIST))
    assert not kept(m("FT@newsletters.ft.com", "FT Unhedged", headers=LIST))
    assert not kept(m("notifications@github.com", "vercel[bot]",
                      headers=[("list-id", "<quant-trading.repo.github.com>")]))


def test_lists_and_brands_are_left_out_and_people_are_kept():
    assert not kept(m("no-reply@spotify.com", "Spotify"))
    assert not kept(m("x@deliveroo.co.uk", "Deliveroo"))
    assert not kept(m("msfamilysafety@microsoft.com", "Microsoft Family Safety"))
    assert kept(m("edward.weeden@gym.co.uk", "Edward Weeden"))
    assert kept(m("friend@example.org", "Whoever"))
    assert kept(m("callum.mitchellthomson@gmail.com", "Callum Mitchell-Thomson"))


def test_build_never_writes_oqts_shared_and_records_what_it_left_out(tmp_path):
    db = tmp_path / "keryx.db"
    c = sqlite3.connect(db)
    c.executescript("""
      CREATE TABLE messages (id INTEGER, account TEXT, thread_id TEXT, message_id TEXT, from_json TEXT,
        to_json TEXT, cc_json TEXT, subject TEXT, date INTEGER, in_inbox INTEGER, read INTEGER,
        flagged INTEGER, list_headers_json TEXT, body_text TEXT, attachments_json TEXT);
      CREATE TABLE tags (account TEXT, thread_id TEXT, kind TEXT, reason TEXT);
      CREATE TABLE links (account TEXT, thread_id TEXT, app TEXT, kind TEXT, target TEXT, title TEXT);
    """)
    rows = [
        (1, "personal", "t1", "<a>", '{"email":"edward.weeden@gym.co.uk","name":"Edward Weeden"}', "[]", "[]",
         "Gym", 1700000000, 1, 1, 0, "[]", "password: hunter22secret", "[]"),
        (2, "oqts-shared", "t2", "<b>", '{"email":"x@gmail.com","name":"A Person"}', "[]", "[]",
         "secret", 1700000000, 1, 1, 0, "[]", "never", "[]"),
        (3, "personal", "t3", "<c>", '{"email":"no-reply@spotify.com","name":"Spotify"}', "[]", "[]",
         "Promo", 1700000000, 1, 1, 0, "[]", "buy", "[]"),
    ]
    c.executemany("INSERT INTO messages VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", rows)
    c.execute("INSERT INTO links VALUES ('personal','t1','locus','item','millennium','Millennium')")
    c.commit()
    c.close()
    stage = tmp_path / "stage"
    r = build(db, stage)
    assert (r.kept, r.left_out, r.excluded) == (1, 1, 1)
    files = list(stage.glob("*/*.json"))
    assert [f.parent.name for f in files] == ["personal"]
    doc = json.loads(files[0].read_text())
    assert "hunter22secret" not in doc["messages"][0]["body"]          # scrubbed before it leaves
    assert json.loads((stage / "_rule.json").read_text())["left_out_samples"]
    it = list(items(stage))
    assert it[0].thing == "millennium" and it[0].area == "Careers"
    assert ("edward.weeden@gym.co.uk", "from") in it[0].people
    assert build(db, stage).threads_written == 0                      # unchanged: nothing rewritten


def test_clean_removes_the_quoted_thread_and_signature():
    body = ("Yes, Thursday works.\n\nBest,\nAlec\n-- \nAlec Mitchell-Thomson\nNew College\n\n"
            "On Mon, 3 Nov 2025 at 10:00, Sam Smith <sam@x.com> wrote:\n> Can we meet Thursday?\n")
    assert clean(body) == "Yes, Thursday works.\n\nBest,\nAlec"


def test_clean_outlook_header_block():
    body = "Thanks, done.\n\nFrom: Olivia Vickers <o@mlp.com>\nSent: 27 October 2026 10:00\nTo: Alec\nSubject: X\n\nold text"
    assert clean(body) == "Thanks, done."


def test_a_forward_under_a_short_note_keeps_the_forwarded_mail_unquoted():
    body = ("Kind Regards,\nCallum\n\nBegin forwarded message:\n\n> From: Morten\n> Subject: Verbier\n>\n"
            "> I have booked the following flights: LX 357\n")
    out = clean(body)
    assert "I have booked the following flights: LX 357" in out and ">" not in out


def test_a_body_that_is_all_quote_is_not_emptied():
    body = "On Mon, Sam wrote:\n> only quoted text here"
    assert clean(body)
