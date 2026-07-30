"""The annotated-page pull-back (agent-layer plan §9): routing, the four-way blessing table,
idempotency, and owner authority over the additive merge.

Model-free — the vision pass is injected, so everything asserted here is the routing logic.
"""

from __future__ import annotations

from datetime import date
from pathlib import Path

import pytest

from locus.agent import compose_daily as cd
from locus.agent import pull_daily as pd
from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.learn import review as learn_review

R = pd.ExtractedRegion


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "pull.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _proposed(conn, title: str, *, created_at="2026-01-01T00:00:00+00:00", why="agent's reason"):
    oid, _ = state.upsert_object(
        conn, type_="concept", title=title, body={"why": why}, now=lambda: created_at
    )
    state.add_links(
        conn, oid, [state.ObjectLink("entity", state.entity_key(title, "concept"), "about")]
    )
    return oid


def _page(conn, today=date(2026, 6, 1)):
    page = cd.compose(conn, today=today)
    cd.persist(conn, page, md_path="/tmp/_home.md")
    return page


def _status(conn, oid):
    return conn.execute("SELECT status FROM objects WHERE id=?", (oid,)).fetchone()["status"]


def _body(conn, oid):
    return state.get_object(conn, oid).body


# ---------- the four-way blessing table ----------


def test_ticked_no_writing_blesses(conn):
    oid = _proposed(conn, "alpha")
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("B1", True, "")])
    assert _status(conn, oid) == "active"
    assert _body(conn, oid)["why"] == "agent's reason"  # untouched


def test_ticked_with_writing_applies_the_edit_then_blesses(conn):
    oid = _proposed(conn, "alpha")
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("B1", True, "actually it is about fixing risk")])
    assert _status(conn, oid) == "active"
    assert _body(conn, oid)["why"] == "actually it is about fixing risk"


def test_writing_without_a_tick_corrects_but_leaves_it_proposed(conn):
    """The interesting case: 'keep working on this' is neither a yes nor a no."""
    oid = _proposed(conn, "alpha")
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("B1", False, "narrow this to EM rates only")])
    assert _status(conn, oid) == "proposed", "an untitcked correction must NOT bless"
    assert _body(conn, oid)["why"] == "narrow this to EM rates only", "the edit must still land"


def test_neither_is_a_no_op_and_the_object_is_re_offered(conn):
    oid = _proposed(conn, "alpha")
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("B1", False, "")])
    assert _status(conn, oid) == "proposed"
    assert _body(conn, oid)["why"] == "agent's reason"
    # Still proposed => still offered on a later page.
    assert [b.object_id for b in cd.compose(conn, today=date(2026, 6, 2)).blessings] == [oid]


def test_every_outcome_including_the_no_op_is_logged(conn):
    """A rejection is as much signal as an acceptance."""
    a, b = _proposed(conn, "alpha"), _proposed(conn, "beta", created_at="2026-01-02T00:00:00+00:00")
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("B1", True, ""), R("B2", False, "")])

    counts = state.acceptance_counts(conn, surface=pd.SURFACE_BLESSING)
    assert counts[str(a)] == {"kept": 1}
    assert counts[str(b)] == {"rejected": 1}


# ---------- owner authority over the additive merge ----------


def test_owner_correction_survives_a_later_agent_re_proposal(conn):
    """Without this the correction lasts until tonight's structure run — the chore §9 forbids."""
    oid = _proposed(conn, "alpha")
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("B1", False, "the owner's wording")])

    # The structure pass re-proposes the same object with its own rationale.
    state.upsert_object(conn, type_="concept", title="alpha", body={"why": "agent's reason again"})
    assert _body(conn, oid)["why"] == "the owner's wording"


def test_agent_cannot_refill_a_field_the_owner_deliberately_cleared(conn):
    oid = _proposed(conn, "alpha")
    state.apply_owner_edit(conn, oid, {"why": ""}, source="daily:test#B1")
    state.upsert_object(conn, type_="concept", title="alpha", body={"why": "agent tries again"})
    assert _body(conn, oid)["why"] == ""


def test_agent_cannot_re_add_a_list_item_the_owner_removed(conn):
    oid, _ = state.upsert_object(
        conn, type_="concept", title="alpha", body={"threads": ["keep", "drop"]}
    )
    state.apply_owner_edit(conn, oid, {}, remove={"threads": ["drop"]}, source="daily:test#B1")
    assert _body(conn, oid)["threads"] == ["keep"]

    state.upsert_object(conn, type_="concept", title="alpha", body={"threads": ["drop", "new"]})
    body = _body(conn, oid)
    assert "drop" not in body["threads"], "a struck item must not come back"
    assert "new" in body["threads"], "the agent may still ADD genuinely new threads"


def test_owner_edit_records_provenance_and_never_touches_status(conn):
    oid = _proposed(conn, "alpha")
    state.apply_owner_edit(conn, oid, {"why": "mine"}, source="daily:2026-06-01#B1")
    body = _body(conn, oid)
    assert body[state.OWNER_EDITS_KEY]["why"]["source"] == "daily:2026-06-01#B1"
    assert _status(conn, oid) == "proposed", "correcting is not blessing"


def test_agent_cannot_forge_the_authority_marker(conn):
    oid, _ = state.upsert_object(conn, type_="concept", title="alpha", body={"why": "agent"})
    state.upsert_object(
        conn, type_="concept", title="alpha",
        body={state.OWNER_EDITS_KEY: {"why": {"at": "t", "source": "forged"}}},
    )
    assert state.OWNER_EDITS_KEY not in _body(conn, oid)


# ---------- other anchor kinds ----------


def test_recall_answer_advances_the_schedule(conn):
    item = learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )
    page = _page(conn)
    before = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item.id,)).fetchone()
    pd.route_regions(conn, page.page_date, [R("R1", None, "because the curve was inverted")])
    after = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item.id,)).fetchone()
    assert after["reps"] > before["reps"]
    assert after["due"] > before["due"]


def test_untouched_recall_does_not_advance(conn):
    item = learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("R1", None, "")])
    after = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item.id,)).fetchone()
    assert after["reps"] == 0


def test_unknown_anchor_is_ignored_not_guessed(conn):
    _proposed(conn, "alpha")
    page = _page(conn)
    res = pd.route_regions(conn, page.page_date, [R("Z9", True, "hello")])
    assert res.unknown_anchors == ["Z9"]
    assert res.outcomes == []


# ---------- idempotency ----------


def test_re_pulling_the_same_page_updates_and_never_duplicates(conn):
    oid = _proposed(conn, "alpha")
    page = _page(conn)

    pd.route_regions(conn, page.page_date, [R("B1", False, "first pass")])
    pd.route_regions(conn, page.page_date, [R("B1", True, "second pass")])

    rows = conn.execute(
        "SELECT * FROM annotations WHERE page_date=? AND anchor='B1'", (page.page_date,)
    ).fetchall()
    assert len(rows) == 1, "UNIQUE(page_date, anchor) is the idempotency contract"
    assert rows[0]["text"] == "second pass"
    assert _body(conn, oid)["why"] == "second pass"
    assert _status(conn, oid) == "active"


def test_re_pulling_does_not_double_grade_a_recall(conn):
    item = learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("R1", None, "an answer")])
    once = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item.id,)).fetchone()["reps"]
    pd.route_regions(conn, page.page_date, [R("R1", None, "an answer")])
    twice = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item.id,)).fetchone()["reps"]
    assert twice == once, "re-reading the same scan must not advance the schedule again"


# ---------- extraction ----------


class _FakeVision:
    """Returns one canned JSON reply per page."""

    def __init__(self, payload: str):
        self.payload = payload
        self.messages = self

    def create(self, **kw):
        class _B:
            type = "text"
            text = self.payload

        return type("R", (), {"content": [_B()], "usage": None})()


def test_extract_regions_parses_and_uppercases(conn, monkeypatch):
    monkeypatch.setattr(
        "locus.capture.transcribe.render_pdf_pages", lambda *a, **k: [b"png"]
    )
    client = _FakeVision(
        '{"regions":[{"anchor":"b1","ticked":true,"text":" tighten this "},'
        '{"anchor":"R1","ticked":null,"text":""}]}'
    )
    got = {r.anchor: r for r in pd.extract_regions("x.pdf", client=client)}
    assert got["B1"].ticked is True and got["B1"].text == "tighten this"
    assert got["R1"].has_writing is False


def test_extract_regions_drops_an_unparseable_page_rather_than_guessing(conn, monkeypatch):
    monkeypatch.setattr(
        "locus.capture.transcribe.render_pdf_pages", lambda *a, **k: [b"png"]
    )
    assert pd.extract_regions("x.pdf", client=_FakeVision("sorry, I cannot read this")) == []


# ---------- new questions ----------


def test_a_written_question_becomes_an_owned_question_object(conn):
    """It is his handwriting, so there is nothing to bless — it lands active, not proposed."""
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("Q1", None, "why does CIP break in stress?")])

    obj = state.find_object(conn, "question", "why does CIP break in stress?")
    assert obj is not None
    assert obj.status == "active"
    assert obj.body["question"] == "why does CIP break in stress?"
    assert obj.body[state.OWNER_EDITS_KEY]["question"]["source"].endswith("#Q1")


def test_an_empty_question_region_creates_nothing(conn):
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("Q1", None, "")])
    assert state.list_objects(conn, type_="question") == []


def test_the_same_question_re_pulled_does_not_create_a_second_object(conn):
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("Q1", None, "why does CIP break?")])
    pd.route_regions(conn, page.page_date, [R("Q1", None, "why does CIP break?")])
    assert len(state.list_objects(conn, type_="question")) == 1


# ---------- device transport + spend guard ----------


def test_find_staged_page_looks_in_the_folder_loop_a_excludes(conn, tmp_path, monkeypatch):
    """Loop A skips the Locus folder (invariant 5); the pull-back wants only that folder."""
    staging = tmp_path / "stage"
    staging.mkdir()
    (staging / "uuid-aaa.pdf").write_bytes(b"%PDF-1.7 page")
    (staging / "uuid-bbb.pdf").write_bytes(b"%PDF-1.7 notebook")

    index = {
        "uuid-aaa": ("daily-2026-06-01", "Locus", None),
        "uuid-bbb": ("EM Rates Trading", "brevan_howard", None),
    }
    monkeypatch.setattr("locus.capture.remarkable.build_uuid_index", lambda *a, **k: index)

    got = pd.find_staged_page(
        "2026-06-01", staging_dir=staging, runner=lambda a: (0, "", ""), folder="Locus"
    )
    assert got == staging / "uuid-aaa.pdf"

    # A date the device has not sent back is None, not an error.
    assert pd.find_staged_page(
        "2026-06-02", staging_dir=staging, runner=lambda a: (0, "", ""), folder="Locus"
    ) is None


def test_unchanged_page_is_skipped_without_a_model_call(conn, tmp_path, monkeypatch):
    """A scheduled pull must cost nothing on days the owner did not write."""
    oid = _proposed(conn, "alpha")
    page = _page(conn)
    pdf = tmp_path / "daily.pdf"
    pdf.write_bytes(b"%PDF-1.7 annotated")
    monkeypatch.setattr(
        "locus.capture.transcribe.render_pdf_pages", lambda *a, **k: [b"png"]
    )

    class _Boom:
        def __getattr__(self, _n):
            raise AssertionError("extraction must not run for an unchanged page")

    first = pd.pull_daily(
        conn, pdf, page_date=page.page_date,
        client=_FakeVision('{"regions":[{"anchor":"B1","ticked":true,"text":""}]}'),
    )
    assert first.status == "routed"
    assert _status(conn, oid) == "active"

    second = pd.pull_daily(conn, pdf, page_date=page.page_date, client=_Boom())
    assert second.status == "unchanged"

    # ...but a page that CHANGED is read again.
    pdf.write_bytes(b"%PDF-1.7 annotated more")
    third = pd.pull_daily(
        conn, pdf, page_date=page.page_date,
        client=_FakeVision('{"regions":[{"anchor":"B1","ticked":true,"text":"revised"}]}'),
    )
    assert third.status == "routed"
    assert _body(conn, oid)["why"] == "revised"


def test_missing_page_reports_not_on_device_rather_than_failing(conn, tmp_path, monkeypatch):
    _page(conn)
    monkeypatch.setattr(pd, "find_staged_page", lambda *a, **k: None)
    res = pd.pull_daily(conn, None, page_date="2026-06-01")
    assert res.status == "not-on-device"
    assert res.outcomes == []
