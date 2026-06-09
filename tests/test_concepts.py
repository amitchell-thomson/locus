"""Code concept pass (CLAUDE.md §1.2 Link) — model-free.

The LLM call is monkeypatched, so these assert the deterministic contract: grounding/noise/
stoplist/cap filtering, section anchoring, and — the use-case proof — that a code doc and a
paper sharing a concept surface link through the deterministic alias tiers (no model).
"""

from dataclasses import dataclass, field
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest import concepts
from locus.ingest.entities import Entity


def _fake_llm(monkeypatch, returned: list[tuple[str, str]]):
    """Patch the pass's generate_structured to return these (name, type) concepts verbatim."""
    payload = concepts._Concepts(
        concepts=[concepts._Concept(name=n, type=t, evidence=n) for n, t in returned]
    )
    monkeypatch.setattr(concepts, "generate_structured", lambda *a, **k: payload)


def test_filters_noise_stoplist_and_ungrounded(monkeypatch):
    narrative_summaries = ["The project implements regime switching with a Markov model."]
    _fake_llm(
        monkeypatch,
        [
            ("regime switching", "concept"),     # grounded, kept
            ("Markov model", "concept"),         # grounded, kept
            ("Python", "tool"),                  # stoplist -> dropped
            ("Kalman filter", "method"),         # NOT in narrative -> grounding drop
            ("equation 1.2", "concept"),         # noise (numbered label) -> dropped
        ],
    )
    out = concepts.extract_code_concepts(
        "regime-ml", "Thesis: detect market regimes.", narrative_summaries
    )
    names = {e.name for e in out}
    assert names == {"regime switching", "Markov model"}
    assert all(isinstance(e, Entity) for e in out)


def test_dedups_and_caps(monkeypatch):
    _fake_llm(
        monkeypatch,
        [("Black-Scholes", "concept"), ("Black-Scholes", "concept"), ("Monte Carlo", "method")],
    )
    narrative = ["Uses Black-Scholes pricing and Monte Carlo simulation."]
    deduped = concepts.extract_code_concepts("alpha", "Thesis: option pricing.", narrative)
    assert len(deduped) == 2  # the duplicate (name,type) collapses

    _fake_llm(monkeypatch, [("Black-Scholes", "concept"), ("Monte Carlo", "method")])
    capped = concepts.extract_code_concepts(
        "alpha", "Thesis: option pricing.", narrative, max_concepts=1
    )
    assert len(capped) == 1


def test_custom_stoplist_overrides_default(monkeypatch):
    _fake_llm(monkeypatch, [("covariance", "concept")])
    narrative = ["Computes a covariance matrix."]
    # 'covariance' is not in the default stoplist (kept) but is when supplied explicitly.
    assert concepts.extract_code_concepts("x", "t", narrative)
    assert not concepts.extract_code_concepts("x", "t", narrative, stoplist={"covariance"})


# --- pipeline helpers (anchoring + README reconstruction) --------------------------------


@dataclass
class _FakeSection:
    summary: str
    entities: list = field(default_factory=list)


def test_attach_concepts_anchors_to_attesting_section():
    from locus.ingest_pipeline import _attach_concepts

    s0 = _FakeSection("unrelated networking code")
    s1 = _FakeSection("implements the Black-Scholes option pricing model")
    _attach_concepts([Entity(name="Black-Scholes", type="concept")], [s0, s1])
    assert not s0.entities and len(s1.entities) == 1  # anchored to the attesting section

    s2 = _FakeSection("nothing relevant here")
    _attach_concepts([Entity(name="Kalman filter", type="method")], [s2])
    assert len(s2.entities) == 1  # fallback: first section when nothing attests


# --- the use-case proof: code <-> paper link via deterministic alias tiers ----------------


def _seed_link_corpus(db: Path) -> None:
    migrate(db)
    conn = get_connection(db)
    with conn:
        rows = [
            ("hc", "code", "locusdrop:alpha-fund", "Alpha Fund"),
            ("hp", "pdf", "2511.99999v1", "Option Pricing under Black-Scholes"),
            ("hx", "pdf", "9999.00000v1", "Unrelated Heat Transfer Paper"),
        ]
        for h, st, uri, title in rows:
            conn.execute(
                "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
                "ingest_model, thesis, method, result, limitations) "
                "VALUES (?,?,?,?,?,?,?,?,?,?)",
                (h, st, uri, f"{h}.x", title, "t", "x", "x", "x", "x"),
            )
        for doc_id in (1, 2, 3):
            conn.execute(
                "INSERT INTO sections (doc_id, position, title, summary) VALUES (?,?,?,?)",
                (doc_id, 0, "s", "s"),
            )
        # The concept the backfill would add to the repo (doc 1) now matches the paper (doc 2).
        ents = [
            (1, 1, "Black-Scholes", "concept"),   # <- from the concept pass
            (2, 2, "Black-Scholes", "concept"),   # <- the paper's own entity
            (3, 3, "conduction", "concept"),       # unrelated doc shares nothing
        ]
        for doc_id, sec_id, name, typ in ents:
            conn.execute(
                "INSERT INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
                (doc_id, sec_id, name, typ),
            )
    conn.close()


def test_code_concept_links_to_paper_after_deterministic_link(tmp_path):
    from locus.link.aliases import build_aliases
    from locus.link.related import related_documents

    db = tmp_path / "locus.db"
    _seed_link_corpus(db)
    conn = get_connection(db)
    try:
        build_aliases(conn, use_llm=False, use_cache=False, log=lambda _m: None)
        related = related_documents(conn, 1, top_n=5)  # the alpha-fund repo
        linked = {r.doc_id for r in related}
        assert 2 in linked          # links to the Black-Scholes paper via the shared concept
        assert 3 not in linked      # not to the unrelated heat-transfer paper
    finally:
        conn.close()
