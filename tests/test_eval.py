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
    # QC: the seeded doc has no synthesis columns and clean props/entities.
    assert m.empty_synthesis is True
    assert m.suspect_props == {}
    assert m.noise_entities == 0


def test_retrieval_scoring_is_pure_and_correct():
    from locus.eval.retrieval_eval import LabelledQuery, aggregate, score_query

    q = LabelledQuery("bridge query", ["Control Theory", "Regime Shift"], cross_domain=True)
    r = score_query(q, [
        "Macro-aware forecasting", "A2 Introduction to Control Theory: Lectures 5-8",
        "Enhancing Regime Shift Detection", "A2 Introduction to Control Theory: Lectures 5-8",
    ])
    assert r.recall == 1.0
    assert r.first_rank == 2  # best-ranked expected doc (titles dedupe first)
    assert r.reciprocal_rank == 0.5

    miss = score_query(LabelledQuery("q", ["Nonexistent Doc"]), ["Some Doc"])
    assert miss.recall == 0.0 and miss.reciprocal_rank == 0.0

    agg = aggregate([r, miss])
    assert agg["recall_at_k"] == 0.5
    assert agg["cross_domain_recall"] == 1.0  # only the cross-domain query counts


def test_corruption_signature_predicate():
    from locus.eval.metrics import has_corruption_signature

    # The JSON-escape corruption residues: TAB+`au` (\tau), FF+`rac` (\frac), BS+`eta` (\beta).
    assert has_corruption_signature("decays with \tau time constant")
    assert has_corruption_signature("the term \x0crac{K}{s-2}")
    assert has_corruption_signature("\x08eta-convergence")
    # Clean text — including legitimate newlines and intact LaTeX — does not flag.
    assert not has_corruption_signature("a clean summary")
    assert not has_corruption_signature("line one\nline two")
    assert not has_corruption_signature(r"intact \tau and \frac{K}{s-2}")
    assert not has_corruption_signature(None)
    assert not has_corruption_signature("")


def test_doc_metrics_counts_corrupted_fields(conn):
    # Baseline: the seeded doc is clean.
    assert doc_metrics(conn, 1).corrupted_fields == 0
    conn.execute("UPDATE sections SET summary='decays with \tau' WHERE id=1")
    conn.execute(
        "INSERT INTO propositions (id, section_id, doc_id, position, text, embed_model)"
        " VALUES (10,1,1,9,'the term \x0crac{K}{s-2} appears','nomic')"
    )
    conn.execute("UPDATE documents SET thesis='uses \x08eta decay' WHERE id=1")
    conn.commit()
    assert doc_metrics(conn, 1).corrupted_fields == 3


def test_zero_prop_sections_are_named(conn):
    from locus.eval.metrics import format_metrics

    conn.execute(
        "INSERT INTO sections (id, doc_id, position, title, summary) "
        "VALUES (2,1,1,'Methodology','the core method section')"
    )
    conn.commit()  # section 2 has no propositions
    m = doc_metrics(conn, 1)
    assert m.empty_prop_sections == 1
    assert m.empty_prop_section_titles == ["Methodology"]
    assert "zero-prop sections: Methodology" in format_metrics([m])


def test_semantic_gaps_exclude_audit_trail_lines(conn):
    from locus.eval.metrics import format_metrics, semantic_gaps

    flags = [
        "math-OCR kept original text on page 3",
        "propositions pass failed for section 2 (Methods)",
        "Root locus design is mentioned but not covered.",
    ]
    assert semantic_gaps(flags) == ["Root locus design is mentioned but not covered."]

    import json as _json

    conn.execute("UPDATE documents SET gap_flags=? WHERE id=1", (_json.dumps(flags),))
    conn.commit()
    m = doc_metrics(conn, 1)
    assert m.semantic_gaps == 1
    # Corpus-wide liveness: with a live gap, no warning; with only audit lines, warn.
    out = format_metrics([m, m])
    assert "gap liveness: 2/2" in out and "WARNING" not in out
    m.semantic_gaps = 0
    out = format_metrics([m, m])
    assert "gap liveness: 0/2" in out and "WARNING: zero semantic gaps" in out


def test_doc_metrics_qc_flags_suspect_rows(conn):
    conn.execute(
        "INSERT INTO propositions (id, section_id, doc_id, position, text, embed_model)"
        " VALUES (3,1,1,2,'Kalman filtering is discussed','nomic')"
    )
    conn.execute(
        "INSERT INTO propositions (id, section_id, doc_id, position, text, embed_model)"
        " VALUES (4,1,1,3,'The state estimate is given by .','nomic')"
    )
    conn.execute("INSERT INTO entities (doc_id, section_id, name, type) VALUES (1,1,'equation 1.36','other')")
    conn.commit()
    m = doc_metrics(conn, 1)
    assert m.suspect_props == {"meta": 1, "dropped-formula": 1}
    assert m.noise_entities == 1


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
    assert agg["sections_judged"] == 1.0
    assert agg["extraction_failures"] == 0.0


def test_sample_excludes_code_sections_by_default(conn):
    # A code doc (props skipped by design) must not enter the prose-pass benchmark sample.
    conn.execute(
        "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, ingest_model)"
        " VALUES (9, 'h9', 'code', 'u9', 'p9', 'test')"
    )
    conn.execute("INSERT INTO sections (id, doc_id, position, title) VALUES (9, 9, 0, 'a.py')")
    conn.commit()
    assert 9 not in harness.sample_section_ids(conn, 100, seed=0)
    assert 9 in harness.sample_section_ids(conn, 100, seed=0, prose_only=False)


def test_evaluate_counts_extraction_failures_instead_of_crashing(conn, monkeypatch):
    from locus.ingest.llm import IngestExtractionError

    def boom(conn_, sid, model):
        raise IngestExtractionError("no schema-valid output")
    monkeypatch.setattr(harness, "regenerate", boom)
    monkeypatch.setattr(
        harness, "judge_section",
        lambda *a, **k: pytest.fail("nothing to judge when extraction fails"),
    )

    judged, agg = harness.evaluate(conn, sample=5, seed=0, model="some-model")
    assert judged == []
    assert agg["extraction_failures"] == 1.0
    assert agg["sections_judged"] == 0.0


def test_retrieval_scoring_file_paths_and_banner():
    from locus.eval.retrieval_eval import LabelledQuery, aggregate, score_query

    # File-level targets: doc title alone is not enough — the source file must surface.
    q = LabelledQuery(
        "show me the HMM detector class", ["Regime-Conditioned"],
        expected_paths=["regimes/hmm.py"],
    )
    half = score_query(q, ["Regime-Conditioned Equity ML"], ["docs/design.md"])
    assert half.recall == 0.5 and half.matched_paths == []
    full = score_query(
        q, ["Regime-Conditioned Equity ML"], ["src/regime_ml/regimes/hmm.py"]
    )
    assert full.recall == 1.0 and full.matched_paths == ["regimes/hmm.py"]

    # Cross-domain banner misfire: right docs retrieved, user warned anyway.
    xq = LabelledQuery("bridge", ["A", "B"], cross_domain=True)
    fired = score_query(xq, ["A doc", "B doc"], confidence_band="ambiguous")
    clean = score_query(xq, ["A doc", "B doc"], confidence_band=None)
    assert fired.banner_misfire and not clean.banner_misfire

    agg = aggregate([half, full, fired, clean])
    assert agg["file_recall"] == 0.5
    assert agg["cross_domain_banner_rate"] == 0.5
