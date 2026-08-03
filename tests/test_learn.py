"""Interview-prep aids: gap detection, practice generation, SM-2 review (plan §3.6, §8.4).

Gap detection and SM-2 are model-free by construction (joins and arithmetic); practice generation
injects the runner. The load-bearing assertions are that the EXPLANATION gap fires on exactly the
"built with it, never wrote about it" case, and that a practice question referencing a proposition
we never offered is dropped.
"""

from __future__ import annotations

import json
from datetime import date
from pathlib import Path

import pytest

from locus.agent import state
from locus.agent.claude import ClaudeError, ClaudeResult
from locus.agent.state import ObjectLink
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.learn import gaps, practice, review


def _doc(conn, doc_id, *, title, uri, category, gap_flags=None):
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title, "
            "category, gap_flags, ingested_at, ingest_model) VALUES (?,?,?,?,?,?,?,?,?,?)",
            (doc_id, f"h{doc_id}", "markdown", uri, f"r{doc_id}", title, category,
             json.dumps(gap_flags or []), "2026-01-01T00:00:00Z", "test"),
        )
        conn.execute(
            "INSERT INTO sections (id, doc_id, position, title, summary) VALUES (?,?,0,?,'s')",
            (doc_id, doc_id, title),
        )


def _entity(conn, doc_id, name, type_="concept"):
    with conn:
        conn.execute(
            "INSERT OR IGNORE INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
            (doc_id, doc_id, name, type_),
        )


def _prop(conn, doc_id, text):
    with conn:
        cur = conn.execute(
            "INSERT INTO propositions (section_id, doc_id, position, text, embed_model) "
            "VALUES (?,?,(SELECT COALESCE(MAX(position),-1)+1 FROM propositions WHERE section_id=?),?,'m')",
            (doc_id, doc_id, doc_id, text),
        )
    return cur.lastrowid


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "learn.db"
    migrate(db)
    c = get_connection(db)
    # The tanker-flow shape: a repo implementing two concepts; he has written about one of them.
    _doc(c, 1, title="tanker-flow", uri="repos/tanker-flow", category="project",
         gap_flags=["no out-of-sample validation is described"])
    _doc(c, 2, title="Notes on laden ton-miles", uri="notes/ton-miles.md", category="note")
    _doc(c, 3, title="AIS paper", uri="papers/ais.pdf", category="paper")
    _entity(c, 1, "laden ton-miles")
    _entity(c, 1, "AIS interpolation")
    _entity(c, 2, "laden ton-miles")   # written about in his own words
    _entity(c, 3, "AIS interpolation")  # only ever read about
    for i in range(4):
        # Long enough to clear review._MIN_PROMPT_CHARS — real propositions are full claims,
        # and a fixture of 30-char stubs would silently exercise the wrong path.
        _prop(c, 3, f"AIS interpolation over leg {i} must resample irregular position fixes before any signal is derived from them.")
    oid, _ = state.upsert_object(c, type_="project", title="tanker-flow")
    state.add_links(c, oid, [ObjectLink("doc", "repos/tanker-flow", "implements")])
    yield c
    c.close()


@pytest.fixture()
def tanker(conn) -> int:
    """The seeded tanker-flow project object's id."""
    return conn.execute("SELECT id FROM objects WHERE title='tanker-flow'").fetchone()["id"]


# --- gap detection -------------------------------------------------------------------------------


def test_explanation_gap_fires_on_used_but_never_explained(conn, tanker):
    found = gaps.gaps_for_object(conn, tanker)
    explanation = [g for g in found if g.kind == "explanation"]
    assert [g.subject for g in explanation] == ["AIS interpolation"]
    assert "never written about" in explanation[0].detail
    assert explanation[0].sources == ["tanker-flow"]


def test_a_concept_he_has_written_about_is_not_an_explanation_gap(conn, tanker):
    subjects = [g.subject for g in gaps.gaps_for_object(conn, tanker)
                if g.kind == "explanation"]
    assert "laden ton-miles" not in subjects


def test_a_recorded_belief_position_also_closes_the_explanation_gap(conn, tanker):
    state.record_position(conn, subject_kind="concept",
                          subject_key=state.entity_key("AIS interpolation", "concept"),
                          stance="linear interpolation is good enough between fixes",
                          dated_at="2026-03-01")
    subjects = [g.subject for g in gaps.gaps_for_object(conn, tanker)
                if g.kind == "explanation"]
    assert subjects == []


def test_thin_coverage_fires_on_a_concept_with_almost_no_propositions(conn, tanker):
    kinds = {(g.kind, g.subject) for g in gaps.gaps_for_object(conn, tanker)}
    # 'laden ton-miles' is explained but carries 0 propositions -> thin, not an explanation gap.
    assert ("thin_coverage", "laden ton-miles") in kinds


def test_ingest_flagged_gaps_are_included_and_audit_lines_are_not(conn, tanker):
    _doc(conn, 4, title="noisy", uri="notes/noisy.md", category="note",
         gap_flags=["math-OCR kept original text on p.4",
                    "the derivation of the bound is not given"])
    with conn:
        conn.execute("INSERT INTO object_links (object_id, target_kind, target_key, relation) "
                     "VALUES (?,'doc','notes/noisy.md','relates')", (tanker,))
    details = [g.detail for g in gaps.gaps_for_object(conn, tanker)
               if g.kind == "flagged"]
    assert "the derivation of the bound is not given" in details
    assert not any("math-OCR" in d for d in details)  # pipeline audit line, not a knowledge gap


def test_gaps_are_ordered_strongest_signal_first(conn, tanker):
    kinds = [g.kind for g in gaps.gaps_for_object(conn, tanker)]
    assert kinds.index("explanation") < kinds.index("thin_coverage")
    assert kinds.index("thin_coverage") < kinds.index("flagged")


def test_gaps_for_an_unknown_object_are_empty(conn):
    assert gaps.gaps_for_object(conn, 9999) == []


def test_alias_variants_count_as_having_written_about_it(conn, tanker):
    """Writing about 'AIS-interpolation' must close the gap on canonical 'AIS interpolation'."""
    _doc(conn, 5, title="Interp notes", uri="notes/interp.md", category="note")
    _entity(conn, 5, "AIS-interpolation")
    with conn:
        for variant in ("AIS interpolation", "AIS-interpolation"):
            conn.execute(
                "INSERT INTO entity_aliases (variant_name, variant_type, canonical_name, "
                "canonical_type, cluster_id, tier) VALUES (?,?,?,?,1,'punct')",
                (variant, "concept", "AIS interpolation", "concept"),
            )
    subjects = [g.subject for g in gaps.gaps_for_object(conn, tanker)
                if g.kind == "explanation"]
    assert "AIS interpolation" not in subjects


# --- practice generation ---------------------------------------------------------------------------


def test_practice_questions_keep_the_proposition_as_the_reference_answer(conn):
    cands = practice.candidates_for_concept(conn, "AIS interpolation")
    assert cands, "expected propositions from the AIS paper"
    pid = cands[0].id

    def runner(p, m):
        return ClaudeResult(text=json.dumps({"questions": [
            {"proposition_id": pid, "question": "Why can't you assume AIS fixes are evenly spaced?"}
        ]}))

    out = practice.generate_practice(conn, cands, runner=runner)
    assert len(out.items) == 1
    assert out.items[0].answer == cands[0].text  # the stored proposition IS the answer
    assert out.items[0].source_title == "AIS paper"


def test_a_question_about_a_proposition_we_never_offered_is_dropped(conn):
    cands = practice.candidates_for_concept(conn, "AIS interpolation")
    def runner(p, m):
        return ClaudeResult(text=json.dumps({"questions": [
            {"proposition_id": 99999, "question": "What is the Black-Scholes PDE?"}
        ]}))

    assert practice.generate_practice(conn, cands, runner=runner).items == []


def test_practice_selection_is_gap_driven(conn, tanker):
    """§12.3: what he cannot explain comes before what he already can."""
    cands = practice.candidates_for_object(conn, tanker)
    assert cands and cands[0].concept == "AIS interpolation"  # the explanation gap leads


def test_practice_respects_max_items(conn):
    cands = practice.candidates_for_concept(conn, "AIS interpolation")
    def runner(p, m):
        return ClaudeResult(text=json.dumps({"questions": [
            {"proposition_id": c.id, "question": f"Q{c.id}?"} for c in cands
        ]}))

    assert len(practice.generate_practice(conn, cands, runner=runner, max_items=2).items) == 2


def test_practice_degrades_on_model_failure(conn):
    def failing(p, m):
        raise ClaudeError("nope")

    out = practice.generate_practice(
        conn, practice.candidates_for_concept(conn, "AIS interpolation"), runner=failing
    )
    assert out.degraded is True and out.items == []


def test_no_candidates_means_no_model_call(conn):
    def explode(p, m):
        raise AssertionError("must not call the model with nothing to ask about")

    assert practice.generate_practice(conn, [], runner=explode).items == []


# --- SM-2 -----------------------------------------------------------------------------------------


def test_scheduling_is_idempotent_and_due_immediately(conn):
    first = review.schedule_prompt(conn, prompt_kind="proposition", prompt_ref="7",
                                   today=date(2026, 6, 1))
    again = review.schedule_prompt(conn, prompt_kind="proposition", prompt_ref="7",
                                   today=date(2026, 6, 2))
    assert first.id == again.id
    assert first.due == "2026-06-01"  # never seen -> nothing to wait for


def test_sm2_interval_progression(conn):
    ease, interval, reps = review.next_interval(grade=5, ease=2.5, interval=0, reps=0)
    assert (interval, reps) == (1, 1)
    ease, interval, reps = review.next_interval(grade=5, ease=ease, interval=interval, reps=reps)
    assert (interval, reps) == (6, 2)
    ease, interval, reps = review.next_interval(grade=4, ease=ease, interval=interval, reps=reps)
    assert interval == round(6 * ease) and reps == 3


def test_a_lapse_resets_repetitions_but_keeps_ease_tracking_difficulty(conn):
    ease, interval, reps = review.next_interval(grade=1, ease=2.5, interval=30, reps=5)
    assert (interval, reps) == (1, 0)      # re-learn from tomorrow
    assert ease < 2.5                       # but the item is remembered as hard
    assert ease >= 1.3


def test_ease_floors_at_1_3(conn):
    ease = 2.5
    for _ in range(10):
        ease, _i, _r = review.next_interval(grade=0, ease=ease, interval=1, reps=0)
    assert ease == pytest.approx(1.3)


def test_grading_reschedules_and_records_history(conn):
    item = review.schedule_prompt(conn, prompt_kind="proposition", prompt_ref="7",
                                  today=date(2026, 6, 1))
    updated = review.grade_item(conn, item.id, 5, today=date(2026, 6, 1))
    assert updated.due == "2026-06-02" and updated.reps == 1 and updated.last_grade == 5
    assert review.due_items(conn, today=date(2026, 6, 1)) == []       # no longer due today
    assert len(review.due_items(conn, today=date(2026, 6, 2))) == 1   # due tomorrow


def test_due_items_respects_the_daily_cap(conn):
    for ref in range(10):
        review.schedule_prompt(conn, prompt_kind="proposition", prompt_ref=str(ref),
                               today=date(2026, 6, 1))
    assert len(review.due_items(conn, today=date(2026, 6, 1), limit=5)) == 5


def test_grading_an_unknown_item_returns_none(conn):
    assert review.grade_item(conn, 999, 4) is None


def test_an_invalid_grade_is_rejected(conn):
    with pytest.raises(ValueError):
        review.next_interval(grade=9, ease=2.5, interval=1, reps=1)


def test_prompt_with_a_deleted_referent_degrades_rather_than_vanishing(conn):
    pid = _prop(conn, 3, "some claim")
    item = review.schedule_prompt(conn, prompt_kind="proposition", prompt_ref=str(pid))
    with conn:
        conn.execute("DELETE FROM propositions WHERE id=?", (pid,))
    text, _source = review.resolve_prompt(conn, item)
    assert "no longer in the corpus" in text
    assert review.due_items(conn, today=date(2030, 1, 1))  # the history row survives


def test_ast_identifiers_are_not_reported_as_knowledge_gaps(conn, tanker):
    """The first live run buried the one real finding (Kalman filter) under Alembic boilerplate:
    `upgrade`, `run_migrations_offline`, `main`. gaps.py now shares the proposer's filters."""
    for name in ("upgrade", "run_migrations_offline", "main", "_rebuild_constraint"):
        _entity(conn, 1, name, "method")
    with conn:
        for name in ("upgrade", "run_migrations_offline", "main", "_rebuild_constraint",
                     "AIS interpolation"):
            conn.execute(
                "INSERT OR IGNORE INTO entity_aliases (variant_name, variant_type, canonical_name, "
                "canonical_type, cluster_id, tier) VALUES (?,?,?,?,1,'identity')",
                (name, "method" if name != "AIS interpolation" else "concept", name,
                 "method" if name != "AIS interpolation" else "concept"),
            )
    subjects = {g.subject for g in gaps.gaps_for_object(conn, tanker)}
    assert not ({"upgrade", "run_migrations_offline", "main", "_rebuild_constraint"} & subjects)


def test_tool_typed_entities_are_not_gaps(conn, tanker):
    """Same kind-filter as the proposer: never having 'written about Python' is not a gap."""
    _entity(conn, 1, "Python", "tool")
    subjects = {g.subject for g in gaps.gaps_for_object(conn, tanker)}
    assert "Python" not in subjects


def test_document_fallback_honours_retrieval_order_not_proposition_id(conn):
    """A live synthesise() generated practice off a Python tutorial because the lowest doc id
    won. The fallback must follow the ORDER it is given (retrieval relevance)."""
    _doc(conn, 10, title="Python tutorial", uri="notes/py.md", category="note")
    _doc(conn, 11, title="Market making repo", uri="repos/mm", category="project")
    for i in range(5):
        _prop(conn, 10, f"an IDE offers code completion ({i})")
    for i in range(5):
        _prop(conn, 11, f"inventory-skewed quoting manages risk ({i})")

    # Retrieval ranked the market-making doc first, despite its higher id / later propositions.
    cands = practice.candidates_from_documents(conn, [11, 10])
    assert cands[0].doc_title == "Market making repo"
    assert {c.doc_title for c in cands} == {"Market making repo", "Python tutorial"}  # both reached


def test_document_fallback_caps_any_single_document(conn):
    """One verbose document must not monopolise the practice set."""
    _doc(conn, 10, title="Verbose", uri="notes/v.md", category="note")
    _doc(conn, 11, title="Terse", uri="notes/t.md", category="note")
    for i in range(20):
        _prop(conn, 10, f"claim {i}")
    _prop(conn, 11, "the one terse claim")
    cands = practice.candidates_from_documents(conn, [10, 11])
    assert sum(c.doc_title == "Verbose" for c in cands) <= 3
    assert any(c.doc_title == "Terse" for c in cands)


# ---------- enrolment (the reason review_schedule sat at zero rows) ----------


def test_enrolment_only_draws_from_blessed_objects(conn, tanker):
    """Unblessed means he has not agreed it matters; enrolling it would be presumptuous."""
    assert review.enrol_from_blessed_objects(conn) == []

    state.set_status(conn, tanker, "active")
    added = review.enrol_from_blessed_objects(conn)
    assert added, "a blessed object with propositions must yield review items"
    assert all(i.prompt_kind == "proposition" for i in added)


def test_enrolment_never_duplicates_and_eventually_exhausts(conn, tanker):
    """It ACCUMULATES by design (a few more each night) but never re-adds the same prompt.

    Strict "second run is a no-op" would be the wrong contract: gradual growth is the point.
    What must hold is that a prompt is scheduled at most once, and that repeated runs converge
    on the available material rather than growing without bound.
    """
    state.set_status(conn, tanker, "active")
    for _ in range(10):
        review.enrol_from_blessed_objects(conn)

    rows = conn.execute(
        "SELECT prompt_kind, prompt_ref, COUNT(*) n FROM review_schedule "
        "GROUP BY prompt_kind, prompt_ref HAVING n > 1"
    ).fetchall()
    assert rows == [], f"duplicate schedule rows: {[tuple(r) for r in rows]}"

    scheduled = conn.execute("SELECT COUNT(*) FROM review_schedule").fetchone()[0]
    available = conn.execute("SELECT COUNT(*) FROM propositions").fetchone()[0]
    assert scheduled <= available
    assert review.enrol_from_blessed_objects(conn) == [], "converged: nothing left to add"


def test_enrolment_is_gradual_not_a_dump(conn, tanker):
    """A saturated queue is the guilt-inducing backlog §9 exists to prevent."""
    state.set_status(conn, tanker, "active")
    added = review.enrol_from_blessed_objects(conn, per_object=1, max_new=2)
    assert len(added) <= 2


def test_enrolled_items_become_due_and_resolve_to_their_proposition(conn, tanker):
    state.set_status(conn, tanker, "active")
    added = review.enrol_from_blessed_objects(conn, today=date(2026, 1, 1))
    due = review.due_items(conn, today=date(2026, 1, 1), limit=5)
    assert {i.id for i in due} & {i.id for i in added}
    text, _source = review.resolve_prompt(conn, due[0])
    assert text and "no longer in the corpus" not in text


def test_enrolment_skips_prompts_too_thin_to_be_worth_asking(conn, tanker):
    """Live enrolment surfaced 'Python is an interpreted programming language.' — true, and
    useless as a recall question. Short propositions are overwhelmingly status lines or
    definitions of the obvious."""
    state.set_status(conn, tanker, "active")
    conn.execute(
        "INSERT INTO propositions(id, section_id, doc_id, position, text, embed_model) "
        "VALUES (900, 1, 3, 90, 'The project is prioritized.', 'nomic-embed-text')"
    )
    conn.commit()

    for _ in range(10):
        review.enrol_from_blessed_objects(conn)

    refs = {r["prompt_ref"] for r in conn.execute("SELECT prompt_ref FROM review_schedule")}
    assert "900" not in refs
    assert refs, "substantive propositions must still be enrolled"


# ---------- re-read ranking ----------


def test_reread_ranks_by_gaps_closed(conn, tanker):
    from locus.learn import reread

    state.set_status(conn, tanker, "active")
    got = reread.reread_candidates(conn, limit=3)
    assert got, "a blessed object with open gaps must yield a re-read candidate"
    # The AIS paper covers the gap concept; it is not the project's own document.
    assert any("AIS paper" == c.title for c in got)
    assert all("tanker-flow" != c.title for c in got)


def test_reread_never_suggests_his_own_notes(conn, tanker):
    """A gap exists because he has not written it up; handing his notes back is circular."""
    from locus.learn import reread

    state.set_status(conn, tanker, "active")
    titles = {c.title for c in reread.reread_candidates(conn, limit=10)}
    assert "Notes on laden ton-miles" not in titles


def test_reread_is_empty_without_blessed_objects(conn):
    from locus.learn import reread

    assert reread.reread_candidates(conn) == []


def test_reread_reason_names_the_concepts(conn, tanker):
    from locus.learn import reread

    state.set_status(conn, tanker, "active")
    got = reread.reread_candidates(conn, limit=1)
    assert "AIS interpolation" in got[0].reason or got[0].concepts


def test_rarity_outranks_raw_count(conn, tanker):
    """The coursework-dominance fix, in miniature.

    Live on 2026-07-30 the read-next slot offered Nyquist plots and Mechanical Vibration: the
    open gaps included `frequency response` (18 documents) and `eigenvector` (13), which every
    signals handout mentions, so a raw count of gaps closed put generic coursework above the
    one paper that uniquely covered a gap that mattered.
    """
    from locus.learn import reread

    state.set_status(conn, tanker, "active")
    # Two more gap concepts that are EVERYWHERE — the `eigenvector`/`frequency response` shape.
    _entity(conn, 1, "eigenvector")           # the project uses them, so they become gaps...
    _entity(conn, 1, "frequency response")
    # ...and one handout covers BOTH of them: a higher RAW COUNT than the AIS paper's single gap.
    _doc(conn, 10, title="Generic handout", uri="course/generic.pdf", category="coursework")
    _entity(conn, 10, "eigenvector")
    _entity(conn, 10, "frequency response")
    for i in range(11, 25):                   # both generic concepts span many documents
        _doc(conn, i, title=f"handout {i}", uri=f"course/h{i}.pdf", category="coursework")
        _entity(conn, i, "eigenvector")
        _entity(conn, i, "frequency response")

    got = reread.reread_candidates(conn, limit=3)
    assert got, "there are open gaps, so there must be candidates"
    assert got[0].title == "AIS paper", (
        "the paper covering the RARE gap must outrank the handout covering more gaps"
    )
    # ...and the printed reason leads with the concept that singles the document out.
    assert got[0].concepts[0] == "AIS interpolation"


def test_concept_weight_falls_as_a_concept_spreads(conn):
    from locus.learn.reread import concept_weight

    assert concept_weight(1) > concept_weight(13) > concept_weight(18)
    assert concept_weight(0) == concept_weight(1), "an unseen concept is not infinitely valuable"


# --- concept cards: the question and its answer are one object ---------------------------------


def test_the_answer_is_stored_with_the_question_it_answers(conn):
    """THE DEFECT (2026-08-03): page 3 and page 4 were produced by different mechanisms.

    The question was generated here; the answer was re-derived at page-composition time as "the
    longest proposition in any section mentioning the concept". Live, page 3 asked why volatility
    undermines Bollinger-band mean reversion and page 4 replied with a study's input format — and
    two different questions printed the SAME answer, because both concepts sat in one section.
    """
    review.schedule_prompt(conn, prompt_kind="concept", prompt_ref="AIS interpolation",
                           today=date(2026, 1, 1))

    def runner(p, m):
        return ClaudeResult(text=json.dumps({
            "question": "When does resampling irregular fixes distort a derived signal?",
            "answer": "Resampling irregular position fixes imposes an even spacing the data "
                      "never had, so any signal derived from the interpolated leg inherits that "
                      "assumption rather than the movement itself.",
        }))

    assert review.fill_concept_questions(conn, limit=1, runner=runner) == 1
    answer, _src = review.concept_answer(conn, "AIS interpolation")
    assert answer.startswith("Resampling irregular position fixes")

    item = conn.execute(
        "SELECT question, answer FROM review_schedule WHERE prompt_ref='AIS interpolation'"
    ).fetchone()
    assert item["question"].endswith("?")
    assert item["answer"] == answer            # one row, both halves, cannot drift apart


def test_an_ungrounded_answer_falls_back_to_corpus_text_not_to_nothing(conn):
    """An empty answer would leave the row incomplete and re-billed every night forever.

    That is the trap the tension cache had (§26): paying again for the same rejection. The
    fallback is the best-ranked proposition — corpus text, and at least about the right concept.
    """
    review.schedule_prompt(conn, prompt_kind="concept", prompt_ref="AIS interpolation",
                           today=date(2026, 1, 1))

    def runner(p, m):
        return ClaudeResult(text=json.dumps({
            "question": "Why does resampling matter here?",
            # Long enough to clear _MIN_ANSWER_CHARS, so it is ABOUTNESS that rejects this and
            # not length — otherwise the test would pass for the wrong reason.
            "answer": "Quantum chromodynamics governs the strong nuclear interaction between "
                      "quarks and gluons, and the coupling constant runs with energy scale so "
                      "that confinement emerges at low energies and asymptotic freedom at high "
                      "energies, which is why perturbation theory works only in one regime.",
        }))

    assert review.fill_concept_questions(conn, limit=1, runner=runner) == 1
    answer, _ = review.concept_answer(conn, "AIS interpolation")
    assert "chromodynamics" not in answer
    assert "AIS interpolation" in answer        # a stored proposition, not the invention

    # ...and the row is now COMPLETE, so it is not offered for rewriting again.
    pending = review.items_without_questions(conn, kinds=("concept",))
    assert [i.prompt_ref for i in pending] == []


def test_evidence_is_chosen_by_relevance_not_by_length(conn):
    """`ORDER BY LENGTH(text) DESC` is what put a study's input format under a Bollinger question."""
    _prop(conn, 3, "A" * 60 + " this very long sentence never mentions the subject at all and "
                              "exists only to be the longest thing in the section by a margin.")
    facts, _src = review.concept_evidence(conn, "AIS interpolation")
    assert facts, "expected evidence"
    assert "AIS interpolation" in facts[0], f"length won again: {facts[0][:60]!r}"
