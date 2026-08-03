"""The daily reMarkable page — composition, pagination, the no-repeat ledger, anchors, rendering.

Model-free: the composer is aggregate-only by design, so a seeded tmp DB exercises all of it.
What is asserted is mostly the guardrails, because those outrank any feature on this surface:
one section per physical page, empty-is-valid, no guilt metrics (with the one deliberate reading
exception), nothing shown twice, and — the one the pull-back depends on — that every writable
region carries a stable anchor recorded in `daily_anchors`.
"""

from __future__ import annotations

from datetime import date, datetime, timedelta, timezone
from pathlib import Path

import pytest

from locus.agent import compose_daily as cd
from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.learn import review as learn_review


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "daily.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _proposed(conn, title: str, *, created_at: str, why: str = "because") -> int:
    oid, _ = state.upsert_object(
        conn, type_="concept", title=title, body={"why": why}, now=lambda: created_at
    )
    state.add_links(
        conn, oid, [state.ObjectLink("entity", state.entity_key(title, "concept"), "about")]
    )
    return oid


def _thread(conn, title: str, *, type_="question", created_at="2026-01-01T00:00:00+00:00") -> int:
    oid, _ = state.upsert_object(conn, type_=type_, title=title, now=lambda: created_at)
    state.set_status(conn, oid, "active")
    return oid


def _mark(conn, *, uri="books/apm.pdf", page=70, text, kind="underline", at="2026-07-30T10:00:00"):
    with conn:
        conn.execute(
            "INSERT INTO pdf_annotations (source_uri, pdf_page, kind, bbox_key, covered_text, "
            "in_margin, captured_at) VALUES (?,?,?,?,?,0,?)",
            (uri, page, kind, f"k{page}-{len(text)}", text, at),
        )


def _proposal(conn, title: str, *, status="proposed", why="cited by 2 papers you kept",
              why_long="", written="", proposed_at=None, folder=None, score=1.0) -> int:
    with conn:
        cur = conn.execute(
            "INSERT INTO reading_proposals (kind, dedupe_key, title, why, why_kind, "
            "evidence_key, status, score, proposed_at, created_at, why_long, why_written_at, "
            "device_folder) VALUES ('paper',?,?,?,'citation','papers/x.pdf',?,?,?,?,?,?,?)",
            (f"key-{title}", title, why, status, score, proposed_at, "2026-07-01",
             why_long or None, written or None, folder),
        )
    return cur.lastrowid


_PASSAGE = "the extreme case in which every hedge fund holds a copy of the same portfolio"


def _marks_on(page):
    return [t for t in page.threads if t.kind == "mark"]


# ---------- migration ----------


def test_0023_columns_and_ledger_exist(conn):
    cols = {r["name"] for r in conn.execute("PRAGMA table_info(daily_pages)")}
    assert "read_at" in cols
    cols = {r["name"] for r in conn.execute("PRAGMA table_info(reading_proposals)")}
    assert {"why_long", "why_written_at"} <= cols
    cols = {r["name"] for r in conn.execute("PRAGMA table_info(review_schedule)")}
    assert "question" in cols
    conn.execute("SELECT item_key, kind, page_date FROM daily_shown")


def test_annotations_unique_by_date_and_anchor(conn):
    import sqlite3

    conn.execute("INSERT INTO annotations (page_date, anchor, text, captured_at) "
                 "VALUES ('2026-06-01','R1','x','t')")
    conn.commit()
    with pytest.raises(sqlite3.IntegrityError):
        conn.execute("INSERT INTO annotations (page_date, anchor, text, captured_at) "
                     "VALUES ('2026-06-01','R1','y','t')")


# ---------- guardrails ----------


def test_empty_is_a_valid_calm_state(conn):
    page = cd.compose(conn)
    assert page.is_empty
    body = cd.render(page)
    assert "Nothing to surface today." in body
    # No empty section headings, and above all no counts to feel behind on.
    for banned in ("## Read", "## Think", "## Recall"):
        assert banned not in body
    # ...but the status line survives an empty day: it is the only place a failure is announced.
    assert "overnight:" in body


def test_the_page_no_longer_offers_blessings(conn):
    """Approvals moved to `locus decide` (plan §3), and no decision may live on two surfaces.

    This is the invariant, asserted where it can regress: a proposed object must produce no
    region and no anchor here, however many are pending.
    """
    for i in range(12):
        _proposed(conn, f"concept {i}", created_at=f"2026-07-{i + 1:02d}T00:00:00+00:00")
    page = cd.compose(conn)
    assert not [a for a in page.anchors if a.kind == "blessing"]
    body = cd.render(page)
    for phrase in ("Awaiting your call", "bless", "pending", "remaining", "unread", "streak"):
        assert phrase not in body.lower()


def test_recalls_fit_the_page(conn):
    for i in range(9):
        learn_review.schedule_prompt(
            conn, prompt_kind="object", prompt_ref=str(i), today=date(2026, 1, 1)
        )
    page = cd.compose(conn, today=date(2026, 6, 1))
    assert len(page.recalls) == cd._FIT["recall"]


# ---------- page 1: Read ----------


def test_read_page_comes_from_the_discovery_shelf(conn):
    """The section this replaces offered corpus re-reads — an Optibook manual from last year —
    while ten real papers sat in Reading/Proposed and compose_daily never referenced them."""
    _proposal(conn, "Ledoit-Wolf shrinkage", why_long="regime-ml estimates 55x55 from 250 days.")
    page = cd.compose(conn, today=date(2026, 8, 2))
    assert [r.title for r in page.readings] == ["Ledoit-Wolf shrinkage"]
    assert "regime-ml estimates 55x55" in cd.render(page)


def test_the_written_reason_is_used_when_there_is_one_and_falls_back_when_not(conn):
    _proposal(conn, "with prose", why="deterministic grounding", why_long="what you could do")
    _proposal(conn, "without prose", why="deterministic grounding", score=0.5)
    by_title = {r.title: r for r in cd.build_readings(conn)}
    assert by_title["with prose"].why == "what you could do"
    assert by_title["without prose"].why == "deterministic grounding"


def test_only_proposed_papers_are_offered_to_read(conn):
    _proposal(conn, "already accepted", status="accepted", folder="In-Progress")
    assert cd.build_readings(conn) == []


def _target(conn, *, title, subject="regime-ml", why=None, proposal_id=None, marks=0,
            folder="In-Progress"):
    with conn:
        cur = conn.execute(
            "INSERT INTO reading_targets (doc_uuid, device_path, source_uri, proposal_id, "
            "linked_by, created_at, device_folder, marks, title, subject_kind, subject_label, "
            "fit, why_long) VALUES (?,?,?,?,'manual','2026-08-01',?,?,?,'project',?,0.7,?)",
            (f"u-{title}", f"/Reading/{folder}/{title}", f"raw/{title}.pdf", proposal_id,
             folder, marks, title, subject, why),
        )
    return cur.lastrowid


def test_the_shelf_state_prints_the_true_count(conn):
    """The one deliberate exception to no-guilt-metrics: 'full queue, and tell me the truth'.

    The count is the brake — if `oldest` keeps growing while nothing moves, that is the signal,
    and it only works if the number is printed rather than hidden behind the principle.
    """
    now = datetime(2026, 8, 2, tzinfo=timezone.utc)
    _proposal(conn, "old one", proposed_at=(now - timedelta(days=21)).isoformat())
    _proposal(conn, "new one", proposed_at=(now - timedelta(days=1)).isoformat())
    _target(conn, title="being read")

    st = cd.build_reading_state(conn, now=now)
    assert st.proposed == 2
    assert st.oldest_days == 21
    assert [i.title for i in st.in_progress] == ["being read"]

    body = cd.render(cd.compose(conn, today=date(2026, 8, 2)))
    assert "2 waiting" in body
    assert "being read" in body


def test_a_book_he_added_himself_appears_with_what_it_links_to(conn):
    """His request, and it closes a real asymmetry: a proposed paper arrived with a project link
    and a written reason, while the book he was actually reading and annotating had neither — and
    was the one document the section left out entirely."""
    _target(conn, title="Advanced Portfolio Management", subject="regime-ml", marks=26,
            why="Your factor covariance estimator is the thing this chapter attacks.")
    st = cd.build_reading_state(conn)

    item = st.in_progress[0]
    assert item.owner_added, "nothing in the pipeline chose it — that is what makes it interesting"
    assert item.links_to == "regime-ml"

    body = cd.render(cd.compose(conn, today=date(2026, 8, 2)))
    assert "Advanced Portfolio Management" in body
    assert "regime-ml" in body
    assert "factor covariance estimator" in body
    assert "*yours*" in body


def test_a_delivered_paper_in_progress_is_not_marked_as_his_own(conn):
    _proposal(conn, "a delivered paper")
    _target(conn, title="a delivered paper", proposal_id=1)
    assert not cd.build_reading_state(conn).in_progress[0].owner_added


def test_reading_degrades_silently_without_the_discovery_tables(conn):
    conn.execute("DROP TABLE reading_proposals")
    conn.commit()
    assert cd.build_readings(conn) == []
    assert cd.build_reading_state(conn).is_empty


# ---------- page 2: Think ----------


def test_the_three_subsections_are_named_for_where_the_item_came_from(conn):
    """"I do not want all these different sections that are all very similar and confusingly
    labelled" — one page, one action, but provenance stated."""
    _mark(conn, text=_PASSAGE)
    _thread(conn, "is a factor like a feature?")
    page = cd.compose(conn, today=date(2026, 7, 31))
    body = cd.render(page)
    assert cd.SECTION_MARKED in body
    assert cd.SECTION_OPEN in body
    # An empty subsection is omitted entirely rather than left as a bare heading.
    assert cd.SECTION_CONNECTION not in body


def test_marks_threads_and_connections_share_one_anchor_series(conn):
    _mark(conn, text=_PASSAGE)
    _thread(conn, "a question of mine")
    page = cd.compose(conn, today=date(2026, 7, 31))
    assert [t.anchor for t in page.threads] == [f"T{i}" for i in range(1, len(page.threads) + 1)]
    # ...while still routing to different places on the way back.
    assert {a.kind for a in page.anchors if a.anchor.startswith("T")} == {"mark", "open"}


def test_only_the_agent_originated_item_carries_a_tick(conn):
    """A tick must mean exactly one thing on the whole page."""
    _mark(conn, text=_PASSAGE)
    _thread(conn, "mine")
    page = cd.compose(conn, today=date(2026, 7, 31))
    assert all(not t.tick for t in page.threads if t.kind in ("mark", "open"))


def test_marked_passages_are_surfaced(conn):
    """Loop B stored 26 marks with NOTHING reading them — capture was write-only."""
    _mark(conn, text=_PASSAGE)
    marks = cd.build_marked(conn)
    assert len(marks) == 1
    assert marks[0].headline == _PASSAGE
    assert "p.71" in marks[0].context, "pages are printed 1-based for a human"


def test_a_mark_already_turned_into_something_is_not_re_offered(conn):
    """Once a passage has become an idea it has moved on; re-offering it is an unread count."""
    _mark(conn, text=_PASSAGE)
    conn.execute("UPDATE pdf_annotations SET object_id=1")
    conn.commit()
    assert cd.build_marked(conn) == []


def test_transcribing_the_ink_does_not_hide_the_mark(conn):
    """`note` is the transcription, NOT a has-been-dealt-with flag.

    The first cut filtered on `note IS NULL`, so reading his handwriting HID the mark — exactly
    backwards, since a passage he wrote a paragraph about is the most worth returning.
    """
    _mark(conn, text=_PASSAGE)
    conn.execute("UPDATE pdf_annotations SET note='is this any good?'")
    conn.commit()
    marks = cd.build_marked(conn)
    assert len(marks) == 1
    assert marks[0].body == ("you wrote: is this any good?",)


def test_his_own_words_rank_above_a_bare_underline(conn):
    _mark(conn, uri="books/a.pdf", page=1, text=_PASSAGE + " bare", at="2026-07-31T10:00:00")
    _mark(conn, uri="books/b.pdf", page=2, text=_PASSAGE + " annotated", at="2026-07-30T10:00:00")
    conn.execute("UPDATE pdf_annotations SET note='my objection' WHERE source_uri='books/b.pdf'")
    conn.commit()
    assert cd.build_marked(conn, limit=1)[0].target_key == "books/b.pdf"


def test_his_comment_is_printed_with_the_passage(conn):
    _mark(conn, text=_PASSAGE)
    conn.execute("UPDATE pdf_annotations SET note='no momentum in Japan??'")
    conn.commit()
    body = cd.render(cd.compose(conn, today=date(2026, 7, 31)))
    assert "no momentum in Japan??" in body
    assert _PASSAGE in body, "the objection and the claim must travel together"


def test_a_stray_stroke_is_not_a_passage(conn):
    """Below a few words a mark means nothing when it comes back a week later."""
    _mark(conn, text="the")
    assert cd.build_marked(conn) == []


def test_one_passage_per_document(conn):
    """Two quotes from the same chapter are one thought; a book must not own the section."""
    _mark(conn, page=70, text=_PASSAGE)
    _mark(conn, page=71, text=_PASSAGE + " and again on the next page entirely")
    _mark(conn, uri="books/other.pdf", page=5, text=_PASSAGE + " but in a different book")
    assert {m.target_key for m in cd.build_marked(conn, limit=4)} == {
        "books/apm.pdf", "books/other.pdf"
    }


def test_marks_degrade_silently_when_loop_b_has_never_run(conn):
    conn.execute("DROP TABLE pdf_annotations")
    conn.commit()
    assert cd.build_marked(conn) == []


# ---------- the no-repeat ledger ----------


def test_nothing_is_offered_on_two_pages(conn):
    """"I dont want to see the same thing twice on different daily pages, even if I missed one."

    A page is now built every morning whether or not the last was read, so without the ledger a
    skipped day would re-offer its whole contents.
    """
    _mark(conn, text=_PASSAGE)
    _proposal(conn, "Ledoit-Wolf shrinkage")
    first = cd.compose(conn, today=date(2026, 8, 1))
    cd.persist(conn, first, md_path="/tmp/_home.md")

    second = cd.compose(conn, today=date(2026, 8, 2))
    assert not _marks_on(second)
    assert second.readings == []
    # ...and nothing was lost: it is still on the first page, which is sitting in the inbox.
    assert _marks_on(first)


def test_a_thread_he_developed_becomes_eligible_again_and_an_untouched_one_does_not(conn):
    """The key carries the item's VERSION, which is what makes one rule right in both
    directions — developing a thread is exactly when it is worth showing again.

    Eligibility is what the ledger decides; WHEN it actually reappears is the separate
    least-recently-touched ordering, which deliberately sends a thread he just wrote on to the
    back of the queue rather than straight back at him.
    """
    thread = _thread(conn, "factors vs features")
    cd.persist(conn, cd.compose(conn, today=date(2026, 8, 1)), md_path="/tmp/_home.md")

    # Shown once => blocked, however many pages are built afterwards.
    assert cd.build_open(conn, limit=5, seen=cd._shown_keys(conn)) == []
    cd.persist(conn, cd.compose(conn, today=date(2026, 8, 2)), md_path="/tmp/_home.md")
    assert cd.build_open(conn, limit=5, seen=cd._shown_keys(conn)) == []

    # ...until he writes on it, which changes its version and makes it worth offering again.
    state.apply_owner_edit(conn, thread, {"development": [{"at": "x", "text": "new"}]},
                           source="daily:2026-08-02#T1")
    eligible = cd.build_open(conn, limit=5, seen=cd._shown_keys(conn))
    assert [t.target_key for t in eligible] == [str(thread)]
    assert eligible[0].body == ("new",), "it comes back carrying what he last said"


def test_a_rescheduled_recall_is_offered_again(conn):
    """Blocking it would have quietly disabled spaced repetition — a new `due` is a new offering."""
    item = learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )
    cd.persist(conn, cd.compose(conn, today=date(2026, 8, 1)), md_path="/tmp/_home.md")
    assert cd.compose(conn, today=date(2026, 8, 2)).recalls == []

    learn_review.grade_item(conn, item.id, 5, today=date(2026, 8, 2))
    later = cd.compose(conn, today=date(2027, 1, 1))
    assert [r.item_id for r in later.recalls] == [item.id]


def test_a_proposal_returns_only_when_its_reason_was_rewritten(conn):
    """The narrow Read-page exception: a repeat must always carry new text."""
    pid = _proposal(conn, "Ledoit-Wolf", why_long="first reason", written="2026-08-01")
    cd.persist(conn, cd.compose(conn, today=date(2026, 8, 1)), md_path="/tmp/_home.md")
    assert cd.compose(conn, today=date(2026, 8, 2)).readings == []

    conn.execute(
        "UPDATE reading_proposals SET why_long=?, why_written_at=? WHERE id=?",
        ("rewritten against what he is thinking about now", "2026-08-08", pid),
    )
    conn.commit()
    again = cd.compose(conn, today=date(2026, 8, 8))
    assert [r.why for r in again.readings] == ["rewritten against what he is thinking about now"]


def test_composing_twice_without_persisting_changes_nothing(conn):
    """`compose` is a pure read — the ledger is written by `persist`, so a dry run is free."""
    _mark(conn, text=_PASSAGE)
    assert _marks_on(cd.compose(conn, today=date(2026, 8, 1)))
    assert _marks_on(cd.compose(conn, today=date(2026, 8, 1)))


def test_the_ledger_degrades_when_absent(conn):
    conn.execute("DROP TABLE daily_shown")
    conn.commit()
    _mark(conn, text=_PASSAGE)
    assert _marks_on(cd.compose(conn, today=date(2026, 8, 1)))


# ---------- the /Daily inbox ----------


def test_read_at_is_set_once_and_never_overwritten(conn):
    """It moves the page out of the inbox, so it must mean 'he wrote on this' and nothing looser.

    Set twice would make a page he wrote on last week look like today's reading."""
    page = cd.compose(conn, today=date(2026, 8, 1))
    cd.persist(conn, page, md_path="/tmp/_home.md")
    assert cd.mark_read(conn, page.page_date, at="2026-08-01T09:00:00+00:00")
    assert not cd.mark_read(conn, page.page_date, at="2026-08-05T09:00:00+00:00")
    row = conn.execute(
        "SELECT read_at FROM daily_pages WHERE page_date=?", (page.page_date,)
    ).fetchone()
    assert row["read_at"] == "2026-08-01T09:00:00+00:00"


# ---------- the status line ----------


def test_status_reports_what_ran_and_shouts_about_what_broke(conn):
    """`locus-maintain` failed six consecutive nights and nothing said so (2026-08-01)."""
    from locus.agent import journal

    now = datetime.now(timezone.utc)
    ok = journal.start_run(conn, "discover-pull")
    journal.finish_run(conn, ok, "ok")
    broken = journal.start_run(conn, "maintain")
    journal.finish_run(conn, broken, "error", stats={"error": "boom"})

    st = cd.build_status(conn, now=now + timedelta(minutes=1))
    assert "discover-pull" in " ".join(st.produced)
    assert any("maintain" in f for f in st.failures)
    assert "MAINTAIN ERROR" in st.render()


def test_a_run_that_never_finished_is_a_failure_not_a_silence(conn):
    """...but only once it is older than any plausible run. Flagging a run that started a minute
    ago would put a false alarm on the page every morning, which trains him to skip the line."""
    from locus.agent import journal

    journal.start_run(conn, "maintain")  # opened, never closed: the process died
    now = datetime.now(timezone.utc)
    assert not any(
        "never finished" in f for f in cd.build_status(conn, now=now + timedelta(minutes=1)).failures
    ), "a run in progress is not a fault"
    later = cd.build_status(conn, now=now + timedelta(hours=3))
    assert any("never finished" in f for f in later.failures)


# ---------- anchors ----------


def test_every_writable_region_has_an_anchor_and_it_is_rendered(conn):
    _mark(conn, text=_PASSAGE)
    _thread(conn, "alpha")
    _proposal(conn, "a paper")
    learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )

    page = cd.compose(conn, today=date(2026, 8, 1))
    body = cd.render(page)

    assert page.anchors, "a page with content must carry anchors"
    for a in page.anchors:
        assert a.anchor in body, f"anchor {a.anchor} not printed on the page"
    labels = [a.anchor for a in page.anchors]
    assert len(labels) == len(set(labels)), "the pull-back keys on them"


def test_persist_records_anchors_and_a_rebuild_replaces_them(conn):
    _thread(conn, "alpha", created_at="2026-01-02T00:00:00+00:00")
    page = cd.compose(conn, today=date(2026, 6, 1))
    cd.persist(conn, page, md_path="/tmp/_home.md")

    stored = cd.anchors_for(conn, page.page_date)
    assert {a.anchor for a in page.anchors} == set(stored)
    assert stored["T1"].target_kind == "object"
    assert stored["T1"].label == "alpha"
    assert len(conn.execute("SELECT * FROM daily_pages").fetchall()) == 1


def test_persist_leaves_annotations_alone(conn):
    """Evidence the owner produced outlives a rebuild of the page that prompted it."""
    _thread(conn, "alpha")
    page = cd.compose(conn, today=date(2026, 6, 1))
    cd.persist(conn, page, md_path="/tmp/_home.md")
    conn.execute(
        "INSERT INTO annotations (page_date, anchor, text, captured_at) VALUES (?,?,?,?)",
        (page.page_date, "T1", "owner wrote this", "t"),
    )
    conn.commit()

    cd.persist(conn, cd.compose(conn, today=date(2026, 6, 1)), md_path="/tmp/_home.md")
    row = conn.execute("SELECT text FROM annotations WHERE page_date=? AND anchor='T1'",
                       (page.page_date,)).fetchone()
    assert row["text"] == "owner wrote this"


# ---------- rendering ----------


def test_each_section_gets_its_own_physical_page(conn):
    """"one full page for each, each page should be well laid out and look nice"."""
    _proposal(conn, "a paper")
    _mark(conn, text=_PASSAGE)
    learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )
    body = cd.render(cd.compose(conn, today=date(2026, 8, 1)))
    # Read | Think | Recall | back page => three breaks.
    assert body.count("#pagebreak()") == 3
    assert body.index("## Read") < body.index("## Think") < body.index("## Recall")


def test_a_quiet_day_is_a_short_document_not_four_blank_pages(conn):
    _mark(conn, text=_PASSAGE)
    body = cd.render(cd.compose(conn, today=date(2026, 8, 1)))
    assert body.count("#pagebreak()") == 1  # Think, then the back page
    assert "## Recall" not in body


def test_writable_rows_render_ascii_boxes_and_ruled_lines(conn):
    _mark(conn, text=_PASSAGE)
    body = cd.render(cd.compose(conn, today=date(2026, 8, 1)))
    assert "***" in body, "ruled writing lines must use *** (--- is eaten by pandoc)"
    assert " " not in body, "a non-breaking space stops the next line parsing as a rule"


def test_a_connection_tick_box_is_ascii(conn):
    """The ballot-box glyph has no font coverage and rendered as a tofu box (2026-07-30)."""
    item = cd.ThreadItem(
        anchor="T1", section=cd.SECTION_CONNECTION, kind="connection",
        headline="a → b", context="both develop x", tick=True,
    )
    page = cd.DailyPage(page_date="2026-08-01", threads=[item])
    assert "[   ]" in cd.render(page)


def test_rendered_page_has_no_nbsp_anywhere(conn):
    """U+00A0 is not markdown whitespace: it silently changes how the next line parses."""
    _proposal(conn, "a paper")
    _mark(conn, text=_PASSAGE)
    learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )
    assert " " not in cd.render(cd.compose(conn, today=date(2026, 8, 1)))


def test_the_entity_key_separator_never_reaches_a_rendered_surface(conn):
    """U+001F is invisible, so a raw key reads as one run-on word ('matching engineconcept').

    The page no longer prints entity grounding (that left with the blessing section, and returns
    in `locus decide`), so this guards the decoder itself rather than the renderer.
    """
    key = state.entity_key("matching engine", "concept")
    assert "\x1f" in key
    assert state.parse_entity_key(key) == ("matching engine", "concept")


# ---------- recall: ask on page 3, answer on page 4 ----------


def _proposition(conn, text: str) -> int:
    with conn:
        conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, "
            "ingest_model, title) VALUES ('h','pdf','papers/x.pdf','raw/x.pdf','test','A paper')"
        )
        conn.execute(
            "INSERT INTO sections (doc_id, position, title) VALUES (1, 0, 'Intro')"
        )
        cur = conn.execute(
            "INSERT INTO propositions (doc_id, section_id, position, text, embed_model) "
            "VALUES (1,1,0,?,'nomic')", (text,)
        )
    return cur.lastrowid


def test_the_question_is_asked_on_the_recall_page_and_answered_overleaf(conn):
    pid = _proposition(conn, "A factor covariance estimator decomposes r into Bf + e.")
    item = learn_review.schedule_prompt(
        conn, prompt_kind="proposition", prompt_ref=str(pid), today=date(2026, 1, 1)
    )
    learn_review.set_question(conn, item.id, "What does a factor covariance estimator decompose?")

    page = cd.compose(conn, today=date(2026, 8, 1))
    assert page.recalls[0].prompt == "What does a factor covariance estimator decompose?"
    assert page.recalls[0].answer.startswith("A factor covariance estimator decomposes")

    body = cd.render(page)
    recall_page, back_page = body.split("#pagebreak()")[-2:]
    assert "What does a factor covariance estimator decompose?" in recall_page
    assert "Bf + e" not in recall_page, "seeing the answer first makes the attempt worthless"
    assert "Bf + e" in back_page


def test_without_a_stored_question_no_answer_is_printed(conn):
    """An 'answer' identical to the prompt is worse than none — which is what it used to be."""
    pid = _proposition(conn, "A factor covariance estimator decomposes r into Bf + e.")
    learn_review.schedule_prompt(
        conn, prompt_kind="proposition", prompt_ref=str(pid), today=date(2026, 1, 1)
    )
    page = cd.compose(conn, today=date(2026, 8, 1))
    assert page.recalls[0].prompt.startswith("A factor covariance estimator decomposes")
    assert page.recalls[0].answer == ""
    assert "*Answers*" not in cd.render(page)


# ---------- pagination: the layout constraint the whole design rests on ----------


def test_writing_space_expands_when_there_is_less_to_show(conn):
    """A fixed three lines per item left a two-item page 60% white. On a surface meant to be
    written on, empty space is not restraint — it is wasted paper."""
    assert cd._lines_for(3, "think") == cd._MIN_LINES["think"]
    assert cd._lines_for(1, "think") > cd._MIN_LINES["think"]
    assert cd._lines_for(1, "think") == cd._MAX_LINES
    # Recall regions floor lower on purpose: recalling a stored claim is a sentence or two, so
    # two lines there buys a fourth question rather than white space.
    assert cd._lines_for(4, "recall") == cd._MIN_LINES["recall"] == 2


def test_a_full_page_does_not_overflow_into_a_fifth(conn):
    """The regression this pins: raising the line budget silently pushed Think onto two pages,
    which breaks one-section-per-page — the constraint the whole layout is built on.

    Asserted against the REAL renderer, because the failure is typographic and invisible to any
    assertion on the markdown.
    """
    pytest.importorskip("pypandoc", reason="the [reading] extra renders the PDF")
    pytest.importorskip("typst")
    pymupdf = pytest.importorskip("pymupdf")
    from locus.reading.md2pdf import PageGeometry, render_markdown_to_pdf

    # THE FIXTURE MUST BE THE WORST REALISTIC PAGE, not a comfortable one. The first version of
    # this test seeded short reasons and no in-progress list, so it passed at every cap while the
    # REAL page rendered five pages (measured 2026-08-02). A layout test that does not carry
    # live-sized content is only testing the renderer.
    for i in range(cd._FIT["read"]):
        _proposal(
            conn, f"Portfolio Optimization and Tail-Risk Analytics of Actively Managed ETFs {i}",
            # 300 chars is what `_clip` allows a written reason, so it is what the page must hold.
            why_long=("Alpha Fund's backtesting uses Mean-Variance Optimization; this paper "
                      "compares MVO, CVaR minimization and tangency strategies for long-short "
                      "portfolios with tail-risk diagnostics. Apply these to evaluate whether "
                      "CVaR-optimization better handles tail-risk in your cascade portfolios.")[:300],
        )
    for i in range(5):
        _target(conn, title=f"AlphaZeroBeta Deep Reinforcement Learning for Market-Neutral {i}",
                marks=i,
                why="Stop-loss cascade detection in Alpha Fund directly models synchronized "
                    "deleveraging during adverse events across correlated books.")
    for i in range(4):
        _mark(conn, uri=f"books/b{i}.pdf", page=i, text=f"{_PASSAGE} number {i}")
    for i in range(cd._FIT["recall"]):
        learn_review.schedule_prompt(
            conn, prompt_kind="object", prompt_ref=str(i), today=date(2026, 1, 1)
        )

    page = cd.compose(conn, today=date(2026, 8, 1))
    out = Path(str(conn.execute("PRAGMA database_list").fetchone()["file"])).parent / "p.pdf"
    render_markdown_to_pdf(
        cd.render(page), out, geometry=PageGeometry(rule_gap_em=2.6)
    )
    assert pymupdf.open(out).page_count == 4, "a section overflowed onto a second page"
