"""P6: Canvas deadlines become task proposals, once each, and only his and still to do."""

from __future__ import annotations

from datetime import datetime, timezone

from locus import canvas_deadlines as cd

NOW = datetime(2026, 11, 15, 12, tzinfo=timezone.utc)


def a(i, name, due, state="unsubmitted", **kw):
    return {"id": i, "name": name, "due_at": due, "published": True,
            "submission": {"workflow_state": state}, **kw}


ASSIGN = {
    "b1": [a(1, "Numerical Algorithms Practical - Submission", "2026-11-05T16:00:00Z", "submitted"),
           a(2, "Optimisation Practical - Submission", "2026-11-19T16:00:00Z")],
    "b3": [a(3, "MT7 Logbook submission - Cultured meat production", "2026-11-25T23:59:00Z"),
           a(4, "3YP Logbook Submission - deadline 20th May, 1 pm", "2027-05-19T12:00:00Z"),
           a(5, "MT4 Logbook old", "2026-11-04T23:59:00Z")],
    "b15": [a(6, "Submit report: Answers to Questions 1-30", None)],
}
SKIP = {"b3": {"cultured-meat-production-a-food-supply-chain-for-the-future"}}


def test_only_his_future_unsubmitted_dated_deadlines_are_proposed():
    p = cd.plan(ASSIGN, [], {}, now=NOW, skip=SKIP)
    assert [(x.course, x.assignment) for x in p.propose] == [("b1", 2), ("b3", 4)]
    assert p.submitted == ["b1: Numerical Algorithms Practical - Submission"]
    assert p.not_his == ["b3: MT7 Logbook submission - Cultured meat production"]
    assert p.past == ["b3: MT4 Logbook old"] and len(p.no_date) == 1


def test_one_kairos_already_has_or_one_proposed_before_is_not_proposed_again():
    tasks = [{"locus": "b1", "due": {"type": "at", "value": "2026-11-19T17:00:00Z"}},
             {"locus": "b3", "due": {"type": "date", "value": "2027-05-19"}}]
    p = cd.plan(ASSIGN, tasks, {}, now=NOW, skip=SKIP)
    assert not p.propose and len(p.in_kairos) == 2
    p = cd.plan(ASSIGN, [], {"2": {}, "4": {}}, now=NOW, skip=SKIP)
    assert not p.propose and p.proposed_before == 2
