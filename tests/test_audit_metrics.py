"""Structural ingest metrics (`locus audit`, `locus inspect`), on a seeded DB, no API.

The judge, math and retrieval suites (`locus eval`) were deleted in Slice C (C2).
"""

from pathlib import Path
from types import SimpleNamespace

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
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


# --- harness -----------------------------------------------------------------------------


def test_unattested_numbers_predicate():
    from locus.eval.metrics import unattested_numbers

    # The verified corpus instance: a degraded math page reading "K\n6 . 77" produced
    # "equals K/(6.77)" (true value K/6) — the number must flag.
    src = "the magnitude |L(jwG)| =\nK\n6 . 77\nThe gain margin is then GM = 6/K"
    assert unattested_numbers("The magnitude equals K / (6.77).", src) == ["6.77"]

    # Faithful conversions must NOT flag (the two false positives the first sweep hit):
    assert unattested_numbers(
        "NAIRU is estimated around 4.75%.", "UE at 4.9%, NAIRU estimated ~4¾%"
    ) == []
    assert unattested_numbers(
        "Expectations anchored at 3.7% since November 2025.",
        "BoE/Ipsos 5y expectations stuck at 3.7% since Nov-25",
    ) == []

    # Plainly attested numbers, formatting variants, and trivial ordinals pass.
    assert unattested_numbers("Accuracy reached 95.20% over 24,000 samples in step 2.",
                              "accuracy of 95.2 percent on 24000 samples") == []
    # A genuinely invented number flags.
    assert unattested_numbers("The threshold is 0.42.", "no numbers here at all") == ["0.42"]
    assert unattested_numbers(None, "anything") == []


def test_unattested_numbers_handles_k_suffix_lists_and_years():
    from locus.eval.metrics import unattested_numbers

    # k-suffix shorthand attests the expanded number (live corpus: "payrolled -11k").
    assert unattested_numbers("Employment decreased by 11,000 in March.",
                              "payrolled -11k in march") == []
    assert unattested_numbers("Roughly 13,000 images for training.",
                              "approximately 13k images") == []
    # Digit LISTS must not be comma-merged: every member attests (live corpus: K-set).
    assert unattested_numbers(
        "Metrics for K in {10, 50, 100, 250, 500, 1000}.",
        r"coverage@k for \(k \in\) \(\{10,50,100,250,500,1000\}\)",
    ) == []
    # Month-year shorthand attests case-insensitively (doc_metrics lowercases source).
    assert unattested_numbers("Anchored since November 2025.", "stuck at 3.7% since nov-25") == []
    # Real thousands-grouping still normalizes.
    assert unattested_numbers("Sample of 24,000 rows.", "a sample of 24000 rows") == []


