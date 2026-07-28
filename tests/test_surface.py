"""The critique / synthesis surface (plan §3.5, §8.4).

The assertion that carries the module: a claim citing an evidence key it was never given is
DROPPED. Without that, a critique is indistinguishable from a language model's opinion at the
point of reading, which is exactly how trust erodes (failure mode #2).
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import pytest

from locus.agent import state
from locus.agent.claude import ClaudeError, ClaudeResult
from locus.agent.state import ObjectLink
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.surface import grounding
from locus.surface.critique import critique
from locus.surface.synthesise import synthesise


@dataclass
class _Cite:
    """Stands in for a retrieval SURVIVOR (a Candidate): `text` is the unit's real content.

    Deliberately not a Citation — a Citation's `.text` is the provenance string, and the
    surfaces must read content, not a bibliography (a live run failed exactly this way)."""

    doc_id: int
    rerank_score: float | None
    text: str


@dataclass
class _Result:
    survivors: list
    low_confidence: bool = False
    citation_details: list = None  # type: ignore[assignment]

    def __post_init__(self):
        # Provenance strings, as the real pipeline builds them — never the unit content.
        self.citation_details = [
            _Cite(c.doc_id, c.rerank_score, f"doc {c.doc_id}, provenance") for c in self.survivors
        ]


def _doc(conn, doc_id, *, title, uri, category="note"):
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title, "
            "category, ingested_at, ingest_model) VALUES (?,?,?,?,?,?,?,?,?)",
            (doc_id, f"h{doc_id}", "markdown", uri, f"r{doc_id}", title, category,
             "2026-01-01T00:00:00Z", "test"),
        )
        conn.execute(
            "INSERT INTO sections (id, doc_id, position, title, summary) VALUES (?,?,0,?,'s')",
            (doc_id, doc_id, title),
        )


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "surface.db"
    migrate(db)
    c = get_connection(db)
    _doc(c, 1, title="regime-ml", uri="repos/regime-ml", category="project")
    _doc(c, 2, title="Brevan Howard notes", uri="notes/brevan.md", category="note")
    oid, _ = state.upsert_object(
        c, type_="project", title="regime-ml",
        body={"approach": "HMM over equity returns", "open_threads": ["validate out of sample"]},
    )
    state.add_links(c, oid, [ObjectLink("doc", "repos/regime-ml", "implements")])
    state.record_position(c, subject_kind="project", subject_key=str(oid),
                          stance="the HMM overfits; changepoint detection is the honest baseline",
                          dated_at="2026-05-01", source_doc_id=2)
    yield c
    c.close()


def _retrieve(*cites, low=False):
    def fn(_query):
        return _Result(survivors=list(cites), low_confidence=low)
    return fn


def _runner(payload):
    return lambda prompt, model: ClaudeResult(text=json.dumps(payload), cost_usd=0.001)


# --- grounding -----------------------------------------------------------------------------------


def test_grounding_gathers_evidence_objects_and_trajectory(conn):
    g = grounding.ground_topic(
        conn, "regime-ml", retrieve_fn=_retrieve(_Cite(2, 4.0, "HMM regimes are unstable")),
    )
    assert [e.text for e in g.evidence] == ["HMM regimes are unstable"]
    assert g.evidence[0].source == "Brevan Howard notes"
    assert [o.title for o in g.objects] == ["regime-ml"]
    assert [t.label for t in g.trajectories] == ["regime-ml"]
    rendered = g.render()
    assert "open thread: validate out of sample" in rendered
    assert "YOUR POSITIONS ON REGIME-ML" in rendered


def test_sub_floor_citations_are_not_evidence(conn):
    g = grounding.ground_topic(
        conn, "regime-ml",
        retrieve_fn=_retrieve(_Cite(2, 4.0, "kept"), _Cite(2, -8.0, "noise")),
    )
    assert [e.text for e in g.evidence] == ["kept"]


def test_low_confidence_is_reported_not_hidden(conn):
    g = grounding.ground_topic(
        conn, "quantum tunnelling", retrieve_fn=_retrieve(low=True),
    )
    assert g.low_confidence is True


def test_object_matching_is_literal_not_fuzzy(conn):
    """A confidently-answered question about the WRONG project is worse than a miss."""
    g = grounding.ground_topic(conn, "tanker flow ton-miles", retrieve_fn=_retrieve())
    assert g.objects == []


# --- critique ------------------------------------------------------------------------------------


def test_challenge_citing_real_evidence_survives(conn):
    result = critique(
        conn, "regime-ml", retrieve_fn=_retrieve(_Cite(2, 4.0, "HMM regimes are unstable")),
        runner=_runner({"strengths": ["clear approach"], "challenges": [
            {"point": "you rejected the HMM in May, but the approach is still HMM",
             "citation_key": "S1"}
        ]}),
    )
    assert len(result.challenges) == 1
    assert result.challenges[0].citation_text == "HMM regimes are unstable"
    assert result.challenges[0].source == "Brevan Howard notes"
    assert result.dropped == 0
    assert result.strengths == ["clear approach"]


def test_challenge_citing_an_invented_key_is_dropped(conn):
    result = critique(
        conn, "regime-ml", retrieve_fn=_retrieve(_Cite(2, 4.0, "HMM regimes are unstable")),
        runner=_runner({"challenges": [
            {"point": "have you considered overfitting?", "citation_key": "S99"}
        ]}),
    )
    assert result.challenges == [] and result.dropped == 1
    assert "No challenge could be grounded" in result.render()


def test_critique_surfaces_recorded_open_threads_and_gaps(conn):
    result = critique(
        conn, "regime-ml", retrieve_fn=_retrieve(), runner=_runner({"challenges": []}),
    )
    assert result.open_threads == ["validate out of sample"]


def test_critique_degrades_to_the_deterministic_half(conn):
    def failing(prompt, model):
        raise ClaudeError("nope")

    result = critique(conn, "regime-ml", retrieve_fn=_retrieve(), runner=failing)
    assert result.degraded is True
    assert result.challenges == []
    assert result.open_threads == ["validate out of sample"]  # still worth having


def test_critique_can_be_centred_on_an_object(conn):
    oid = state.list_objects(conn, type_="project")[0].id
    result = critique(
        conn, "is this approach sound?", object_id=oid, retrieve_fn=_retrieve(),
        runner=_runner({"challenges": []}),
    )
    assert result.open_threads == ["validate out of sample"]


# --- synthesise ----------------------------------------------------------------------------------


def test_synthesis_includes_the_dated_trajectory(conn):
    result = synthesise(
        conn, "regime-ml", retrieve_fn=_retrieve(_Cite(2, 4.0, "HMM regimes are unstable")),
        runner=_runner({"summary": "You built an HMM and then doubted it.", "points": [
            {"text": "your own notes say the regimes are unstable", "citation_key": "S1"}
        ]}),
    )
    assert result.summary.startswith("You built an HMM")
    assert len(result.points) == 1
    assert "2026-05-01" in result.trajectory_md
    rendered = result.render()
    assert "## How your view has moved" in rendered
    assert "changepoint detection is the honest baseline" in rendered


def test_synthesis_point_with_a_bad_citation_is_dropped(conn):
    result = synthesise(
        conn, "regime-ml", retrieve_fn=_retrieve(_Cite(2, 4.0, "real")),
        runner=_runner({"summary": "s", "points": [
            {"text": "regime models are a standard technique", "citation_key": "S7"}
        ]}),
    )
    assert result.points == [] and result.dropped == 1


def test_synthesis_reports_low_confidence_rather_than_filling_the_gap(conn):
    result = synthesise(
        conn, "quantum tunnelling", retrieve_fn=_retrieve(low=True),
        runner=_runner({"summary": "You have very little on this.", "points": []}),
    )
    assert result.low_confidence is True
    assert "LOW CONFIDENCE" in result.render()


def test_synthesis_degrades_on_model_failure(conn):
    def failing(prompt, model):
        raise ClaudeError("nope")

    result = synthesise(conn, "regime-ml", retrieve_fn=_retrieve(), runner=failing)
    assert result.degraded is True and result.summary == ""
    assert "2026-05-01" in result.trajectory_md  # the deterministic half survives


# --- MCP wiring -----------------------------------------------------------------------------------


def test_new_tools_are_advertised_and_none_of_them_bills_the_api_key():
    from locus import mcp_server

    default_tools = {t.name for t in mcp_server._build()._tool_manager.list_tools()}
    for expected in ("critique", "synthesise", "objects", "evolution"):
        assert expected in default_tools
    # The cost guard is unchanged: `query` (metered) is still the only opt-in tool.
    enabled = {t.name for t in mcp_server._build(enable_query=True)._tool_manager.list_tools()}
    assert enabled - default_tools == {"query"}
