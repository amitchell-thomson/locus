"""Multi-query cross-domain expansion (locus/retrieve/multiquery.py + pipeline wiring).

Model-free: the local-qwen reformulator (generate_structured) and the cross-encoder
(score_pairs) are monkeypatched, so these test the expansion LOGIC — dedup, best-effort
failure, and the core mechanism (a cross-domain candidate scored against its own variant
clears the floor instead of being demoted by the original phrasing).
"""

import types

import pytest

from locus.retrieve import multiquery
from locus.retrieve import pipeline as pl
from locus.retrieve.multiquery import _Reformulations, reformulate
from locus.retrieve.search import Candidate


def test_reformulate_dedups_and_keeps_k(monkeypatch):
    monkeypatch.setattr(
        multiquery, "generate_structured",
        lambda *a, **k: _Reformulations(variants=["State-space / Kalman observer design", "the query", "HMM time series", ""]),
    )
    out = reformulate("the query", 2)
    assert out == ["State-space / Kalman observer design", "HMM time series"]  # dropped echo + blank, capped at k


def test_reformulate_is_best_effort_on_failure(monkeypatch):
    def _boom(*a, **k):
        raise RuntimeError("ollama down")

    monkeypatch.setattr(multiquery, "generate_structured", _boom)
    assert reformulate("q", 2) == []  # never raises; falls back to single-query


def test_reformulate_k_zero_skips_model(monkeypatch):
    monkeypatch.setattr(multiquery, "generate_structured", lambda *a, **k: pytest.fail("should not call"))
    assert reformulate("q", 0) == []


def _cand(kind, id_, text):
    return Candidate(kind, id_, doc_id=id_, section_id=id_, text=text, score=0.0, sources={"dense"})


def test_expansion_surfaces_cross_domain_match(monkeypatch):
    """The headline H2 fix: a doc buried by the original phrasing surfaces once it's scored
    against the reformulation that speaks its vocabulary."""
    # score_pairs(query, texts): finance query loves `fin`, buries `eng`; the eng variant flips it.
    def fake_score_pairs(q, texts):
        out = []
        for t in texts:
            if q == "ENG_VARIANT":
                out.append(6.0 if "ENGINEERING" in t else -2.0)
            else:  # original finance phrasing
                out.append(5.0 if "FINANCE" in t else -3.0)  # eng buried below the 0.22 floor
        return out

    monkeypatch.setattr(pl, "score_pairs", fake_score_pairs)
    monkeypatch.setattr(multiquery, "reformulate", lambda q, k, **kw: ["ENG_VARIANT"])
    # search() builds FRESH Candidate objects each call (same unit, new instance) — model that.
    gather = lambda q: [_cand("chunk", 1, "FINANCE regime switching"),
                        _cand("chunk", 2, "ENGINEERING state-space control")]
    base = gather("orig")

    cfg = types.SimpleNamespace(
        rerank_top_k=8, per_doc_cap=3, min_rerank_score=0.22,
        multi_query_expansion=True, multi_query_k=1,
    )
    # bridge-shaped query → expansion triggers
    survivors = pl._rerank_with_expansion(
        "how does regime switching relate to state-space control", base, gather, cfg, prefer_code=False,
    )
    by_id = {c.id: c.rerank_score for c in survivors}
    assert by_id[2] == 6.0  # eng kept its best-variant score (was -3.0 under the original)
    assert by_id[1] == 5.0  # fin kept its original-query score (max over variants)


def test_facet_decomposition_surfaces_crowded_out_facet(monkeypatch):
    """A 'both X and Y' query whose Y-facet is buried by the dominant X-facet in a single
    pass: the deterministic decomposition retrieves each facet separately so both surface.
    No LLM reformulation needed (multi_query_k=0) — decomposition is free and still fires."""
    def fake_score_pairs(q, texts):
        ql = q.lower()
        only_sig = "signals" in ql and "pde" not in ql
        only_pde = "pde" in ql and "signals" not in ql
        out = []
        for t in texts:
            if only_sig:
                out.append(6.0 if "SIGNALS" in t else -2.0)
            elif only_pde:
                out.append(6.0 if "PDE" in t else -2.0)
            else:  # full query carries BOTH facets; PDE dominates, SIGNALS buried below floor
                out.append(5.0 if "PDE" in t else -3.0)
        return out

    monkeypatch.setattr(pl, "score_pairs", fake_score_pairs)
    monkeypatch.setattr(multiquery, "reformulate", lambda *a, **k: pytest.fail("LLM not needed"))
    gather = lambda q: [_cand("chunk", 1, "SIGNALS time frequency"),
                        _cand("chunk", 2, "PDE laplace transform")]
    cfg = types.SimpleNamespace(
        rerank_top_k=8, per_doc_cap=3, min_rerank_score=0.22,
        multi_query_expansion=True, multi_query_k=0,  # LLM off; decomposition still fires
    )
    survivors = pl._rerank_with_expansion(
        "How is analysis used both for signals and for solving PDEs?",
        gather("orig"), gather, cfg, prefer_code=False,
    )
    by_id = {c.id: c.rerank_score for c in survivors}
    assert by_id[1] == 6.0  # SIGNALS surfaced via the decomposed 'for signals' sub-query
    assert by_id[2] == 6.0  # PDE via 'for solving PDEs' (max over variants beats the orig 5.0)


def test_no_expansion_when_disabled_is_single_pass(monkeypatch):
    fin = _cand("chunk", 1, "FINANCE regime switching")
    monkeypatch.setattr(pl, "score_pairs", lambda q, texts: [4.0 for _ in texts])
    monkeypatch.setattr(multiquery, "reformulate", lambda *a, **k: pytest.fail("expansion must not run"))
    cfg = types.SimpleNamespace(
        rerank_top_k=8, per_doc_cap=3, min_rerank_score=0.22,
        multi_query_expansion=False, multi_query_k=2,
    )
    survivors = pl._rerank_with_expansion("plain query", [fin], lambda q: [fin], cfg, prefer_code=False)
    assert [c.rerank_score for c in survivors] == [4.0]
