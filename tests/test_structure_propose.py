"""The object/belief proposer and — the point of the module — its precision gates.

Model-free: the `claude -p` runner and retrieval are both injected, so these tests assert what
Python does with the model's output, which is exactly where the precision bar lives. A test that
let the model's proposal through unchallenged would be testing nothing.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import pytest

from locus.agent import state
from locus.agent.claude import ClaudeError, ClaudeResult
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.structure import propose
from locus.structure.propose import Plan, PlannedObject, stance_is_grounded


# --- fixtures: a tiny corpus -------------------------------------------------------------------


@dataclass
class _Cite:
    doc_id: int
    rerank_score: float | None
    text: str = "cite"


@dataclass
class _Result:
    citation_details: list
    low_confidence: bool = False


def _doc(conn, doc_id, *, title, uri, category="note", date="2026-05-01", text="", thesis="T"):
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title, "
            "source_date, category, thesis, ingested_at, ingest_model) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (doc_id, f"h{doc_id}", "markdown", uri, f"raw{doc_id}", title, date, category,
             thesis, "2026-05-02T00:00:00Z", "test"),
        )
        conn.execute(
            "INSERT INTO sections (id, doc_id, position, title, summary) VALUES (?,?,0,?,?)",
            (doc_id, doc_id, title, "s"),
        )
        if text:
            conn.execute(
                "INSERT INTO chunks (doc_id, section_id, position, raw_text, embed_model) "
                "VALUES (?,?,0,?,'m')",
                (doc_id, doc_id, text),
            )


def _entity(conn, doc_id, name, type_="concept"):
    with conn:
        conn.execute(
            "INSERT OR IGNORE INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
            (doc_id, doc_id, name, type_),
        )


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "propose.db"
    migrate(db)
    c = get_connection(db)
    # A note (owner-authored) that engages a concept the corpus already carries in two docs.
    _doc(c, 1, title="Brevan Howard notes", uri="notes/brevan.md", category="note",
         date="2026-05-01",
         text="Regime detection only helps if the regimes are stable enough to trade. "
              "I now think the HMM approach overfits and changepoint detection is the honest baseline.")
    _doc(c, 2, title="Regime paper", uri="papers/regime.pdf", category="paper", date="2024-01-01",
         text="Markov switching models for asset returns.")
    _doc(c, 3, title="regime-ml", uri="repos/regime-ml", category="project", date="2026-02-01",
         text="HMM regime detection over equity returns.")
    for d in (1, 2, 3):
        _entity(c, d, "regime detection")
    _entity(c, 1, "solo idea")  # only one doc -> must not be proposable as a concept
    yield c
    c.close()


def _runner(payload: dict):
    """A fake CliRunner returning one canned proposal set."""
    return lambda prompt, model: ClaudeResult(text=json.dumps(payload), cost_usd=0.001)


def _no_support(_query):
    return _Result(citation_details=[])


# --- gate 1: concepts must be canonical entities spanning >= min_concept_docs -------------------


def test_concept_spanning_two_docs_is_accepted(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [
            {"type": "concept", "title": "Regime Detection", "anchor": "regime detection",
             "mastery": "working", "why": "the note argues about it"}
        ]}),
    )
    assert [o.type for o in plan.objects] == ["concept"]
    # Titled by the CANONICAL name, not the model's capitalisation — so both notes land on one object.
    assert plan.objects[0].title == "regime detection"
    assert any(link.target_kind == "entity" for link in plan.objects[0].links)


def test_concept_in_only_one_document_is_rejected(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [
            {"type": "concept", "title": "solo idea", "anchor": "solo idea"}
        ]}),
    )
    assert plan.objects == []
    assert "canonical entity" in plan.rejected[0].reason


def test_invented_concept_is_rejected(conn):
    """The model cannot conjure a concept the corpus does not carry — gate 1 is a whitelist."""
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [
            {"type": "concept", "title": "stochastic volatility", "anchor": "stochastic volatility"}
        ]}),
    )
    assert plan.objects == []


def test_canonical_concepts_uses_the_alias_substrate(conn):
    """A surface variant counts toward the same canonical, so aliasing raises the doc span."""
    _doc(conn, 4, title="Notes on regimes", uri="notes/r2.md", text="regimes")
    _entity(conn, 4, "regime-detection")
    with conn:
        for variant in ("regime detection", "regime-detection"):
            conn.execute(
                "INSERT INTO entity_aliases (variant_name, variant_type, canonical_name, "
                "canonical_type, cluster_id, tier) VALUES (?,?,?,?,1,'punct')",
                (variant, "concept", "regime detection", "concept"),
            )
    concepts = propose.canonical_concepts(conn, 4, min_docs=4)
    assert "regime detection" in concepts  # 4 docs once the variant folds in


# --- gate 2: projects/readings must anchor to a real document ----------------------------------


def test_project_anchor_resolves_to_a_real_repo(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [
            {"type": "project", "title": "regime-ml", "anchor": "repos/regime-ml",
             "approach": "HMM", "open_threads": ["compare to changepoint"]}
        ]}),
    )
    assert len(plan.objects) == 1
    link = plan.objects[0].links[0]
    assert (link.target_kind, link.target_key, link.relation) == ("doc", "repos/regime-ml", "implements")
    assert plan.objects[0].body["open_threads"] == ["compare to changepoint"]


def test_unresolvable_anchor_is_rejected_not_fuzzed(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [
            {"type": "reading", "title": "Some book", "anchor": "books/never-ingested.pdf"}
        ]}),
    )
    assert plan.objects == []
    assert "does not resolve" in plan.rejected[0].reason


def test_ambiguous_anchor_substring_resolves_to_nothing(conn):
    _doc(conn, 5, title="regime-ml docs", uri="repos/regime-ml-docs", category="project")
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [{"type": "project", "title": "P", "anchor": "regime-ml"}]}),
    )
    assert plan.objects == []  # two documents match the fragment -> not grounding


# --- gates 3 & 4: grounding links and the per-document cap --------------------------------------


def test_support_links_only_come_from_floor_clearing_citations(conn):
    def retrieve_fn(_q):
        return _Result(citation_details=[_Cite(2, 5.0), _Cite(3, -9.0)])  # floor is 0.22

    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=retrieve_fn,
        runner=_runner({"objects": [
            {"type": "concept", "title": "regime detection", "anchor": "regime detection"}
        ]}),
    )
    keys = {link.target_key for link in plan.objects[0].links}
    assert "papers/regime.pdf" in keys      # +5.0 clears the floor
    assert "repos/regime-ml" not in keys    # -9.0 does not


def test_low_confidence_retrieval_contributes_no_links(conn):
    def retrieve_fn(_q):
        return _Result(citation_details=[_Cite(2, 9.0)], low_confidence=True)

    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=retrieve_fn,
        runner=_runner({"objects": [
            {"type": "concept", "title": "regime detection", "anchor": "regime detection"}
        ]}),
    )
    assert {link.target_key for link in plan.objects[0].links} == {
        "regime detection\x1fconcept", "notes/brevan.md"
    }


def test_every_object_carries_at_least_one_link(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [{"type": "question", "title": "Do regimes persist?"}]}),
    )
    assert plan.objects[0].links[0].relation == "raised_by"
    assert all(o.links for o in plan.objects)


def test_per_document_cap_truncates_the_tail(conn):
    objs = [
        {"type": "question", "title": f"Q{i}"} for i in range(6)
    ]
    plan = propose.plan_for_document(conn, 1, retrieve_fn=_no_support,
                                     runner=_runner({"objects": objs}))
    assert len(plan.objects) == 3  # default max_objects_per_doc
    assert [o.title for o in plan.objects] == ["Q0", "Q1", "Q2"]  # model's own ranking kept
    assert sum(r.reason == "over per-document cap" for r in plan.rejected) == 3


# --- gate 5: belief positions -------------------------------------------------------------------


def test_position_from_an_owner_note_is_recorded_with_the_source_date(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [], "positions": [
            {"subject_kind": "concept", "subject": "regime detection",
             "stance": "the HMM approach overfits and changepoint detection is the honest baseline"}
        ]}),
    )
    assert len(plan.positions) == 1
    assert plan.positions[0].dated_at == "2026-05-01"  # the SOURCE date, not today
    res = propose.apply_plan(conn, plan)
    assert res.positions == 1
    chain = state.positions_for(conn, "concept", state.entity_key("regime detection", "concept"))
    assert "overfits" in chain[0].stance


def test_position_from_a_paper_is_rejected(conn):
    """A paper's claim is the PAPER's position. Attributing it to the owner corrupts §3.4."""
    plan = propose.plan_for_document(
        conn, 2, retrieve_fn=_no_support,
        runner=_runner({"objects": [], "positions": [
            {"subject_kind": "concept", "subject": "regime detection",
             "stance": "Markov switching models capture asset return regimes"}
        ]}),
    )
    assert plan.positions == []
    assert "not owner-authored" in plan.rejected[0].reason


def test_stance_not_in_the_owners_words_is_rejected(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [], "positions": [
            {"subject_kind": "concept", "subject": "regime detection",
             "stance": "diversification across uncorrelated carry strategies dominates"}
        ]}),
    )
    assert plan.positions == []
    assert "owner's words" in plan.rejected[0].reason


def test_stance_grounding_accepts_a_trimmed_quote_and_rejects_a_rewrite():
    source = ("I now think the HMM approach overfits and changepoint detection is the honest "
              "baseline for regime work.")
    assert stance_is_grounded("the HMM approach overfits; changepoint detection is the baseline", source)
    assert not stance_is_grounded("neural sequence models generalise better across market cycles", source)
    assert not stance_is_grounded("it is fine", source)  # nothing distinctive to attribute


def test_position_on_an_unproposed_project_is_rejected(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [], "positions": [
            {"subject_kind": "project", "subject": "regime-ml",
             "stance": "the HMM approach overfits and changepoint detection is the honest baseline"}
        ]}),
    )
    assert plan.positions == []
    assert "no project object proposed" in plan.rejected[0].reason


def test_project_position_binds_to_the_object_created_in_the_same_run(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({
            "objects": [{"type": "project", "title": "regime-ml", "anchor": "repos/regime-ml"}],
            "positions": [{"subject_kind": "project", "subject": "regime-ml",
                           "stance": "the HMM approach overfits and changepoint detection is the honest baseline"}],
        }),
    )
    res = propose.apply_plan(conn, plan)
    assert res.positions == 1
    oid = res.created[0]
    assert state.positions_for(conn, "project", str(oid))[0].stance.startswith("the HMM")


# --- invariants: generated sources, degradation, dry run ----------------------------------------


def test_agent_generated_documents_are_never_structured(conn):
    """Invariant 5 — no feedback loop: the structurer must not read the agent's own output."""
    _doc(conn, 9, title="Surfaced connections", uri="_generated/connections.md", category="note",
         text="anything")
    plan = propose.plan_for_document(
        conn, 9, retrieve_fn=_no_support,
        runner=_runner({"objects": [{"type": "question", "title": "Q"}]}),
    )
    assert plan.objects == [] and plan.rejected[0].reason == "agent-generated source"


def test_a_promoted_thread_is_never_structured_back_into_a_second_object(conn):
    """The other feedback loop, and the one that actually fired in production.

    `locus promote` writes a thread to `vault/notes/threads/` as HIS words, so the
    `_is_generated` guard correctly does not catch it — but the object already exists. Live
    before this guard: obj 79 was answered and ARCHIVED, promoted, ingested as doc 493, and
    re-proposed as obj 85 `active`. The resolved question came back as an open one.
    """
    _doc(conn, 11, title="Is leetcode that important?",
         uri="/home/alec/vault/notes/threads/is-leetcode-that-important-79.md",
         category="note", text="Is leetcode that important or should we focus on codeforces?")
    plan = propose.plan_for_document(
        conn, 11, retrieve_fn=_no_support,
        runner=_runner({"objects": [{"type": "question", "title": "Is leetcode that important?"}]}),
    )
    assert plan.objects == []
    assert plan.rejected[0].reason == "already an object — promoted thread"


def test_model_failure_degrades_and_proposes_nothing(conn):
    def failing(prompt, model):
        raise ClaudeError("nope")

    plan = propose.plan_for_document(conn, 1, retrieve_fn=_no_support, runner=failing)
    assert plan.degraded is True and plan.objects == [] and plan.positions == []


def test_dry_run_writes_nothing(conn):
    runner = _runner({"objects": [
        {"type": "concept", "title": "regime detection", "anchor": "regime detection"}
    ]})
    out = propose.structure_documents(conn, [1], runner=runner, retrieve_fn=_no_support, dry_run=True)
    assert out.created == 1  # would create
    assert conn.execute("SELECT COUNT(*) c FROM objects").fetchone()["c"] == 0
    assert conn.execute("SELECT COUNT(*) c FROM agent_runs").fetchone()["c"] == 0


def test_batch_run_is_journaled_and_survives_one_bad_document(conn):
    def runner(prompt, model):
        # Keyed on the document, not the call count: `run_structured` retries, so a
        # fail-once fake would just succeed on attempt 2 (which is the runner working).
        if "Brevan Howard notes" in prompt:
            raise ClaudeError("transient")
        return ClaudeResult(text=json.dumps({"objects": [
            {"type": "concept", "title": "regime detection", "anchor": "regime detection"}
        ]}), cost_usd=0.002)

    out = propose.structure_documents(conn, [1, 3], runner=runner, retrieve_fn=_no_support)
    assert out.documents == 2
    assert out.degraded == 1          # doc 1
    assert out.created == 1           # doc 3 still proposed
    row = conn.execute("SELECT kind, status, stats FROM agent_runs").fetchone()
    assert row["kind"] == "structure" and row["status"] == "ok"
    assert json.loads(row["stats"])["documents"] == 2


def test_reproposing_the_same_concept_updates_one_object(conn):
    runner = _runner({"objects": [
        {"type": "concept", "title": "regime detection", "anchor": "regime detection",
         "mastery": "thin"}
    ]})
    propose.structure_documents(conn, [1], runner=runner, retrieve_fn=_no_support)
    propose.structure_documents(conn, [3], runner=runner, retrieve_fn=_no_support)
    assert conn.execute("SELECT COUNT(*) c FROM objects").fetchone()["c"] == 1


def test_apply_plan_is_the_only_writer(conn):
    """A hand-built plan writes exactly what it says — no hidden model call, no extra objects."""
    from locus.agent.state import ObjectLink

    plan = Plan(doc_id=1, objects=[PlannedObject(
        type="question", title="Do regimes persist?", body={"why": "raised in the note"},
        links=[ObjectLink("doc", "notes/brevan.md", "raised_by")],
    )])
    res = propose.apply_plan(conn, plan, run_id=None)
    assert len(res.created) == 1
    obj = state.get_object(conn, res.created[0])
    assert obj.type == "question" and obj.status == "proposed"
    assert obj.links[0].relation == "raised_by"


# --- tolerant parsing (shapes a real Haiku run produced, 2026-07-28) ----------------------------


def test_kind_alias_and_scalar_lists_are_absorbed(conn):
    """A live run emitted 'kind' for 'type' and prose where a list belongs. Tolerant parse,
    unchanged gates — the object still has to clear gate 2."""
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [
            {"kind": "project", "title": "regime-ml", "anchor": "repos/regime-ml",
             "open_threads": "compare to changepoint", "learnings": "HMM overfits"}
        ]}),
    )
    assert len(plan.objects) == 1
    assert plan.objects[0].body["open_threads"] == ["compare to changepoint"]
    assert plan.objects[0].body["learnings"] == ["HMM overfits"]


def test_a_titleless_candidate_is_named_by_its_anchor(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [
            {"type": "concept", "anchor": "regime detection", "mastery": "working"}
        ]}),
    )
    assert [o.title for o in plan.objects] == ["regime detection"]


def test_a_titleless_candidate_with_an_unresolvable_anchor_is_still_rejected(conn):
    """The fallback must not become a way past the gates."""
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [{"type": "project", "anchor": "repos/does-not-exist"}]}),
    )
    assert plan.objects == []


def test_position_aliases_are_absorbed(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [], "positions": [
            {"subject": "regime detection",
             "position": "the HMM approach overfits and changepoint detection is the honest baseline"}
        ]}),
    )
    assert len(plan.positions) == 1  # 'position' read as 'stance', subject_kind defaulted


def test_an_empty_stance_is_not_a_position(conn):
    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [], "positions": [
            {"subject_kind": "concept", "subject": "regime detection", "stance": ""}
        ]}),
    )
    assert plan.positions == []


def test_generic_code_identifiers_are_not_proposable_concepts(conn):
    """Gate 1b: `state`/`ingest` clear the cross-document bar trivially (every repo has them).
    Reuses link/related.non_topical_names so the two surfaces agree on what a concept is."""
    _doc(conn, 6, title="repo A", uri="repos/a", category="project", text="x")
    _doc(conn, 7, title="repo B", uri="repos/b", category="project", text="y")
    for d in (1, 6, 7):
        _entity(conn, d, "state")
    with conn:
        conn.execute(
            "INSERT INTO entity_aliases (variant_name, variant_type, canonical_name, "
            "canonical_type, cluster_id, tier) VALUES ('state','concept','state','concept',9,'identity')"
        )
    concepts = propose.canonical_concepts(conn, 1, min_docs=2)
    assert "state" not in concepts  # short bare lowercase identifier

    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [{"type": "concept", "title": "state", "anchor": "state"}]}),
    )
    assert plan.objects == []


def test_tool_and_person_entities_cannot_become_concept_objects(conn):
    """Gate 1c: a Concept is an IDEA. The first live run proposed `Python` (tool) as one."""
    _doc(conn, 8, title="repo C", uri="repos/c", category="project", text="z")
    for d in (1, 8):
        _entity(conn, d, "Python", "tool")
        _entity(conn, d, "Ken Griffin", "author")
        _entity(conn, d, "mean reversion", "concept")

    concepts = propose.canonical_concepts(
        conn, 1, min_docs=2, exclude_types=["tool", "author", "organization", "ticker", "other"]
    )
    assert "python" not in concepts and "ken griffin" not in concepts
    assert "mean reversion" in concepts  # a real idea is untouched

    plan = propose.plan_for_document(
        conn, 1, retrieve_fn=_no_support,
        runner=_runner({"objects": [{"type": "concept", "title": "Python", "anchor": "Python"}]}),
    )
    assert plan.objects == []


def test_excluding_no_types_keeps_everything(conn):
    """The filter is config-driven; an empty exclusion list is a no-op."""
    _doc(conn, 8, title="repo C", uri="repos/c", category="project", text="z")
    for d in (1, 8):
        _entity(conn, d, "Python", "tool")
    assert "python" in propose.canonical_concepts(conn, 1, min_docs=2, exclude_types=[])
