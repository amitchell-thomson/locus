"""Tests for the eval harness: structural metrics, judge parsing, and orchestration.

Metrics + harness run on a seeded in-memory-ish DB (no API). The judge is tested with a fake
Anthropic client so parsing is covered without the network.
"""

from pathlib import Path
from types import SimpleNamespace

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.eval import harness
from locus.eval.judge import SectionScores, judge_section
from locus.eval.metrics import doc_metrics


def _seed(conn):
    conn.execute(
        "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, ingest_model)"
        " VALUES (1,'h','pdf','u','p','qwen2.5:7b')"
    )
    conn.execute("INSERT INTO sections (id, doc_id, position, title, summary) VALUES (1,1,0,'Sec','a summary')")
    conn.execute(
        "INSERT INTO chunks (id, section_id, doc_id, position, raw_text, embed_model)"
        " VALUES (1,1,1,0,'The Kalman filter estimates state. LTI systems are linear.','nomic')"
    )
    # one self-contained proposition, one starting with a pronoun
    conn.execute("INSERT INTO propositions (id, section_id, doc_id, position, text, embed_model) VALUES (1,1,1,0,'The Kalman filter estimates state.','nomic')")
    conn.execute("INSERT INTO propositions (id, section_id, doc_id, position, text, embed_model) VALUES (2,1,1,1,'It estimates the state.','nomic')")
    # grounded, redundant (substring), and ungrounded entities
    conn.execute("INSERT INTO entities (doc_id, section_id, name, type) VALUES (1,1,'Kalman filter','method')")
    conn.execute("INSERT INTO entities (doc_id, section_id, name, type) VALUES (1,1,'Kalman','method')")
    conn.execute("INSERT INTO entities (doc_id, section_id, name, type) VALUES (1,1,'Fourier transform','concept')")
    conn.commit()


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "eval.db"
    migrate(db)
    c = get_connection(db)
    _seed(c)
    yield c
    c.close()


# --- structural metrics ------------------------------------------------------------------


def test_doc_metrics_counts_and_flags(conn):
    m = doc_metrics(conn, 1)
    assert m.sections == 1
    assert m.propositions == 2
    assert m.entities == 3
    assert m.non_self_contained_props == 1  # "It estimates the state."
    assert m.ungrounded_entities == 1       # "Fourier transform" not in source
    assert m.redundant_entity_pairs == 1    # "Kalman" inside "Kalman filter"
    assert m.entity_type_counts.get("method") == 2


# --- judge parsing -----------------------------------------------------------------------


class _FakeMessages:
    def __init__(self, payload):
        self.payload = payload

    def create(self, **_kw):
        return SimpleNamespace(content=[SimpleNamespace(type="tool_use", input=self.payload)])


class _FakeAnthropic:
    def __init__(self, payload):
        self.messages = _FakeMessages(payload)


def test_judge_parses_tool_use():
    payload = {
        "summary_faithfulness": 5, "proposition_faithfulness": 4,
        "proposition_atomicity": 3, "proposition_self_containment": 4,
        "entity_recall": 4, "entity_precision": 5, "notes": "ok",
    }
    scores = judge_section("src", "sum", ["p"], [("E", "concept")], client=_FakeAnthropic(payload))
    assert isinstance(scores, SectionScores)
    assert scores.summary_faithfulness == 5
    assert abs(scores.mean() - (5 + 4 + 3 + 4 + 4 + 5) / 6) < 1e-9


def test_judge_raises_without_tool_use():
    class NoTool:
        messages = SimpleNamespace(create=lambda **k: SimpleNamespace(content=[SimpleNamespace(type="text")]))

    with pytest.raises(RuntimeError):
        judge_section("s", "s", [], [], client=NoTool())


# --- harness -----------------------------------------------------------------------------


def test_sample_section_ids_is_deterministic(conn):
    a = harness.sample_section_ids(conn, 1, seed=0)
    b = harness.sample_section_ids(conn, 1, seed=0)
    assert a == b
    assert len(a) == 1


def test_evaluate_uses_existing_outputs_and_aggregates(conn, monkeypatch):
    fixed = SectionScores(
        summary_faithfulness=4, proposition_faithfulness=4, proposition_atomicity=4,
        proposition_self_containment=4, entity_recall=4, entity_precision=4,
    )
    monkeypatch.setattr(harness, "judge_section", lambda *a, **k: fixed)

    judged, agg = harness.evaluate(conn, sample=5, seed=0)
    assert len(judged) == 1  # only one section seeded
    assert agg["overall_mean"] == 4.0
    assert agg["entity_recall"] == 4.0
