"""`locus decide`: what is pending, which surface owns it, and what a decision actually does.

The first test is the point of the module. "Nothing in the tui should also ever be on the daily
note, they should ALWAYS be two separate approval things. I should not be able to approve the same
thing on the daily page and the tui."
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from locus.agent import compose_daily as cd
from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.decide import queue as Q


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "decide.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _proposed(conn, title, *, created_at="2026-01-01T00:00:00+00:00", why="because"):
    oid, _ = state.upsert_object(
        conn, type_="concept", title=title, body={"why": why}, now=lambda: created_at
    )
    state.add_links(
        conn, oid, [state.ObjectLink("entity", state.entity_key(title, "concept"), "about")]
    )
    return oid


def _thread(conn, title):
    oid, _ = state.upsert_object(conn, type_="question", title=title)
    state.set_status(conn, oid, "active")
    return oid


def _mark(conn, text, uri="books/apm.pdf", page=1):
    with conn:
        conn.execute(
            "INSERT INTO pdf_annotations (source_uri, pdf_page, kind, bbox_key, covered_text, "
            "in_margin, captured_at) VALUES (?,?,'underline',?,?,0,'2026-07-30')",
            (uri, page, f"k{page}", text),
        )


def _target(conn, *, title="A Paper", swept, marks=0, subject="regime-ml"):
    with conn:
        cur = conn.execute(
            "INSERT INTO reading_targets (doc_uuid, device_path, source_uri, proposal_id, "
            "linked_by, created_at, device_folder, marks, title, subject_kind, subject_label) "
            "VALUES (?,?,?,NULL,'manual','2026-07-01','In-Progress',?,?,'project',?)",
            (f"u-{title}", f"/Reading/In-Progress/{title}", f"raw/{title}.pdf", marks, title,
             subject),
        )
        conn.execute("UPDATE reading_targets SET last_swept=? WHERE id=?", (swept, cur.lastrowid))
    return cur.lastrowid


_PASSAGE = "the extreme case in which every hedge fund holds a copy of the same portfolio"


# --- THE INVARIANT ------------------------------------------------------------------------------


def test_no_decision_appears_on_both_surfaces(conn):
    """His rule. Two surfaces that can both resolve one item is how a decision gets lost: he ticks
    it on paper, clears it in the terminal having forgotten, and the second silently overwrites
    the first — or the flywheel learns twice from one judgement."""
    for i in range(6):
        _proposed(conn, f"concept {i}", created_at=f"2026-07-{i + 1:02d}T00:00:00+00:00")
    _thread(conn, "a question of mine")
    _mark(conn, _PASSAGE)
    _target(conn, swept="2026-01-01T00:00:00+00:00")

    tui = {d.key for d in Q.pending(conn).flat()}
    page = Q.page_keys(conn)

    assert tui, "the queue must not be trivially empty for this to mean anything"
    assert page, "nor the page"
    assert tui & page == set(), f"a decision is on both surfaces: {tui & page}"


def test_an_item_the_page_is_offering_is_removed_from_the_queue(conn):
    """Directly exercised rather than left to the kinds happening not to overlap today: once
    mark-intent inference lands, a marked passage will be BOTH a Think item and (when ambiguous)
    a TUI decision, and the subtraction is what stops them colliding."""
    tid = _target(conn, swept="2026-01-01T00:00:00+00:00")
    key = f"reading:{tid}"
    assert key in {d.key for d in Q.pending(conn).flat()}

    # Pretend the page is offering it.
    original = Q.page_keys
    try:
        Q.page_keys = lambda _conn: {key}
        assert key not in {d.key for d in Q.pending(conn).flat()}
    finally:
        Q.page_keys = original


def test_the_page_no_longer_offers_a_proposed_object_at_all(conn):
    """The other half of the split: blessings left the page in the step-2 rebuild."""
    _proposed(conn, "alpha")
    page = cd.compose(conn)
    assert not [a for a in page.anchors if a.kind == "blessing"]


# --- what the subtraction is allowed to cost -----------------------------------------------------


def test_page_keys_does_not_pay_for_a_retrieval_pass(conn, monkeypatch):
    """Opening the TUI ran the cross-encoder once per not-understood mark and re-derived every
    cross-corpus pair — 31s and 77s of CPU to show one decision (measured 2026-08-03). The queue
    wants keys, and neither section produces a key it can collide with."""
    def boom(*_a, **_k):
        raise AssertionError("page_keys must not run a retrieval pass")

    monkeypatch.setattr(cd, "build_rereads", boom)
    monkeypatch.setattr(cd, "build_connections", boom)
    Q.page_keys(conn)


def test_the_skipped_sections_produce_nothing_the_tui_could_offer(conn):
    """WHY the skip is safe, asserted rather than reasoned about: the two namespaces it gives up
    are disjoint from every key this surface issues."""
    _proposed(conn, "alpha")
    _thread(conn, "a question of mine")
    _mark(conn, _PASSAGE)
    _target(conn, swept="2026-01-01T00:00:00+00:00")

    tui = {d.key for d in Q.pending(conn).flat()}
    assert tui, "the queue must not be trivially empty for this to mean anything"
    assert not [k for k in tui if k.startswith(cd.RETRIEVAL_BACKED_KEY_PREFIXES)]


def test_the_skip_never_drops_a_key_the_full_page_would_offer(conn):
    """The direction that matters. Over-reporting only subtracts more from the queue; UNDER-
    reporting puts one decision on both surfaces, which is the thing his rule forbids. Skipping a
    pool frees seats and so can only ever add — asserted here rather than argued, because the next
    person to make this faster will be tempted by a section that is not safe to drop."""
    _proposed(conn, "alpha")
    _thread(conn, "a question of mine")
    _mark(conn, _PASSAGE)
    _target(conn, swept="2026-01-01T00:00:00+00:00")

    full = {k for k, _kind in cd.compose(conn).items_shown()}
    lost = {
        k for k in full - Q.page_keys(conn)
        if not k.startswith(cd.RETRIEVAL_BACKED_KEY_PREFIXES)
    }
    assert not lost, f"the skip dropped keys the page would have offered: {lost}"


# --- proposed objects ----------------------------------------------------------------------------


def test_objects_are_offered_oldest_first_and_carry_their_grounding(conn):
    _proposed(conn, "newest", created_at="2026-07-30T00:00:00+00:00")
    _proposed(conn, "oldest", created_at="2026-01-01T00:00:00+00:00")

    items = Q.object_decisions(conn)
    assert [d.title for d in items] == ["concept — oldest", "concept — newest"]
    assert "oldest (concept)" in items[0].grounding
    assert "\x1f" not in items[0].grounding, "the entity separator is invisible when printed"


def test_blessing_makes_an_object_active_and_records_the_judgement(conn):
    oid = _proposed(conn, "alpha")
    decision = Q.object_decisions(conn)[0]
    assert Q.resolve(conn, decision, accept=True) == "blessed"

    assert state.get_object(conn, oid).status == "active"
    assert state.acceptance_counts(conn, surface=Q.SURFACE_OBJECT)[str(oid)] == {"kept": 1}


def test_dropping_archives_so_it_is_never_offered_again(conn):
    oid = _proposed(conn, "alpha")
    Q.resolve(conn, Q.object_decisions(conn)[0], accept=False)

    assert state.get_object(conn, oid).status == "archived"
    assert Q.object_decisions(conn) == [], "archived is what keeps it off every surface"


def test_a_typed_correction_is_the_owners_and_survives_a_re_proposal(conn):
    """Same asymmetry the page enforces: his wording goes through `apply_owner_edit`, which
    carries a marker the agent's additive merge cannot overwrite."""
    oid = _proposed(conn, "alpha", why="agent's reason")
    Q.resolve(conn, Q.object_decisions(conn)[0], accept=True, note="narrow this to EM rates")

    state.upsert_object(conn, type_="concept", title="alpha", body={"why": "agent tries again"})
    assert state.get_object(conn, oid).body["why"] == "narrow this to EM rates"


# --- abandoned reading ---------------------------------------------------------------------------


def _now():
    return datetime(2026, 8, 2, tzinfo=timezone.utc)


def test_a_reading_untouched_for_a_fortnight_is_asked_about(conn):
    _target(conn, title="Stalled", swept=(_now() - timedelta(days=20)).isoformat())
    items = Q.abandoned_readings(conn, now=_now())
    assert [d.title for d in items] == ["Stalled"]
    assert "20 days" in items[0].detail
    assert "wrong paper, or just not yet" in items[0].detail, "it ASKS rather than concluding"


def test_a_reading_he_has_marked_is_not_treated_as_abandoned(conn):
    _target(conn, title="Being read", swept=(_now() - timedelta(days=40)).isoformat(), marks=12)
    assert Q.abandoned_readings(conn, now=_now()) == []


def test_a_recent_reading_is_left_alone(conn):
    _target(conn, title="Fresh", swept=(_now() - timedelta(days=2)).isoformat())
    assert Q.abandoned_readings(conn, now=_now()) == []


def test_the_abandonment_answer_reaches_the_thing_that_tunes_proposals(conn):
    """His requirement: the signal must DO something rather than sit in a table.

    `acceptance_log(surface='discovery')` is the per-channel prior that already tunes what gets
    proposed — so "wasn't useful" changes future recommendations rather than being recorded.
    """
    _target(conn, title="Stalled", swept=(_now() - timedelta(days=20)).isoformat())
    decision = Q.abandoned_readings(conn, now=_now())[0]

    assert Q.resolve(conn, decision, accept=False) == "recorded as not useful"
    counts = state.acceptance_counts(conn, surface=Q.SURFACE_DISCOVERY)
    assert counts[decision.key] == {"rejected": 1}


def test_saying_still_reading_defers_it_rather_than_resolving_it(conn):
    _target(conn, title="Stalled", swept=(_now() - timedelta(days=20)).isoformat())
    decision = Q.abandoned_readings(conn, now=_now())[0]
    Q.resolve(conn, decision, accept=True)
    # last_swept moved to now, so it is not asked about again tomorrow.
    assert Q.abandoned_readings(conn, now=_now()) == []


# --- the queue itself -----------------------------------------------------------------------------


def test_sections_are_grouped_by_type_and_ordered(conn):
    """"in neat sections by type in a tui"."""
    _proposed(conn, "alpha")
    _target(conn, title="Stalled", swept=(_now() - timedelta(days=20)).isoformat())
    queue = Q.pending(conn, now=_now())

    assert list(queue.sections) == list(Q.TUI_KINDS)
    assert [d.kind for d in queue.flat()] == [Q.KIND_OBJECT, Q.KIND_ABANDONED]
    assert queue.total == 2


def test_an_empty_queue_is_a_valid_state(conn):
    queue = Q.pending(conn)
    assert queue.total == 0 and queue.flat() == []
