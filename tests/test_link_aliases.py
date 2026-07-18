"""Step 12: cross-document entity-alias resolution (locus/link/).

All model-free: deterministic tiers run as-is; the Claude adjudicator is exercised through
an injected fake runner (the headless `claude -p` seam); embeddings come from an injected embed_fn.
"""

import json
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.link import aliases as al
from locus.link.adjudicate import AliasVerdict, ClusterMember, adjudicate_cluster


def _seed_doc(conn, doc_id: int, source_type: str = "pdf", title: str | None = None):
    conn.execute(
        "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title,"
        " ingest_model) VALUES (?,?,?,?,?,?,?)",
        (doc_id, f"h{doc_id}", source_type, f"u{doc_id}", f"p{doc_id}",
         title or f"Doc {doc_id}", "m"),
    )


def _seed_section(conn, sec_id: int, doc_id: int, position: int = 0):
    conn.execute(
        "INSERT INTO sections (id, doc_id, position, title, summary) VALUES (?,?,?,?,?)",
        (sec_id, doc_id, position, f"S{sec_id}", f"summary {sec_id}"),
    )


def _seed_entity(conn, doc_id: int, sec_id: int, name: str, type_: str = "concept"):
    conn.execute(
        "INSERT OR IGNORE INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
        (doc_id, sec_id, name, type_),
    )


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "a.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _aliases(conn) -> dict[tuple[str, str], tuple[str, str, str]]:
    """(variant_name, variant_type) -> (canonical_name, canonical_type, tier)."""
    return {
        (r["variant_name"], r["variant_type"]):
            (r["canonical_name"], r["canonical_type"], r["tier"])
        for r in conn.execute("SELECT * FROM entity_aliases")
    }


# --- deterministic tiers ----------------------------------------------------------------------


def test_casefold_merges_case_variants(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_section(conn, 2, 1, 1)
    _seed_doc(conn, 2)
    _seed_section(conn, 3, 2)
    # "Bode Diagram" in two docs, "Bode diagram" in one -> canonical = most doc-attested.
    _seed_entity(conn, 1, 1, "Bode Diagram")
    _seed_entity(conn, 2, 3, "Bode Diagram")
    _seed_entity(conn, 1, 2, "Bode diagram")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    assert a[("Bode diagram", "concept")] == ("Bode Diagram", "concept", "casefold")
    assert a[("Bode Diagram", "concept")][0] == "Bode Diagram"


def test_casefold_same_type_only(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_section(conn, 2, 1, 1)
    _seed_entity(conn, 1, 1, "Fourier Transform", "concept")
    _seed_entity(conn, 1, 2, "fourier transform", "method")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    # Different types never merge deterministically (LLM-only).
    assert a[("Fourier Transform", "concept")][0] == "Fourier Transform"
    assert a[("fourier transform", "method")][0] == "fourier transform"


def test_short_names_never_casefold_merge(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_section(conn, 2, 1, 1)
    _seed_entity(conn, 1, 1, "VAR", "method")
    _seed_entity(conn, 1, 2, "var", "method")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    assert a[("VAR", "method")] == ("VAR", "method", "identity")
    assert a[("var", "method")] == ("var", "method", "identity")


def test_punct_tier_merges_hyphen_variants(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_section(conn, 2, 1, 1)
    _seed_entity(conn, 1, 1, "Black-Scholes model", "method")
    _seed_entity(conn, 1, 2, "Black Scholes model", "method")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    canon = a[("Black-Scholes model", "method")][0]
    assert a[("Black Scholes model", "method")][0] == canon


def test_acronym_expansion_links_attested_surfaces(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_doc(conn, 2)
    _seed_section(conn, 2, 2)
    _seed_section(conn, 3, 2, 1)
    # Long form in doc 1; bare acronym form in doc 2 (the singular of "LTI models").
    _seed_entity(conn, 1, 1, "Linear, Time-invariant (LTI) models")
    _seed_entity(conn, 2, 2, "LTI model")
    _seed_entity(conn, 2, 3, "LTI")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    canon = a[("Linear, Time-invariant (LTI) models", "concept")][0]
    # All three surfaces collapse onto one canonical.
    assert a[("LTI model", "concept")][0] == canon
    assert a[("LTI", "concept")][0] == canon


def test_acronym_requires_attestation(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    # Long form only; no bare-acronym entity exists -> nothing to link, no invention.
    _seed_entity(conn, 1, 1, "Kullback-Leibler (KL) divergence")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    assert a[("Kullback-Leibler (KL) divergence", "concept")][2] == "identity"
    assert len(a) == 1  # no invented surfaces


def test_cross_doc_plural_merges_onto_attested_singular(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_doc(conn, 2)
    _seed_section(conn, 2, 2)
    _seed_entity(conn, 1, 1, "Laplace transform")
    _seed_entity(conn, 2, 2, "Laplace transforms")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    assert a[("Laplace transforms", "concept")][0] == "Laplace transform"


def test_plural_without_attested_singular_stays(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_entity(conn, 1, 1, "Fourier series")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    # "Fourier serie" is attested nowhere -> never mangled.
    assert a[("Fourier series", "concept")] == ("Fourier series", "concept", "identity")


def test_code_entities_excluded_from_clustering(conn):
    _seed_doc(conn, 1, source_type="code")
    _seed_section(conn, 1, 1)
    _seed_doc(conn, 2)
    _seed_section(conn, 2, 2)
    # Same-ish surfaces on a code doc and a prose doc; the code-only identity must not
    # join any cluster, but still gets an identity row (join totality).
    _seed_entity(conn, 1, 1, "Rebalancer.weights", "method")
    _seed_entity(conn, 1, 1, "rebalancer.Weights", "method")
    _seed_entity(conn, 2, 2, "portfolio rebalancing", "concept")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    a = _aliases(conn)
    assert a[("Rebalancer.weights", "method")] == ("Rebalancer.weights", "method", "identity")
    assert a[("rebalancer.Weights", "method")] == ("rebalancer.Weights", "method", "identity")
    assert len(a) == 3


def test_totality_and_rebuild_idempotence(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_entity(conn, 1, 1, "Kalman filter", "method")
    _seed_entity(conn, 1, 1, "transfer function", "concept")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    first = _aliases(conn)
    n_entities = conn.execute(
        "SELECT COUNT(*) FROM (SELECT DISTINCT name, type FROM entities)"
    ).fetchone()[0]
    assert len(first) == n_entities  # total mapping
    al.build_aliases(conn, use_llm=False)
    assert _aliases(conn) == first  # delete + recompute is idempotent


def test_pick_canonical_prefers_doc_freq_then_shortest(conn):
    nodes = [
        al._Node("Long Name Surface", "concept", docs={1}),
        al._Node("Short", "concept", docs={1, 2}),
        al._Node("Other", "concept", docs={1, 2}),
    ]
    # doc_freq wins first; among ties, alphabetical after length ("Other" vs "Short": same
    # length -> alpha).
    assert al.pick_canonical_idx([0, 1, 2], nodes) == 2


# --- LLM path with injected fake client --------------------------------------------------------


class _FakeRunner:
    """Fake `claude -p` runner: returns a fixed verdict as JSON text (no subprocess)."""

    def __init__(self, verdict: dict):
        self._text = json.dumps(verdict)
        self.calls = 0

    def __call__(self, prompt: str, model: str | None) -> str:
        self.calls += 1
        return self._text


def _seed_fuzzy_pair(conn):
    """Two cross-type surfaces of one concept, in different sections (no co-occurrence)."""
    _seed_doc(conn, 1, title="Signals Notes")
    _seed_section(conn, 1, 1)
    _seed_doc(conn, 2, title="PDE Notes")
    _seed_section(conn, 2, 2)
    _seed_entity(conn, 1, 1, "Fourier transform", "concept")
    _seed_entity(conn, 2, 2, "fourier transform", "method")
    conn.commit()


def _close_embed(names: list[str]) -> list[list[float]]:
    """Embed near-identical strings identically: same casefolded text -> same vector."""
    out = []
    seen: dict[str, int] = {}
    for n in names:
        key = n.lower()
        if key not in seen:
            seen[key] = len(seen)
        v = [0.0] * 768
        v[seen[key]] = 1.0
        out.append(v)
    return out


def test_llm_merges_cross_type_variants(conn):
    _seed_fuzzy_pair(conn)
    fake = _FakeRunner({
        "groups": [{
            "member_indices": [0, 1],
            "canonical_name": "Fourier transform",
            "canonical_type": "concept",
        }]
    })
    report = al.build_aliases(conn, use_llm=True, runner=fake, model="fake", embed_fn=_close_embed)
    a = _aliases(conn)
    assert a[("fourier transform", "method")] == ("Fourier transform", "concept", "llm")
    assert a[("Fourier transform", "concept")][0] == "Fourier transform"
    assert report.llm_calls == 1 and fake.calls == 1


def test_llm_verdict_cached_in_pass_cache(conn):
    _seed_fuzzy_pair(conn)
    verdict = {
        "groups": [{
            "member_indices": [0, 1],
            "canonical_name": "Fourier transform",
            "canonical_type": "concept",
        }]
    }
    fake = _FakeRunner(verdict)
    al.build_aliases(conn, use_llm=True, runner=fake, model="fake", embed_fn=_close_embed)
    assert fake.calls == 1
    # Second run: verdict comes from pass_cache, no API call.
    fake2 = _FakeRunner(verdict)
    report = al.build_aliases(conn, use_llm=True, runner=fake2, model="fake", embed_fn=_close_embed)
    assert fake2.calls == 0
    assert report.cache_hits == 1 and report.llm_calls == 0
    assert _aliases(conn)[("fourier transform", "method")][0] == "Fourier transform"


def test_token_jaccard_guard_blocks_theme_mates(conn):
    # "Kalman filter" vs "particle filter": embedder pulls them together (faked at cos 1.0)
    # but token Jaccard is 1/3 < 0.34 -> never even becomes an LLM candidate.
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_section(conn, 2, 1, 1)
    _seed_entity(conn, 1, 1, "Kalman filter", "method")
    _seed_entity(conn, 1, 2, "particle filter", "method")
    conn.commit()
    fake = _FakeRunner({"groups": []})

    def both_close(names):
        return [[1.0] + [0.0] * 767 for _ in names]

    report = al.build_aliases(conn, use_llm=True, runner=fake, model="fake", embed_fn=both_close)
    assert report.llm_candidate_clusters == 0 and fake.calls == 0
    a = _aliases(conn)
    assert a[("Kalman filter", "method")][0] == "Kalman filter"
    assert a[("particle filter", "method")][0] == "particle filter"


def test_cooccurrence_guard_overrides_llm(conn):
    # Both surfaces in the SAME section -> the author treats them as distinct; a fake
    # verdict trying to merge them must be split back. ("extended Kalman filter" shares
    # 2/3 tokens with "Kalman filter", so it passes the token guard and reaches the LLM.)
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_entity(conn, 1, 1, "Kalman filter", "method")
    _seed_entity(conn, 1, 1, "extended Kalman filter", "method")
    conn.commit()
    fake = _FakeRunner({
        "groups": [{
            "member_indices": [0, 1],
            "canonical_name": "Kalman filter",
            "canonical_type": "method",
        }]
    })

    def both_close(names):
        return [[1.0] + [0.0] * 767 for _ in names]

    report = al.build_aliases(conn, use_llm=True, runner=fake, model="fake", embed_fn=both_close)
    a = _aliases(conn)
    assert a[("Kalman filter", "method")][0] == "Kalman filter"
    assert a[("extended Kalman filter", "method")][0] == "extended Kalman filter"
    assert report.guard_splits >= 1


def test_short_name_guard_overrides_llm(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_doc(conn, 2)
    _seed_section(conn, 2, 2)
    _seed_entity(conn, 1, 1, "VaR", "metric")
    _seed_entity(conn, 2, 2, "Vary", "metric")  # 4 chars, close-ish string
    conn.commit()
    fake = _FakeRunner({
        "groups": [{
            "member_indices": [0, 1],
            "canonical_name": "VaR",
            "canonical_type": "metric",
        }]
    })

    def both_close(names):
        return [[1.0] + [0.0] * 767 for _ in names]

    al.build_aliases(conn, use_llm=True, runner=fake, model="fake", embed_fn=both_close)
    a = _aliases(conn)
    # "VaR" is < min_merge_len -> the group is rejected, both stay identity.
    assert a[("VaR", "metric")][0] == "VaR"
    assert a[("Vary", "metric")][0] == "Vary"


def test_invented_canonical_snaps_to_member(conn):
    _seed_fuzzy_pair(conn)
    fake = _FakeRunner({
        "groups": [{
            "member_indices": [0, 1],
            "canonical_name": "The Fourier Transform (invented)",
            "canonical_type": "concept",
        }]
    })
    al.build_aliases(conn, use_llm=True, runner=fake, model="fake", embed_fn=_close_embed)
    a = _aliases(conn)
    canon = a[("fourier transform", "method")][0]
    # Never an invented surface: the canonical is one of the actual members.
    assert canon in {"Fourier transform", "fourier transform"}


def test_oversize_cluster_skipped(conn, monkeypatch):
    _seed_doc(conn, 1)
    for i in range(1, 12):
        _seed_section(conn, i, 1, i - 1)
        _seed_entity(conn, 1, i, f"thing variant number {i}", "concept")
    conn.commit()

    def all_same(names):
        return [[1.0] + [0.0] * 767 for _ in names]

    fake = _FakeRunner({"groups": []})
    report = al.build_aliases(conn, use_llm=True, runner=fake, model="fake", embed_fn=all_same)
    assert report.oversize_skipped == 1
    assert fake.calls == 0  # never sent to the LLM


# --- related documents ---------------------------------------------------------------------------


def _seed_related_corpus(conn):
    """Three docs: 1 and 2 share two canonicals (one via alias variants); 3 shares one
    ubiquitous canonical with everything (the stop-entity). "simulation" not "model":
    short bare lowercase tokens are excluded from the related pool (round 6)."""
    for d in (1, 2, 3):
        _seed_doc(conn, d, title=f"Doc {d}")
        _seed_section(conn, d, d)
    _seed_entity(conn, 1, 1, "Fourier transform")
    _seed_entity(conn, 1, 1, "Laplace transform")
    _seed_entity(conn, 1, 1, "simulation")
    _seed_entity(conn, 2, 2, "fourier transform")  # variant of doc 1's surface
    _seed_entity(conn, 2, 2, "Laplace transform")
    _seed_entity(conn, 2, 2, "simulation")
    _seed_entity(conn, 3, 3, "simulation")
    conn.commit()
    al.build_aliases(conn, use_llm=False)


def test_related_documents_ranked_by_shared_canonicals(conn):
    from locus.link.related import related_documents

    _seed_related_corpus(conn)
    rel = related_documents(conn, 1)
    assert [r.doc_id for r in rel] == [2, 3]
    assert rel[0].shared_count == 3  # fourier + laplace + simulation
    assert rel[1].shared_count == 1  # simulation only
    assert "Laplace transform" in rel[0].shared_names
    # Distinctive names (doc_freq 2) sample before the corpus-wide one ('model', freq 3).
    assert rel[0].shared_names[-1] == "simulation"


def test_related_documents_dedupe_same_name_across_types(conn):
    # Round-5 audit: "LLM" stored under three types must count as ONE shared name,
    # not render as "LLM, LLM, LLM".
    from locus.link.related import related_documents

    for d in (1, 2):
        _seed_doc(conn, d, title=f"Doc {d}")
        _seed_section(conn, d, d)
        _seed_entity(conn, d, d, "LLM", "concept")
        _seed_entity(conn, d, d, "LLM", "method")
        _seed_entity(conn, d, d, "LLM", "tool")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    (rel,) = related_documents(conn, 1)
    assert rel.shared_count == 1
    assert rel.shared_names == ("LLM",)


def test_related_documents_idf_weighting_beats_raw_count(conn):
    # Doc 2 shares TWO corpus-ubiquitous names with doc 1; doc 3 shares ONE rare name.
    # Raw count would rank doc 2 first; IDF weighting ranks the genuine neighbour first.
    from locus.link.related import related_documents

    for d in (1, 2, 3, 4, 5, 6):
        _seed_doc(conn, d, title=f"Doc {d}")
        _seed_section(conn, d, d)
    for d in (1, 2, 4, 5, 6):  # "F1" and "LLM" appear in 5 of 6 docs (generic)
        _seed_entity(conn, d, d, "F1 score", "metric")
        _seed_entity(conn, d, d, "LLM agent", "concept")
    _seed_entity(conn, 1, 1, "Regime-PCMCI", "method")  # rare: only docs 1 and 3
    _seed_entity(conn, 3, 3, "Regime-PCMCI", "method")
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    rel = related_documents(conn, 1)
    # weight(doc 3) = 1/2 = 0.5 > weight(docs 2/4/5/6) = 1/5 + 1/5 = 0.4: the single
    # rare shared name outranks two corpus-ubiquitous ones. Raw counting would put
    # doc 3 last.
    assert rel[0].doc_id == 3
    assert rel[0].shared_names == ("Regime-PCMCI",)


def test_related_documents_stop_entity_guard(conn):
    from locus.link.related import related_documents

    _seed_related_corpus(conn)
    # "simulation" spans 3 docs; with the guard at 2 it stops linking anything.
    rel = related_documents(conn, 1, stop_doc_freq=2)
    assert [r.doc_id for r in rel] == [2]
    assert rel[0].shared_count == 2
    assert "simulation" not in rel[0].shared_names


def test_related_documents_top_n(conn):
    from locus.link.related import related_documents

    _seed_related_corpus(conn)
    assert len(related_documents(conn, 1, top_n=1)) == 1


def test_resolve_stop_doc_freq_scales_and_floors(conn, monkeypatch):
    from locus.link import related as rel_mod

    # ratio x corpus, but off below the small-corpus floor and when ratio <= 0.
    monkeypatch.setattr(rel_mod, "_MIN_CORPUS_FOR_STOP", 3)
    for i in range(1, 6):  # 5 docs
        _seed_doc(conn, i)
    conn.commit()
    assert rel_mod.resolve_stop_doc_freq(conn, 0.4) == 2  # int(0.4 * 5)
    assert rel_mod.resolve_stop_doc_freq(conn, 0.0) is None  # disabled
    monkeypatch.setattr(rel_mod, "_MIN_CORPUS_FOR_STOP", 50)
    assert rel_mod.resolve_stop_doc_freq(conn, 0.4) is None  # below the small-corpus floor


def test_format_related_before_link_run(conn):
    from locus.link.related import format_related

    _seed_doc(conn, 1)
    conn.commit()
    (line,) = format_related(conn, 1)
    assert "locus link" in line  # graceful hint, never an error


# --- typo-class candidate tier (round-5 audit: PCMCI/PCMIC) ---------------------------------------


def _orthogonal_embed(names: list[str]) -> list[list[float]]:
    """Every distinct name embeds orthogonally: NO embedding edges form, so any candidate
    cluster reaching the LLM got there through the typo tier alone."""
    out = []
    seen: dict[str, int] = {}
    for n in names:
        if n not in seen:
            seen[n] = len(seen)
        v = [0.0] * 768
        v[seen[n]] = 1.0
        out.append(v)
    return out


def test_typo_pair_reaches_llm_and_merges(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_doc(conn, 2)
    _seed_section(conn, 2, 2)
    _seed_entity(conn, 1, 1, "PCMCI", "method")
    _seed_entity(conn, 2, 2, "PCMIC", "method")  # the paper's own typo (transposition)
    conn.commit()
    fake = _FakeRunner({
        "groups": [{
            "member_indices": [0, 1],
            "canonical_name": "PCMCI",
            "canonical_type": "method",
        }]
    })
    report = al.build_aliases(conn, use_llm=True, runner=fake, model="fake",
                              embed_fn=_orthogonal_embed)
    assert report.llm_candidate_clusters == 1 and fake.calls == 1
    a = _aliases(conn)
    assert a[("PCMIC", "method")] == ("PCMCI", "method", "llm")


def test_typo_digit_guard_blocks_model_variants(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_section(conn, 2, 1, 1)
    _seed_entity(conn, 1, 1, "MSH(2) models", "other")
    _seed_entity(conn, 1, 2, "MSH(20) models", "other")  # one edit apart, DIFFERENT models
    conn.commit()
    fake = _FakeRunner({"groups": []})
    report = al.build_aliases(conn, use_llm=True, runner=fake, model="fake",
                              embed_fn=_orthogonal_embed)
    assert report.llm_candidate_clusters == 0 and fake.calls == 0
    a = _aliases(conn)
    assert a[("MSH(2) models", "other")][2] == "identity"
    assert a[("MSH(20) models", "other")][2] == "identity"


def test_typo_tier_respects_type_and_length(conn):
    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_section(conn, 2, 1, 1)
    # Same edit distance but different types -> no edge; short names -> no edge.
    _seed_entity(conn, 1, 1, "BinSeg", "method")
    _seed_entity(conn, 1, 2, "BiaSeg", "tool")
    _seed_entity(conn, 1, 1, "VaR", "metric")
    _seed_entity(conn, 1, 2, "VeR", "metric")
    conn.commit()
    fake = _FakeRunner({"groups": []})
    report = al.build_aliases(conn, use_llm=True, runner=fake, model="fake",
                              embed_fn=_orthogonal_embed)
    assert report.llm_candidate_clusters == 0 and fake.calls == 0


def test_related_documents_filter_bare_identifiers(conn):
    # Round-6/round-7 audits: code repos linked on scaffolding identifiers every project of
    # a stack defines. Excluded classes: short bare tokens ("main", "run"), privates/dunders
    # ("_cand"), test scaffolding ("test_*"), and framework boilerplate by name — the Alembic
    # env API ("upgrade", "run_migrations_offline"), which round 6 had wrongly kept as
    # "distinctive". A DISTINCTIVE shared identifier ("implied_vol_from_mid") is a real
    # cross-project link and survives — that is the whole point of the layer.
    from locus.link.related import related_documents

    for d in (1, 2):
        _seed_doc(conn, d, source_type="code", title=f"Repo {d}")
        _seed_section(conn, d, d)
        _seed_entity(conn, d, d, "main", "method")  # short bare token
        _seed_entity(conn, d, d, "run", "method")  # short bare token
        _seed_entity(conn, d, d, "upgrade", "method")  # Alembic env API (no '_' to catch it)
        _seed_entity(conn, d, d, "run_migrations_offline", "method")  # Alembic env API
        _seed_entity(conn, d, d, "_cand", "method")  # private (leading underscore)
        _seed_entity(conn, d, d, "test_signal", "method")  # test scaffolding
        _seed_entity(conn, d, d, "implied_vol_from_mid", "method")  # distinctive — survives
    conn.commit()
    al.build_aliases(conn, use_llm=False)
    (rel,) = related_documents(conn, 1)
    assert rel.shared_count == 1
    assert rel.shared_names == ("implied_vol_from_mid",)


# --- API throttle ----------------------------------------------------------------------------------


def test_throttle_spaces_api_calls_but_not_cache_hits(conn, monkeypatch):
    # Two fuzzy clusters -> two API calls on the first build: exactly ONE sleep (between
    # calls, none before the first). Second build is all cache hits: zero sleeps.
    for d, names in ((1, ("Fourier transform", "Laplace transform")),
                     (2, ("fourier transform", "laplace transform"))):
        _seed_doc(conn, d, title=f"Doc {d}")
        _seed_section(conn, d, d)
        for n in names:
            _seed_entity(conn, d, d, n, "concept" if d == 1 else "method")
    conn.commit()
    sleeps: list[float] = []
    monkeypatch.setattr(al.time, "sleep", lambda s: sleeps.append(s))
    fake = _FakeRunner({"groups": []})
    report = al.build_aliases(conn, use_llm=True, runner=fake, model="fake",
                              embed_fn=_close_embed)
    assert report.llm_calls == 2
    assert len(sleeps) == 1 and sleeps[0] > 0
    sleeps.clear()
    fake2 = _FakeRunner({"groups": []})
    report2 = al.build_aliases(conn, use_llm=True, runner=fake2, model="fake",
                               embed_fn=_close_embed)
    assert report2.cache_hits == 2 and report2.llm_calls == 0
    assert sleeps == []


# --- audit QC + links eval -----------------------------------------------------------------------


def test_alias_qc_reports_substrate(conn):
    from locus.eval.metrics import alias_qc, format_alias_qc

    assert alias_qc(conn) is None  # not built yet
    assert "locus link" in format_alias_qc(None)
    _seed_related_corpus(conn)
    qc = alias_qc(conn)
    assert qc is not None
    assert qc.variants == conn.execute("SELECT COUNT(*) FROM entity_aliases").fetchone()[0]
    assert qc.nontrivial_clusters >= 1  # the fourier casefold pair
    assert qc.cross_doc_canonicals >= 2  # fourier (via alias), laplace, simulation
    assert qc.suspicious_merges == 0  # no llm merges in this fixture
    assert "ALIAS SUBSTRATE" in format_alias_qc(qc)


def test_alias_qc_flags_zero_evidence_llm_merge(conn):
    from locus.eval.metrics import alias_qc

    _seed_doc(conn, 1)
    _seed_section(conn, 1, 1)
    _seed_entity(conn, 1, 1, "spectral density")
    conn.execute(
        "INSERT INTO entity_aliases "
        "(variant_name, variant_type, canonical_name, canonical_type, cluster_id, tier) "
        "VALUES ('spectral density','concept','unrelated surface','concept',1,'llm')"
    )
    conn.commit()
    qc = alias_qc(conn)
    assert qc.suspicious_merges == 1
    assert "spectral density" in qc.suspicious_examples[0]


def test_score_links_requires_substrate_and_finds_pairs(conn):
    from locus.eval.retrieval_eval import score_links

    # Pairs are source_uri substrings (re-curation): _seed_doc sets source_uri = "u{id}".
    lines, agg = score_links(conn, [("u1", "u2")])
    assert agg == {} and "locus link" in lines[0]  # graceful skip pre-substrate
    _seed_related_corpus(conn)
    lines, agg = score_links(conn, [("u1", "u2")])
    assert agg["links_recall"] == 1.0
    lines, agg = score_links(conn, [("u1", "no-such-uri")])
    assert agg["links_recall"] == 0.0


# --- adjudicator unit ---------------------------------------------------------------------------


def test_adjudicate_cluster_parses_json():
    fake = _FakeRunner({
        "groups": [{
            "member_indices": [0],
            "canonical_name": "KL divergence",
            "canonical_type": "concept",
        }]
    })
    v = adjudicate_cluster(
        [ClusterMember("KL divergence", "concept", ("Info Theory",))],
        runner=fake, model="fake",
    )
    assert isinstance(v, AliasVerdict)
    assert v.groups[0].canonical_name == "KL divergence"


def test_adjudicate_cluster_tolerates_prose_around_json():
    # The CLI reply may wrap the object in prose / code fences; we slice first { .. last }.
    def runner(prompt, model):
        return ('Here is the partition:\n```json\n'
                '{"groups": [{"member_indices": [0], "canonical_name": "KL divergence", '
                '"canonical_type": "concept"}]}\n```')

    v = adjudicate_cluster([ClusterMember("KL divergence", "concept")], runner=runner, model="m")
    assert v.groups[0].canonical_name == "KL divergence"


def test_adjudicate_cluster_raises_on_unparseable_reply():
    with pytest.raises(RuntimeError):
        adjudicate_cluster(
            [ClusterMember("x", "concept")],
            runner=lambda prompt, model: "sorry, no json here", model="fake",
        )
