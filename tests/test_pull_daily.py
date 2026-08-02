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

def R(anchor: str, ticked, text: str, mark: str | None = None):
    """Build an ExtractedRegion. `ticked=True` means a tick; pass mark='cross' for a refusal."""
    if mark is None:
        mark = "tick" if ticked else "none"
    return pd.ExtractedRegion(anchor, mark == "tick", text, mark=mark)


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


def _legacy_blessing_page(conn, object_ids, today=date(2026, 6, 1)):
    """A page carrying `B*` blessing regions, as pages built before 2026-08-02 did.

    Blessings LEFT the daily page in the step-2 rebuild — he wants every approval at once in the
    terminal TUI, and no decision may live on two surfaces (plan §3). The four-way routing below
    still has to work, though, because a page delivered before the change can be annotated and
    pulled back after it. So the regions are seeded directly rather than composed.
    """
    page = _page(conn, today=today)
    for i, oid in enumerate(object_ids, 1):
        _anchor(conn, page.page_date, f"B{i}", "blessing", "object", str(oid))
    return page


def _status(conn, oid):
    return conn.execute("SELECT status FROM objects WHERE id=?", (oid,)).fetchone()["status"]


def _body(conn, oid):
    return state.get_object(conn, oid).body


def _anchor(conn, page_date, anchor, kind, target_kind, target_key, label=""):
    """Add a region to a persisted page. The fixture DB has no real connections to surface."""
    with conn:
        conn.execute(
            "INSERT INTO daily_anchors (page_date, anchor, kind, target_kind, target_key, label) "
            "VALUES (?,?,?,?,?,?)",
            (page_date, anchor, kind, target_kind, target_key, label),
        )


def _cfg(tmp_path):
    """A config stub for the cloud-fetch transport (no device, no network).

    Built from the REAL config models, not a SimpleNamespace. The first version invented
    `paths.notes_dir`; the code under test read the same invented name, so the test passed and
    the CLI crashed on the first live run. A stub that agrees with the bug is worse than none.
    """
    from types import SimpleNamespace

    from locus.config import PathsConfig, ReadingConfig

    return SimpleNamespace(
        reading=ReadingConfig(),
        paths=PathsConfig(
            db=tmp_path / "x.db",
            raw_store=tmp_path / "raw",
            incoming=tmp_path / "in",
            notes=tmp_path / "notes",
        ),
    )


# ---------- the four-way blessing table ----------


def test_ticked_no_writing_blesses(conn):
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", True, "")])
    assert _status(conn, oid) == "active"
    assert _body(conn, oid)["why"] == "agent's reason"  # untouched


def test_ticked_with_writing_applies_the_edit_then_blesses(conn):
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", True, "actually it is about fixing risk")])
    assert _status(conn, oid) == "active"
    assert _body(conn, oid)["why"] == "actually it is about fixing risk"


def test_writing_without_a_tick_corrects_but_leaves_it_proposed(conn):
    """The interesting case: 'keep working on this' is neither a yes nor a no."""
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", False, "narrow this to EM rates only")])
    assert _status(conn, oid) == "proposed", "an untitcked correction must NOT bless"
    assert _body(conn, oid)["why"] == "narrow this to EM rates only", "the edit must still land"


def test_neither_is_a_no_op_and_the_object_stays_pending(conn):
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", False, "")])
    assert _status(conn, oid) == "proposed"
    assert _body(conn, oid)["why"] == "agent's reason"
    # Still proposed => still waiting for a decision, now in `locus decide` rather than on the
    # page. Nothing about doing nothing may resolve it either way.


def test_every_outcome_including_the_no_op_is_logged(conn):
    """A rejection is as much signal as an acceptance."""
    a, b = _proposed(conn, "alpha"), _proposed(conn, "beta", created_at="2026-01-02T00:00:00+00:00")
    page = _legacy_blessing_page(conn, [a, b])
    pd.route_regions(conn, page.page_date, [R("B1", True, ""), R("B2", False, "")])

    counts = state.acceptance_counts(conn, surface=pd.SURFACE_BLESSING)
    assert counts[str(a)] == {"kept": 1}
    assert counts[str(b)] == {"rejected": 1}


# ---------- owner authority over the additive merge ----------


def test_owner_correction_survives_a_later_agent_re_proposal(conn):
    """Without this the correction lasts until tonight's structure run — the chore §9 forbids."""
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
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


# ---------- writing is content, not a checkbox ----------
#
# Every string here is from the first real annotated page (2026-07-30) or is the exact shape
# that broke on it. He wrote three times, all three were questions, and not one of them was in
# the box labelled for questions.


@pytest.mark.parametrize(
    "text,expected",
    [
        # Verbatim from the page. Note the first has NO question mark.
        ("what are the best methods for regime detection and do we want macro regime or "
         "other types", pd.WRITING_QUESTION),
        ("Is leetcode that important or should we focus on hackerrank/codeforces?",
         pd.WRITING_QUESTION),
        ("does this suggest we should we macro regime predictor in the tanker project?",
         pd.WRITING_QUESTION),
        # A proposal, not a query.
        ("should try this on the tanker project", pd.WRITING_IDEA),
        ("note to self: extend this to the regime model", pd.WRITING_IDEA),
        # A plain remark stays a remark — not everything is an object.
        ("yes exactly", pd.WRITING_NOTE),
        # The regression: "because" CONTAINS "use", and substring matching read a real recall
        # answer as a proposal, skipping its grade entirely.
        ("because the curve was inverted", pd.WRITING_NOTE),
    ],
)
def test_classify_writing(text, expected):
    assert pd.classify_writing(text) == expected


def test_a_question_written_under_a_connection_becomes_an_object(conn):
    """The whole point: he writes where the thought occurs, not where the form asks."""
    page = _page(conn)
    _anchor(conn, page.page_date, "C1", "connection", "doc", "papers/x.pdf")
    pd.route_regions(conn, page.page_date, [R("C1", None, "what are the best methods for "
                                              "regime detection?")])

    questions = state.list_objects(conn, type_="question")
    assert len(questions) == 1
    assert questions[0].status == "active", "his own words need no blessing"
    # ...and it is GROUNDED in the document that provoked it, so it resurfaces with it.
    # (list_objects does not load links; get_object does.)
    assert any(
        link.target_key == "papers/x.pdf" and link.relation == "raised_by"
        for link in state.links_for(conn, questions[0].id)
    )


def test_an_idea_written_under_a_connection_becomes_an_idea_not_a_question(conn):
    page = _page(conn)
    _anchor(conn, page.page_date, "C1", "connection", "doc", "papers/x.pdf")
    pd.route_regions(
        conn, page.page_date, [R("C1", None, "should try this on the tanker project")]
    )
    assert [o.title for o in state.list_objects(conn, type_="idea")] == [
        "should try this on the tanker project"
    ]
    assert state.list_objects(conn, type_="question") == []


def test_a_plain_remark_does_not_manufacture_an_object(conn):
    """Not every scribble is a thing to track — 'yes' must not fill the queue."""
    page = _page(conn)
    _anchor(conn, page.page_date, "C1", "connection", "doc", "papers/x.pdf")
    pd.route_regions(conn, page.page_date, [R("C1", None, "yes exactly")])
    assert state.list_objects(conn, type_="idea") == []
    assert state.list_objects(conn, type_="question") == []


def test_a_question_on_a_recall_line_is_not_graded_as_a_recall(conn):
    """SM-2 only works if its grades mean what they say.

    On the real page he wrote a QUESTION on R4 and it was graded 4 (correct-with-hesitation),
    pushing the item a day out on the strength of him not recalling it.
    """
    item = learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )
    page = _page(conn)
    before = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item.id,)).fetchone()
    pd.route_regions(conn, page.page_date, [
        R("R1", None, "does this suggest we should use a macro regime predictor here?")
    ])
    after = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item.id,)).fetchone()

    assert after["reps"] == before["reps"], "a question is not a recall attempt"
    assert after["due"] == before["due"], "it must still come back"
    # ...but the thought is not thrown away.
    assert len(state.list_objects(conn, type_="question")) == 1


def test_an_idea_written_in_the_question_box_is_filed_as_an_idea(conn):
    page = _page(conn)
    pd.route_regions(conn, page.page_date, [R("Q1", None, "should build a sim for this")])
    assert len(state.list_objects(conn, type_="idea")) == 1
    assert state.list_objects(conn, type_="question") == []


def test_re_pulling_does_not_duplicate_a_captured_thought(conn):
    page = _page(conn)
    for _ in range(2):
        pd.route_regions(conn, page.page_date, [R("Q1", None, "what drives carry in LNG?")])
    assert len(state.list_objects(conn, type_="question")) == 1


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
    page = _legacy_blessing_page(conn, [oid])

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
        '{"regions":[{"anchor":"b1","mark":"tick","text":" tighten this "},'
        '{"anchor":"R1","mark":"none","text":""}]}'
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


def test_an_untouched_page_is_not_fetched_as_annotated(monkeypatch, tmp_path):
    """No strokes means no vision call — an unwritten page is the normal case, not an error.

    The staged-`<uuid>.pdf` route this replaced could not tell the difference: the device hands
    back the ORIGINAL file for an uploaded PDF, so a blank page and a covered one looked
    identical and both cost a vision pass.
    """
    from locus.capture.rmdoc import RmDoc

    monkeypatch.setattr(pd, "load", lambda: _cfg(tmp_path))
    monkeypatch.setattr("locus.capture.rmdoc.fetch_rmdoc", lambda *a, **k: tmp_path / "x.rmdoc")
    monkeypatch.setattr(
        "locus.capture.rmdoc.read_rmdoc", lambda *a, **k: RmDoc("u", b"%PDF-1.7", [])
    )
    assert pd.fetch_annotated_page("2026-06-01") is None


def test_a_page_absent_from_the_cloud_is_not_an_error(monkeypatch, tmp_path):
    monkeypatch.setattr(pd, "load", lambda: _cfg(tmp_path))

    def _boom(*a, **k):
        raise RuntimeError("rmapi get failed: file doesn't exist")

    monkeypatch.setattr("locus.capture.rmdoc.fetch_rmdoc", _boom)
    assert pd.fetch_annotated_page("2026-06-01") is None


def test_the_spend_guard_keys_on_ink_not_on_the_rendered_bytes(conn, tmp_path, monkeypatch):
    """Compositing is not byte-reproducible, so a file hash would re-pay vision on every run.

    Found 2026-07-30: pymupdf stamps each save, so two composites of the SAME strokes differ.
    The guard therefore keys on the strokes, which is what "has he written anything new" means.
    """
    _proposed(conn, "alpha")
    page = _page(conn)
    monkeypatch.setattr("locus.capture.transcribe.render_pdf_pages", lambda *a, **k: [b"png"])

    first_pdf = tmp_path / "a.pdf"
    first_pdf.write_bytes(b"%PDF-1.7 render-one")
    monkeypatch.setattr(pd, "fetch_annotated_page", lambda *a, **k: (first_pdf, "INK-1"))
    assert pd.pull_daily(
        conn, None, page_date=page.page_date,
        client=_FakeVision('{"regions":[{"anchor":"B1","mark":"tick","text":""}]}'),
    ).status == "routed"

    # A DIFFERENT rendering of the SAME ink must not pay for vision again.
    second_pdf = tmp_path / "b.pdf"
    second_pdf.write_bytes(b"%PDF-1.7 render-two-different-bytes")

    class _Boom:
        def __getattr__(self, _n):
            raise AssertionError("unchanged ink must not reach the model")

    monkeypatch.setattr(pd, "fetch_annotated_page", lambda *a, **k: (second_pdf, "INK-1"))
    assert pd.pull_daily(conn, None, page_date=page.page_date, client=_Boom()).status == (
        "unchanged"
    )

    # New ink IS read again.
    monkeypatch.setattr(pd, "fetch_annotated_page", lambda *a, **k: (second_pdf, "INK-2"))
    assert pd.pull_daily(
        conn, None, page_date=page.page_date,
        client=_FakeVision('{"regions":[{"anchor":"B1","mark":"tick","text":"revised"}]}'),
    ).status == "routed"


def test_unchanged_page_is_skipped_without_a_model_call(conn, tmp_path, monkeypatch):
    """A scheduled pull must cost nothing on days the owner did not write."""
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
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
        client=_FakeVision('{"regions":[{"anchor":"B1","mark":"tick","text":""}]}'),
    )
    assert first.status == "routed"
    assert _status(conn, oid) == "active"

    second = pd.pull_daily(conn, pdf, page_date=page.page_date, client=_Boom())
    assert second.status == "unchanged"

    # ...but a page that CHANGED is read again.
    pdf.write_bytes(b"%PDF-1.7 annotated more")
    third = pd.pull_daily(
        conn, pdf, page_date=page.page_date,
        client=_FakeVision('{"regions":[{"anchor":"B1","mark":"tick","text":"revised"}]}'),
    )
    assert third.status == "routed"
    assert _body(conn, oid)["why"] == "revised"


def test_missing_page_reports_not_on_device_rather_than_failing(conn, tmp_path, monkeypatch):
    _page(conn)
    monkeypatch.setattr(pd, "fetch_annotated_page", lambda *a, **k: None)
    res = pd.pull_daily(conn, None, page_date="2026-06-01")
    assert res.status == "not-on-device"
    assert res.outcomes == []


# ---------- the cross: an explicit, durable refusal ----------


def test_cross_archives_instead_of_blessing(conn):
    """The shipped bug: the prompt counted a cross as 'a mark is present, therefore ticked',
    so crossing something out BLESSED it — the exact opposite of what a person means."""
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", None, "", mark="cross")])
    assert _status(conn, oid) == "archived"


def test_a_crossed_object_is_archived_not_merely_deprioritised(conn):
    """Without an archive path an unwanted object only rotates to the back of the queue.

    It used to be checked by re-composing and asserting the object was absent from the page's
    blessing section. That section is gone (approvals moved to `locus decide`, plan §3), so the
    durable fact is asserted directly: `archived` is what keeps it off EVERY pending surface."""
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", None, "", mark="cross")])
    assert _status(conn, oid) == "archived"
    assert not conn.execute(
        "SELECT 1 FROM objects WHERE id=? AND status='proposed'", (oid,)
    ).fetchone()


def test_a_cross_keeps_any_writing(conn):
    """Why it was wrong is worth more than the rejection itself."""
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(
        conn, page.page_date, [R("B1", None, "conflates two different things", mark="cross")]
    )
    assert _status(conn, oid) == "archived"
    assert _body(conn, oid)["why"] == "conflates two different things"


def test_a_cross_is_logged_as_a_rejection(conn):
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", None, "", mark="cross")])
    assert state.acceptance_counts(conn, surface=pd.SURFACE_BLESSING)[str(oid)] == {"rejected": 1}


def test_an_archived_object_cannot_be_revived_by_the_agent(conn):
    """propose-never-mutate: the owner's refusal is not an agent's to undo."""
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", None, "", mark="cross")])
    state.upsert_object(conn, type_="concept", title="alpha", body={"why": "re-proposed"})
    assert _status(conn, oid) == "archived"


def test_changing_a_tick_to_a_cross_re_routes(conn):
    """The re-pull guard keys on the mark SHAPE — a reversal must not read as 'unchanged'."""
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", True, "")])
    assert _status(conn, oid) == "active"
    pd.route_regions(conn, page.page_date, [R("B1", None, "", mark="cross")])
    assert _status(conn, oid) == "archived"


def test_an_unrecognised_mark_never_becomes_an_affirmative(conn):
    oid = _proposed(conn, "alpha")
    page = _legacy_blessing_page(conn, [oid])
    pd.route_regions(conn, page.page_date, [R("B1", None, "", mark="scribble?")])
    assert _status(conn, oid) == "proposed"
