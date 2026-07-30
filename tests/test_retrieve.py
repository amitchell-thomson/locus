"""Stage 6: retrieval — search (hybrid), expand, assemble.

search/expand/assemble run on a seeded DB with a monkeypatched query embedding, so they're
deterministic and need no Ollama. The cross-encoder rerank + full pipeline are guarded
integration tests (need the rerank extra + Ollama).
"""

import struct
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.retrieve import search as search_mod
from locus.retrieve.assemble import assemble
from locus.retrieve.expand import Expanded, expand
from locus.retrieve.rerank import select  # pure selection logic; does not load the model
from locus.retrieve.search import Candidate, search

DIM = 768


def _vec(head):
    return struct.pack(f"{DIM}f", *(head + [0.0] * (DIM - len(head))))


def _seed(conn):
    conn.execute(
        "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title,"
        " ingest_model, source_date, category, thesis, method, result, limitations, section_map) VALUES "
        "(1,'h','pdf','u','p','Control Notes','m','2023-06-01','paper','THESIS','METHOD','RESULT','LIMITS',"
        " '[{\"position\":0,\"title\":\"Stability\",\"page_start\":5,\"page_end\":7}]')"
    )
    conn.execute("INSERT INTO sections (id, doc_id, position, title, summary) VALUES (1,1,0,'Stability','poles determine stability')")
    conn.execute("INSERT INTO chunks (id, section_id, doc_id, position, raw_text, embed_model) VALUES (1,1,1,0,'stability poles feedback criterion','m')")
    conn.execute("INSERT INTO propositions (id, section_id, doc_id, position, text, embed_model) VALUES (1,1,1,0,'Stability is determined by the poles.','m')")
    conn.execute("INSERT INTO entities (doc_id, section_id, name, type) VALUES (1,1,'Nyquist criterion','theorem')")
    conn.execute("INSERT INTO chunk_vectors(chunk_id, embedding) VALUES (1, ?)", (_vec([1.0, 0.0, 0.0]),))
    conn.execute("INSERT INTO proposition_vectors(proposition_id, embedding) VALUES (1, ?)", (_vec([0.9, 0.1, 0.0]),))
    conn.execute("INSERT INTO section_vectors(section_id, embedding) VALUES (1, ?)", (_vec([0.8, 0.2, 0.0]),))
    conn.commit()


@pytest.fixture()
def conn(tmp_path: Path, monkeypatch):
    db = tmp_path / "r.db"
    migrate(db)
    c = get_connection(db)
    _seed(c)
    # Query embeds near the seeded vectors; no Ollama needed.
    monkeypatch.setattr(search_mod, "embed_text", lambda q: [1.0, 0.0, 0.0] + [0.0] * (DIM - 3))
    yield c
    c.close()


def test_search_returns_all_arms(conn):
    cands = search(conn, "stability poles")
    kinds = {c.kind for c in cands}
    assert {"proposition", "chunk", "section"} <= kinds
    # The chunk is found by BOTH dense and lexical (hybrid merge).
    chunk = next(c for c in cands if c.kind == "chunk" and c.id == 1)
    assert {"dense", "lexical"} <= chunk.sources


def test_facets_filter_by_date_and_category(conn):
    from locus.retrieve.search import Facets

    # Within the doc's date range + matching category: candidates survive.
    assert search(conn, "stability poles", Facets(since="2023-01-01", category="paper"))
    # Date window excludes the doc (dated 2023-06-01): no candidates.
    assert search(conn, "stability poles", Facets(until="2022-12-31")) == []
    # Wrong category: no candidates.
    assert search(conn, "stability poles", Facets(category="project")) == []
    # No facets: unrestricted, same as before.
    assert search(conn, "stability poles", None)


def test_lexical_arm_matches_text_terms(conn):
    # A term present in chunk text is found lexically even though we don't rely on embeddings.
    cands = search(conn, "criterion")
    assert any(c.kind == "chunk" and "lexical" in c.sources for c in cands)


def test_entity_arm_fires_on_named_entity(conn):
    # entity_aliases is empty here -> exercises the pre-`locus link` fallback path.
    cands = search(conn, "explain the Nyquist criterion please")
    assert any("entity" in c.sources for c in cands)


def test_entity_arm_matches_via_alias_variants(conn):
    # Section 2 names only the LONG variant; the alias substrate maps both surfaces to one
    # canonical, so a query naming the SHORT variant surfaces that section (step 12).
    conn.execute(
        "INSERT INTO sections (id, doc_id, position, title, summary) "
        "VALUES (2,1,1,'Divergences','measures of distributional distance')"
    )
    conn.execute(
        "INSERT INTO entities (doc_id, section_id, name, type) "
        "VALUES (1,2,'Kullback-Leibler (KL) divergence','concept')"
    )
    conn.executemany(
        "INSERT INTO entity_aliases "
        "(variant_name, variant_type, canonical_name, canonical_type, cluster_id, tier) "
        "VALUES (?,?,?,?,?,?)",
        [
            ("Kullback-Leibler (KL) divergence", "concept",
             "KL divergence", "concept", 1, "acronym"),
            ("KL divergence", "concept", "KL divergence", "concept", 1, "acronym"),
            ("Nyquist criterion", "theorem", "Nyquist criterion", "theorem", 2, "identity"),
        ],
    )
    conn.commit()
    cands = search(conn, "what is the KL divergence")
    hit = [c for c in cands if "entity" in c.sources]
    assert any(c.section_id == 2 for c in hit)
    # The identity-mapped entity still matches through the alias path.
    cands2 = search(conn, "explain the Nyquist criterion please")
    assert any("entity" in c.sources for c in cands2)


def test_expand_attaches_parent_context(conn):
    cands = search(conn, "stability poles")
    chunk = next(c for c in cands if c.kind == "chunk")
    (ex,) = [e for e in expand(conn, [chunk])]
    assert ex.doc_title is not None and ex.thesis == "THESIS"
    assert ex.section_title == "Stability"
    assert ex.section_summary == "poles determine stability"
    assert ex.page_start == 5 and ex.page_end == 7


def _expanded(kind, text):
    c = Candidate(kind=kind, id=1, doc_id=1, section_id=1, text=text, score=0.0)
    return Expanded(
        candidate=c, doc_id=1, doc_title="Doc", thesis="t", method="m", result="r",
        limitations="l", section_id=1, section_title="Sec", section_summary="summary",
        page_start=1, page_end=2,
    )


def test_assemble_drops_finest_first_under_budget():
    items = [_expanded("proposition", "a claim"), _expanded("chunk", "x " * 400)]
    # Budget large enough for synthesis+summary+claim, but not the big chunk.
    out = assemble(items, budget=120)
    assert "a claim" in out.text
    assert out.dropped >= 1  # the chunk was dropped
    assert "thesis: t" in out.text  # coarse content kept


def test_assemble_includes_provenance():
    out = assemble([_expanded("chunk", "excerpt text")], budget=10_000)
    assert out.citations
    assert any("Doc" in c for c in out.citations)


def test_assemble_dedupes_citations():
    # A proposition and a chunk from the same section share one provenance string:
    # it must appear once, not once per included unit.
    items = [_expanded("proposition", "a claim"), _expanded("chunk", "an excerpt")]
    out = assemble(items, budget=10_000)
    assert out.included == 2
    assert len(out.citations) == 1


# --- diversity-aware selection (rerank.select) -------------------------------------------


def _cand(kind, id_, doc_id, section_id):
    return Candidate(kind=kind, id=id_, doc_id=doc_id, section_id=section_id,
                     text="t", score=0.0)


def test_select_caps_units_per_section_and_kind():
    # Two chunks of the same section: only the better-ranked one survives while a
    # different section's chunk is available.
    ranked = [_cand("chunk", 1, 1, 1), _cand("chunk", 2, 1, 1), _cand("chunk", 3, 1, 2)]
    out = select(ranked, top_k=2, per_doc_cap=10)
    assert [c.id for c in out] == [1, 3]


def test_select_drops_section_summary_when_child_in_pool():
    # The section-summary candidate is redundant: expansion re-attaches the summary
    # to the surviving chunk anyway.
    ranked = [_cand("section", 1, 1, 1), _cand("chunk", 10, 1, 1), _cand("chunk", 11, 1, 2)]
    out = select(ranked, top_k=2, per_doc_cap=10)
    assert [(c.kind, c.id) for c in out] == [("chunk", 10), ("chunk", 11)]


def test_select_keeps_section_summary_when_its_child_never_makes_the_cut():
    """The 2026-07-30 defect: a whole document deleted by a child that is never selected.

    Shape of a captured rough note — one section whose summary reranks at the top of the
    pool and whose only chunk is raw handwriting that reranks near the bottom. Suppressing
    the summary because that chunk merely EXISTS lost the document from the results
    entirely (docs/rough-note-retrieval-finding.md). Other documents fill the cut, so the
    note's chunk is far out of reach; the summary must still earn its slot on score.
    """
    ranked = [
        _cand("section", 1, 1, 1),                     # the note's summary — best in pool
        _cand("chunk", 20, 2, 2), _cand("chunk", 21, 3, 3),  # unrelated docs fill the cut
        _cand("chunk", 10, 1, 1),                      # the note's own chunk, ranked last
    ]
    out = select(ranked, top_k=2, per_doc_cap=10)
    kinds = [(c.kind, c.id) for c in out]
    # Even with the section dropped, chunk 10 is out of reach of a 2-slot cut, so
    # suppressing the summary would leave document 1 unrepresented entirely.
    assert ("section", 1) in kinds, kinds
    assert out[0].kind == "section"  # kept in rank order, not demoted to a refill slot


def test_select_keeps_section_summary_without_child():
    ranked = [_cand("section", 1, 1, 1), _cand("chunk", 10, 1, 2)]
    out = select(ranked, top_k=2, per_doc_cap=10)
    assert [(c.kind, c.id) for c in out] == [("section", 1), ("chunk", 10)]


def test_select_per_doc_cap_promotes_second_document():
    # Doc 1 outranks everywhere, but the cap forces doc 2 into the top-k
    # (the cross-domain synthesis fix).
    ranked = [_cand("chunk", i, 1, i) for i in range(1, 6)] + [_cand("chunk", 99, 2, 99)]
    out = select(ranked, top_k=4, per_doc_cap=3)
    assert [c.id for c in out] == [1, 2, 3, 99]


def test_select_refills_rather_than_underfilling():
    # Caps would leave the top-k underfull -> the best skipped candidates come back,
    # and rank order is preserved in the output.
    ranked = [_cand("chunk", i, 1, i) for i in range(1, 5)]
    out = select(ranked, top_k=4, per_doc_cap=2)
    assert [c.id for c in out] == [1, 2, 3, 4]


def _scored(kind, id_, doc_id, section_id, score):
    c = _cand(kind, id_, doc_id, section_id)
    c.rerank_score = score
    return c


def test_select_refill_respects_min_score():
    # Doc cap skips ids 3 and 4; the refill may only restore the one above the floor.
    # An underfull top-k beats padding it with judged-irrelevant candidates.
    ranked = [
        _scored("chunk", 1, 1, 1, 5.0),
        _scored("chunk", 2, 1, 2, 4.0),
        _scored("chunk", 3, 1, 3, 1.0),   # skipped by cap, above floor -> refilled
        _scored("chunk", 4, 1, 4, -3.0),  # skipped by cap, below floor -> stays out
    ]
    out = select(ranked, top_k=4, per_doc_cap=2, min_score=0.0)
    assert [c.id for c in out] == [1, 2, 3]


def test_select_refill_unbounded_without_min_score():
    ranked = [
        _scored("chunk", 1, 1, 1, 5.0),
        _scored("chunk", 2, 1, 2, 4.0),
        _scored("chunk", 3, 1, 3, -3.0),
    ]
    out = select(ranked, top_k=3, per_doc_cap=2)  # min_score=None: old behaviour
    assert [c.id for c in out] == [1, 2, 3]


def test_assemble_citation_details_carry_best_rerank_score():
    a = _expanded("proposition", "a claim")
    b = _expanded("chunk", "an excerpt")
    a.candidate.rerank_score, b.candidate.rerank_score = 2.5, 4.0
    out = assemble([a, b], budget=10_000)
    assert len(out.citation_details) == 1  # shared provenance -> one detail
    d = out.citation_details[0]
    assert d.doc_id == 1 and d.rerank_score == 4.0
    assert d.text == out.citations[0]


def _pipeline_fakes(monkeypatch, floor, scores):
    """Patch pipeline load+rerank: each candidate gets the next score from `scores`."""
    import types

    from locus.retrieve import pipeline as pl

    rcfg = types.SimpleNamespace(rerank_top_k=8, per_doc_cap=3, min_rerank_score=floor)
    monkeypatch.setattr(
        pl, "load",
        lambda: types.SimpleNamespace(retrieve=rcfg, figures=types.SimpleNamespace(image_cap=3)),
    )

    def _rr(query, candidates, gather, cfg, *, prefer_code=False, rough_ids=None, **_kw):
        for c, s in zip(candidates, scores):
            c.rerank_score = s
        return candidates[: len(scores)][: cfg.rerank_top_k]

    monkeypatch.setattr(pl, "_rerank_with_expansion", _rr)
    return pl


def test_retrieve_confidence_bands(conn, monkeypatch):
    # Ambiguous: best within DEEP_FLOOR_MARGIN below the floor (multi-part-query story).
    pl = _pipeline_fakes(monkeypatch, floor=0.0, scores=[-2.0])
    r = pl.retrieve("stability poles", conn=conn)
    assert r.low_confidence and r.confidence_band == "ambiguous"
    assert r.survivors and r.citation_details  # flag, never filter

    # Absent: best below even the deep floor.
    pl = _pipeline_fakes(monkeypatch, floor=0.0, scores=[-7.0])
    r = pl.retrieve("stability poles", conn=conn)
    assert r.low_confidence and r.confidence_band == "absent"

    # Above the floor -> confident.
    pl = _pipeline_fakes(monkeypatch, floor=0.0, scores=[6.0])
    r = pl.retrieve("stability poles", conn=conn)
    assert r.low_confidence is False and r.confidence_band is None

    # No floor configured -> never flagged.
    pl = _pipeline_fakes(monkeypatch, floor=None, scores=[-9.0])
    assert pl.retrieve("stability poles", conn=conn).low_confidence is False


def test_retrieve_prunes_deep_noise_only_when_signal_exists(conn, monkeypatch):
    # Signal (+6) present: the deep-noise unit (-7) is pruned, but the moderate-negative
    # complementary-facet unit (-2) survives (cross-domain co-retrieval must keep it).
    pl = _pipeline_fakes(monkeypatch, floor=0.0, scores=[6.0, -2.0, -7.0])
    r = pl.retrieve("stability poles", conn=conn)
    kept = [c.rerank_score for c in r.survivors]
    assert 6.0 in kept and -2.0 in kept and -7.0 not in kept

    # No signal: nothing is pruned — weak matches are all the consumer gets.
    pl = _pipeline_fakes(monkeypatch, floor=0.0, scores=[-2.0, -7.0])
    r = pl.retrieve("stability poles", conn=conn)
    assert sorted(c.rerank_score for c in r.survivors) == [-7.0, -2.0]


def test_confidence_banner_wording():
    from locus.retrieve.pipeline import confidence_banner

    assert confidence_banner(None) == ""
    assert "covers the parts separately" in confidence_banner("ambiguous")
    assert "very likely does not cover" in confidence_banner("absent")
    # The ambiguous wording must NOT claim the corpus lacks the topic.
    assert "does not cover" not in confidence_banner("ambiguous")


# --- guarded end-to-end ---

def _stack_ready():
    try:
        import sentence_transformers  # noqa: F401
        from locus.ingest.embed import embed_texts
        return bool(embed_texts(["ping"]))
    except Exception:
        return False


@pytest.mark.skipif(not _stack_ready(), reason="rerank extra / Ollama unavailable")
def test_retrieve_end_to_end(conn, monkeypatch):
    from locus.retrieve import retrieve
    # real embedding for the query (Ollama up); rerank uses the cross-encoder.
    monkeypatch.undo()
    r = retrieve("stability poles", conn=conn)
    assert r.survivors
    assert isinstance(r.context, str)


# --- facet-aware confidence + floor enforcement (round-3 evaluation) ----------------------


def test_split_facets_on_bridge_queries():
    from locus.retrieve.pipeline import split_facets

    f = split_facets(
        "How do my signal-processing notes on spectral analysis relate to regime "
        "detection in my quant work?"
    )
    assert len(f) == 2
    assert "spectral analysis" in f[0] and "regime detection" in f[1]

    f = split_facets("What is the connection between Fourier analysis and solving PDEs numerically?")
    assert len(f) == 2

    # Single-topic queries must NOT unlock the facet path.
    assert split_facets("What is the Biot number?") == []
    assert split_facets("gradient descent learning rate") == []


def test_decompose_conjunction():
    from locus.retrieve.pipeline import decompose_conjunction

    # 'both X and Y' distributes the shared context over each conjunct.
    assert decompose_conjunction("How is Fourier used both for signals and for solving PDEs?") == [
        "How is Fourier used for signals?", "How is Fourier used for solving PDEs?",
    ]
    # repeated preposition without 'both' ('in X and in Y').
    assert decompose_conjunction("Where does KL appear in estimation and in regime evaluation?") == [
        "Where does KL appear in estimation?", "Where does KL appear in regime evaluation?",
    ]
    # compound nouns / single-topic queries must NOT fire (no bare-'and' mis-split).
    assert decompose_conjunction("Explain signals and systems theory.") == []
    assert decompose_conjunction("What is the Biot number and transient conduction?") == []
    assert decompose_conjunction("Compare both methods.") == []  # no 'and' after 'both'


def _facet_fakes(monkeypatch, floor, scores, facet_scores):
    """Pipeline fakes where score_pairs returns the next row of facet_scores per facet."""
    import types

    from locus.retrieve import pipeline as pl

    rcfg = types.SimpleNamespace(rerank_top_k=8, per_doc_cap=3, min_rerank_score=floor)
    monkeypatch.setattr(
        pl, "load",
        lambda: types.SimpleNamespace(retrieve=rcfg, figures=types.SimpleNamespace(image_cap=3)),
    )

    def _rr(query, candidates, gather, cfg, *, prefer_code=False, rough_ids=None, **_kw):
        for c, s in zip(candidates, scores):
            c.rerank_score = s
        return candidates[: len(scores)][: cfg.rerank_top_k]

    monkeypatch.setattr(pl, "_rerank_with_expansion", _rr)
    rows = iter(facet_scores)
    monkeypatch.setattr(pl, "score_pairs", lambda f, texts: list(next(rows))[: len(texts)])
    return pl


BRIDGE_Q = "How does spectral analysis from my notes relate to regime detection in finance?"


def test_facet_covered_bridge_query_is_confident(conn, monkeypatch):
    # Full-query scores all below the floor (each unit covers ONE side) — the old
    # behaviour banner-flagged exactly the co-retrieved set the synthesis use case needs.
    # Each facet is cleared by some unit -> confident, no banner.
    pl = _facet_fakes(
        monkeypatch, floor=0.0,
        scores=[-2.0, -3.0, -9.0],
        facet_scores=[[5.0, -8.0, -9.0], [-8.0, 4.0, -9.0]],
    )
    # Seed three candidates: monkeypatched rerank just scores whatever search returns;
    # the seeded DB yields >=3 candidates (prop + chunk + section).
    r = pl.retrieve(BRIDGE_Q, conn=conn)
    assert r.confidence_band is None and r.low_confidence is False
    # Floor enforcement: the unit clearing neither the deep floor nor any facet floor
    # (-9 on everything) is pruned; the facet-covering units survive.
    kept = [c.rerank_score for c in r.survivors]
    assert -2.0 in kept and -3.0 in kept and -9.0 not in kept


def test_facet_uncovered_bridge_query_stays_flagged(conn, monkeypatch):
    # One facet is never cleared -> the low-confidence story is real; nothing is pruned
    # (flag, never filter: weak matches are all the consumer gets).
    pl = _facet_fakes(
        monkeypatch, floor=0.0,
        scores=[-2.0, -3.0],
        facet_scores=[[5.0, -8.0], [-8.0, -7.0]],
    )
    r = pl.retrieve(BRIDGE_Q, conn=conn)
    assert r.confidence_band == "ambiguous"
    assert sorted(c.rerank_score for c in r.survivors) == [-3.0, -2.0]


def test_non_bridge_weak_query_skips_facet_check(conn, monkeypatch):
    # No bridge phrasing: split_facets yields nothing, bands behave as before.
    pl = _facet_fakes(monkeypatch, floor=0.0, scores=[-2.0], facet_scores=[])
    r = pl.retrieve("stability poles", conn=conn)
    assert r.confidence_band == "ambiguous"


# --- implementation-intent source preference (round-3: prose-about-code outranks code) -----


def _src(id_, score, path=None):
    c = Candidate(kind="chunk", id=id_, doc_id=1, section_id=id_, text="t", score=0.0,
                  file_path=path)
    c.rerank_score = score
    return c


def test_prefer_code_promotes_a_source_unit():
    ranked = [
        _src(1, 5.0, "docs/design.md"),
        _src(2, 4.0, "docs/phase2.md"),
        _src(3, 2.0, "src/hmm.py"),
    ]
    out = select(ranked, top_k=2, per_doc_cap=10, min_score=0.0, prefer_code=True)
    # The source unit replaces the lowest-ranked prose unit; the cut stays full.
    assert [(c.id, c.file_path) for c in out] == [(1, "docs/design.md"), (3, "src/hmm.py")]


def test_prefer_code_respects_the_floor():
    ranked = [
        _src(1, 5.0, "docs/design.md"),
        _src(2, 4.0, "docs/phase2.md"),
        _src(3, -3.0, "src/hmm.py"),  # judged irrelevant: must NOT be promoted
    ]
    out = select(ranked, top_k=2, per_doc_cap=10, min_score=0.0, prefer_code=True)
    assert [c.id for c in out] == [1, 2]


def test_prefer_code_noop_when_source_already_selected():
    ranked = [_src(1, 5.0, "src/hmm.py"), _src(2, 4.0, "docs/design.md")]
    out = select(ranked, top_k=2, per_doc_cap=10, min_score=0.0, prefer_code=True)
    assert [c.id for c in out] == [1, 2]


def test_prefer_code_off_by_default():
    ranked = [
        _src(1, 5.0, "docs/design.md"),
        _src(2, 4.0, "docs/phase2.md"),
        _src(3, 2.0, "src/hmm.py"),
    ]
    out = select(ranked, top_k=2, per_doc_cap=10, min_score=0.0)
    assert [c.id for c in out] == [1, 2]


# --- path arm + query-named exemption (round-3 residual: source under-retrieval) -----------


def test_path_arm_surfaces_query_named_file(conn):
    conn.execute(
        "INSERT INTO sections (id, doc_id, position, title, summary, file_path) VALUES "
        "(7,1,1,'retrieve/rerank.py','cross-encoder reranking module','locus/retrieve/rerank.py')"
    )
    conn.execute("INSERT INTO section_vectors(section_id, embedding) VALUES (7, ?)",
                 (_vec([0.0, 1.0, 0.0]),))  # far from the query embedding: dense won't find it
    conn.commit()
    cands = search(conn, "how does the rerank step order candidates?")
    hit = next((c for c in cands if c.file_path == "locus/retrieve/rerank.py"), None)
    assert hit is not None and "path" in hit.sources
    # Generic stems never fire: a file named test.py/config.py is a convention, not a target.
    assert all("path" not in c.sources for c in search(conn, "how do I config the test run?"))


def _pathc(id_, doc_id, section_id, score, sources=("dense",), kind="chunk", path=None):
    c = Candidate(kind=kind, id=id_, doc_id=doc_id, section_id=section_id, text="t",
                  score=0.0, sources=set(sources), file_path=path)
    c.rerank_score = score
    return c


def test_query_named_candidate_bypasses_per_doc_cap():
    # One repo-doc fills its cap with prose-about-code; the query-named file's section
    # (path arm) must still be selectable — the cap's breadth rationale does not apply
    # to a file the query asked for by name.
    ranked = [
        _pathc(1, 1, 1, 6.0),
        _pathc(2, 1, 2, 5.0),
        _pathc(3, 1, 3, 4.0),
        _pathc(4, 1, 4, 3.5, sources=("path",), kind="section", path="src/rerank.py"),
        _pathc(5, 2, 5, 1.0),
    ]
    out = select(ranked, top_k=5, per_doc_cap=3)
    assert any(c.id == 4 for c in out)


def test_query_named_section_survives_child_redundancy():
    # The named file's own chunk sits deep in the pool (will never be selected): it must
    # not demote the path-arm section, or the file vanishes entirely.
    ranked = [
        _pathc(1, 1, 1, 6.0),
        _pathc(2, 1, 9, 3.5, sources=("path",), kind="section", path="src/eval.py"),
        _pathc(3, 1, 9, -5.0),  # the low-ranked child chunk of the same section
    ]
    out = select(ranked, top_k=2, per_doc_cap=3)
    assert [(c.id, c.kind) for c in out] == [(1, "chunk"), (2, "section")]


def test_unnamed_section_redundancy_rule_unchanged():
    ranked = [
        _pathc(1, 1, 9, 6.0, kind="section"),  # plain dense section, child in pool
        _pathc(2, 1, 9, 5.0),
        _pathc(3, 1, 8, 4.0),
    ]
    out = select(ranked, top_k=2, per_doc_cap=3)
    assert [(c.id, c.kind) for c in out] == [(2, "chunk"), (3, "chunk")]


def test_figure_images_floored_and_cited_count_kept(monkeypatch, tmp_path):
    """Image attachment respects the rerank floor (text/citations never filtered), and
    figures_cited records the pre-cap count so consumers can announce truncation
    (2026-06-06 audit findings 3+4)."""
    import types

    from locus.retrieve import pipeline as pl

    rcfg = types.SimpleNamespace(rerank_top_k=8, per_doc_cap=3, min_rerank_score=0.22)
    monkeypatch.setattr(
        pl, "load",
        lambda: types.SimpleNamespace(retrieve=rcfg, figures=types.SimpleNamespace(image_cap=2)),
    )

    def fake_search(conn, q, facets=None):
        return []

    def fake_rerank(q, cands, gather, cfg, *, prefer_code=False, rough_ids=None, **_kw):
        return []

    def fake_expand(conn, survivors):
        from locus.retrieve.expand import Expanded
        from locus.retrieve.search import Candidate

        def fig(id_, score):
            c = Candidate("figure", id_, 1, 1, f"fig {id_}", 0.0)
            c.rerank_score = score
            return Expanded(
                candidate=c, doc_id=1, doc_title="D", thesis=None, method=None,
                result=None, limitations=None, section_id=1, section_title=None,
                section_summary=None, page_start=id_, page_end=id_,
                figure_path=f"f{id_}.png", figure_caption=None,
                figure_kind="raster", figure_page=id_,
            )

        # 4 figure survivors: scores 5.0, 3.0, 1.0, and -0.4 (below the 0.22 floor)
        return [fig(1, 5.0), fig(2, 3.0), fig(3, 1.0), fig(4, -0.4)]

    monkeypatch.setattr(pl, "search", fake_search)
    monkeypatch.setattr(pl, "_rerank_with_expansion", fake_rerank)
    monkeypatch.setattr(pl, "expand", fake_expand)
    monkeypatch.setattr(pl, "assemble", lambda exp: types.SimpleNamespace(
        text="CTX", citations=[], citation_details=[], included=0, dropped=0))

    # Stub conn: retrieve() now queries rough_doc_ids(conn); this test fakes the pipeline
    # internals, so a no-row execute() stands in for "no rough docs".
    r = pl.retrieve("q", conn=types.SimpleNamespace(execute=lambda *a, **k: []))
    assert r.figures_cited == 4
    assert [f.page for f in r.figures] == [1, 2]  # floor killed -0.4, cap kept top 2
    assert all(f.rerank_score >= 0.22 for f in r.figures)


def test_excluded_doc_ids_resolves_source_uris(conn):
    """The self-ingestion exclusion maps configured source_uris to doc ids (exact match)."""
    from locus.retrieve.pipeline import _excluded_doc_ids

    assert _excluded_doc_ids(conn, ["u"]) == {1}  # seeded doc's source_uri
    assert _excluded_doc_ids(conn, ["/not/in/corpus"]) == set()
    assert _excluded_doc_ids(conn, []) == set()
