"""Phase-4 step 2: the discovery engine (harvest -> embed -> rank).

Model-free and network-free — a fake fetcher and deterministic unit vectors instead of Ollama.
The assertions are the design decisions, not the plumbing:

  - the outbound query can carry a category and NOTHING else (the egress guard);
  - familiarity is SUBTRACTED, so a paper he effectively already has loses to one he does not.
    That single sign is the difference between a discovery engine and a similarity search that
    keeps returning more of what the corpus already contains.
"""

from __future__ import annotations

import math
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.discover import arxiv, rank
from locus.discover.profiles import vec_blob

DIM = 768


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "discovery.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


@pytest.fixture(autouse=True)
def _geometry_only(monkeypatch):
    """Disable the cross-encoder for the vector-geometry tests.

    These build synthetic unit vectors with placeholder text, so a cross-encoder scoring the pair
    would be reading "A Paper" against "a project" — arbitrary numbers that swamp the geometric
    differences under test. The reranker gets its own tests below; here the subject is the cosine
    layer, and mixing the two would test neither.
    """
    import locus.discover.rank as R

    monkeypatch.setattr(R, "_cross_scores", lambda pairs: [])


def unit(*, axis: int) -> list[float]:
    """A unit basis vector — orthogonal to any other axis, so cosine is exactly 0 or 1."""
    v = [0.0] * DIM
    v[axis] = 1.0
    return v


def blend(a: int, b: int, w: float) -> list[float]:
    """Unit vector w of the way from axis `a` toward axis `b`."""
    v = [0.0] * DIM
    v[a], v[b] = math.cos(w * math.pi / 2), math.sin(w * math.pi / 2)
    return v


# ---------- the egress guard ----------


@pytest.mark.parametrize("bad", [
    "market regime detection",          # a gap concept
    "Optibook",                         # a project name
    "cat:q-fin.PM OR all:kalman",       # an injected free-text clause
    "q-fin.PM; drop",
    "",
])
def test_only_category_tokens_can_be_sent(bad):
    """Corpus vocabulary must not be able to reach the wire, even by mistake."""
    with pytest.raises(ValueError):
        arxiv.build_query([bad])


def test_real_categories_are_accepted():
    url = arxiv.build_query(["q-fin.PM", "stat.ML"], start=0, page=50)
    assert "cat%3Aq-fin.PM" in url and "cat%3Astat.ML" in url


def test_query_carries_no_corpus_text():
    url = arxiv.build_query(arxiv.DEFAULT_CATEGORIES)
    # Everything after the endpoint is categories, paging and sort order.
    assert "search_query" in url and "max_results" in url
    for leak in ("regime", "kalman", "portfolio", "Optibook"):
        assert leak.lower() not in url.lower()


# ---------- parsing ----------


_FEED = """<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"
      xmlns:arxiv="http://arxiv.org/schemas/atom">
  <entry>
    <id>http://arxiv.org/abs/2607.12345v2</id>
    <title>Sticky State Hidden Markov Models</title>
    <summary>We propose a prior that reduces state persistence in HMMs.</summary>
    <published>2026-07-30T10:00:00Z</published>
    <author><name>A Researcher</name></author>
    <author><name>B Coauthor</name></author>
    <arxiv:primary_category term="stat.ML"/>
    <category term="stat.ML"/>
    <category term="q-fin.ST"/>
    <link title="pdf" href="http://arxiv.org/pdf/2607.12345v2"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2607.00001v1</id>
    <title>No Abstract Here</title>
    <published>2026-07-29T10:00:00Z</published>
  </entry>
</feed>"""


def test_parse_extracts_what_ranking_needs_and_skips_the_unusable():
    papers = arxiv.parse(_FEED)
    assert len(papers) == 1, "an entry with no abstract cannot be ranked — skip, never guess"
    p = papers[0]
    assert p.external_id == "arxiv:2607.12345"   # version suffix stripped for stable identity
    assert p.authors == "A Researcher, B Coauthor"
    assert p.primary_category == "stat.ML"
    assert p.pdf_url.endswith("2607.12345v2")


def test_malformed_xml_raises_rather_than_returning_nothing():
    with pytest.raises(RuntimeError):
        arxiv.parse("<feed><entry>")


def test_harvest_queries_each_category_separately_and_dedupes():
    """One query per category, not one OR-query.

    Measured on the first live harvest: a single OR-query for 200 papers returned 46 stat.ML and
    exactly 2 q-fin.PM, because arXiv sorts the union by date and stat.ML outpublishes q-fin by
    roughly an order of magnitude. The pool, not the score, was the problem.
    """
    urls: list[str] = []

    def fetch(url):
        urls.append(url)
        return _FEED

    papers = arxiv.harvest(["q-fin.PM", "stat.ML"], per_category=25, fetch=fetch, pause_s=0)
    assert len(urls) == 2, "each category must get its own quota"
    assert "cat%3Aq-fin.PM" in urls[0] and "cat%3Astat.ML" in urls[1]
    assert " OR " not in urls[0]
    # The same paper appearing in two categories is stored once.
    assert [p.external_id for p in papers] == ["arxiv:2607.12345"]


# ---------- the score ----------


def test_cosine_conversion_is_exact_for_unit_vectors():
    assert rank.cos_from_l2(0.0) == pytest.approx(1.0)          # identical
    assert rank.cos_from_l2(math.sqrt(2)) == pytest.approx(0.0)  # orthogonal


def _seed_corpus_section(conn, vec, title="Existing Paper") -> int:
    """A document already in his corpus. Returns its doc id."""
    conn.execute(
        "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
        "ingest_model) VALUES (?,'pdf',?,'r',?,'test')",
        (f"h{title}", f"u{title}", title),
    )
    doc_id = conn.execute("SELECT last_insert_rowid() i").fetchone()["i"]
    conn.execute(
        "INSERT INTO sections (doc_id, position, title, summary) VALUES (?,0,'S','summary')",
        (doc_id,),
    )
    sec_id = conn.execute("SELECT last_insert_rowid() i").fetchone()["i"]
    conn.execute(
        "INSERT INTO section_vectors (section_id, embedding) VALUES (?,?)",
        (sec_id, vec_blob(vec)),
    )
    conn.commit()
    return doc_id


def _seed_profile(conn, label, vec, kind="project", doc_ids=()):
    import json

    conn.execute(
        "INSERT INTO discovery_profiles (subject_kind, subject_key, label, text, doc_ids, "
        "built_at) VALUES (?,?,?,?,?,'2026-07-31')",
        (kind, label, label, label, json.dumps(list(doc_ids))),
    )
    pid = conn.execute("SELECT last_insert_rowid() i").fetchone()["i"]
    conn.execute(
        "INSERT INTO discovery_profile_vectors (profile_id, embedding) VALUES (?,?)",
        (pid, vec_blob(vec)),
    )
    conn.commit()


def _seed_candidate(conn, ext, title, vec):
    conn.execute(
        "INSERT INTO discovery_candidates (external_id, dedupe_key, title, abstract, "
        "harvested_at, embedded) VALUES (?,?,?,'abs','2026-07-31',1)",
        (ext, title.lower(), title),
    )
    cid = conn.execute("SELECT last_insert_rowid() i").fetchone()["i"]
    conn.execute(
        "INSERT INTO discovery_vectors (candidate_id, embedding) VALUES (?,?)",
        (cid, vec_blob(vec)),
    )
    conn.commit()


def test_familiarity_is_subtracted_so_known_material_loses(conn):
    """THE design decision: relevant-and-new must beat relevant-and-already-owned.

    Both candidates sit at the SAME distance from the project, so fit alone cannot separate them.
    Only the familiarity term can, and it must pick the one the corpus does not already cover.
    """
    _seed_profile(conn, "regime detection project", unit(axis=0))
    # A paper he already owns: near the project, but off to one side of it.
    _seed_corpus_section(conn, blend(0, 1, 0.2))

    # Identical to the paper he already has.
    _seed_candidate(conn, "arxiv:known", "More Of What I Have", blend(0, 1, 0.2))
    # Equally close to the project, but in a direction nothing in the corpus occupies.
    _seed_candidate(conn, "arxiv:novel", "A Method From Elsewhere", blend(0, 2, 0.2))

    top = rank.rank(conn, limit=5)
    by_id = {s.external_id: s for s in top}
    assert by_id["arxiv:known"].fit == pytest.approx(by_id["arxiv:novel"].fit, abs=1e-3), \
        "the test is only meaningful if fit cannot separate them"
    assert by_id["arxiv:known"].familiarity > by_id["arxiv:novel"].familiarity
    assert top[0].external_id == "arxiv:novel", \
        "a paper he effectively already owns must not outrank a genuinely new one"


def test_a_projects_own_writeups_do_not_count_as_already_having_it(conn):
    """Without this exclusion every score collapses to zero and the ranking becomes noise.

    A project profile is built FROM his write-ups, and those write-ups are in the corpus — so the
    nearest existing material to any candidate matching the project is that same write-up.
    Familiarity would equal fit for every candidate alike.
    """
    own_doc = _seed_corpus_section(conn, unit(axis=0), title="My Own Write-Up")
    _seed_profile(conn, "regime detection project", unit(axis=0), doc_ids=[own_doc])
    _seed_candidate(conn, "arxiv:1", "Relevant Paper", unit(axis=0))

    scored = rank.rank(conn, limit=5)[0]
    assert scored.familiarity == pytest.approx(0.0, abs=1e-6), \
        "his own description of the problem is not the corpus already covering it"


def test_novelty_cannot_promote_an_irrelevant_paper(conn):
    """Being unlike everything he owns is only a virtue in something already relevant.

    The second live run failed exactly here: with novelty as a co-equal term, papers on ocean
    circulation and religious-radio transcripts topped the list because they were unlike anything
    in the corpus — including unlike his projects. Relevance has to gate.
    """
    _seed_profile(conn, "regime detection", unit(axis=0))
    _seed_corpus_section(conn, blend(0, 1, 0.1))          # material he already has, near-ish
    _seed_candidate(conn, "arxiv:relevant", "On Regimes", blend(0, 3, 0.15))
    # Maximally novel and maximally irrelevant: orthogonal to the project and to the corpus.
    for i in range(8):
        _seed_candidate(conn, f"arxiv:noise{i}", f"Unrelated {i}", unit(axis=20 + i))

    top = rank.rank(conn, limit=3)
    assert top[0].external_id == "arxiv:relevant", \
        "an unrelated paper must not win on being unfamiliar"


def test_one_broad_profile_cannot_monopolise_the_list(conn):
    """Measured on the first live run: one generic profile took 8 of the top 12 slots."""
    _seed_profile(conn, "broad project", unit(axis=0))
    _seed_profile(conn, "narrow project", unit(axis=7))
    for i in range(6):
        _seed_candidate(conn, f"arxiv:broad{i}", f"Broad {i}", blend(0, 9, 0.02 * i))
    _seed_candidate(conn, "arxiv:narrow", "Narrow", unit(axis=7))

    top = rank.rank(conn, limit=6)
    from collections import Counter
    counts = Counter(s.matched_label for s in top)
    assert counts["broad project"] <= 2, "the list must span his work, not one attractor"
    assert "narrow project" in counts


def test_gap_matches_count_less_than_project_matches(conn):
    _seed_profile(conn, "a project", unit(axis=1), kind="project")
    _seed_profile(conn, "a gap concept", unit(axis=2), kind="gap")
    _seed_candidate(conn, "arxiv:proj", "Project Match", unit(axis=1))
    _seed_candidate(conn, "arxiv:gap", "Gap Match", unit(axis=2))

    top = {s.external_id: s for s in rank.rank(conn, limit=5)}
    assert top["arxiv:proj"].fit > top["arxiv:gap"].fit
    assert top["arxiv:gap"].fit == pytest.approx(rank.GAP_WEIGHT, abs=1e-3)


def test_every_ranked_candidate_carries_a_grounded_why(conn):
    _seed_profile(conn, "regime detection", unit(axis=0))
    _seed_candidate(conn, "arxiv:1", "Something", blend(0, 4, 0.2))
    for s in rank.rank(conn, limit=5):
        assert "regime detection" in s.why and s.matched_kind == "project"


def test_ranking_without_profiles_is_silent_not_arbitrary(conn):
    _seed_candidate(conn, "arxiv:1", "Orphan", unit(axis=0))
    assert rank.rank(conn) == [], "with nothing to rank against, propose nothing"


# ---------- storage ----------


def test_store_is_idempotent_by_external_id(conn):
    papers = arxiv.parse(_FEED)
    assert rank.store(conn, papers) == 1
    assert rank.store(conn, papers) == 0


def test_embed_pending_only_embeds_new_rows(conn):
    rank.store(conn, arxiv.parse(_FEED))
    calls: list[list[str]] = []

    def fake_embed(texts):
        calls.append(texts)
        return [unit(axis=3) for _ in texts]

    assert rank.embed_pending(conn, embed_fn=fake_embed) == 1
    assert rank.embed_pending(conn, embed_fn=fake_embed) == 0
    assert len(calls) == 1
    # The title is embedded with the abstract: the method name often lives there.
    assert "Sticky State Hidden Markov Models" in calls[0][0]


def test_candidates_are_not_documents(conn):
    """Structural, not a config flag: no retrieval arm can reach a third-party abstract."""
    rank.store(conn, arxiv.parse(_FEED))
    assert conn.execute("SELECT COUNT(*) n FROM documents").fetchone()["n"] == 0


def test_bare_acronyms_are_not_embedded_as_gaps():
    """`AIS` embeds essentially as "AI" — measured, it matched an AI-strategy paper."""
    from locus.discover.profiles import is_bare_acronym

    for ticker in ("AIS", "AEX", "DAX", "GMV", "TTF"):
        assert is_bare_acronym(ticker)
    for real in ("Kalman filter", "Modern Portfolio Theory", "regime detection", "HMM tuning"):
        assert not is_bare_acronym(real)


def test_a_project_gets_many_facets_not_one_summary(conn):
    """One vector per project matched relevant work by luck; the specifics live below the pitch."""
    from locus.agent import state
    from locus.discover.profiles import _project_facets

    doc = _seed_corpus_section(conn, unit(axis=0), title="regime-ml")
    conn.execute(
        "UPDATE documents SET thesis='detects market regimes', method='hidden markov models', "
        "result='sharpe improved', limitations='regime lag' WHERE id=?", (doc,))
    for i in range(4):
        conn.execute(
            "INSERT INTO sections (doc_id, position, title, summary) VALUES (?,?,?,?)",
            (doc, i + 1, f"mod{i}", "tuning state persistence so regimes do not flicker " * 4),
        )
    for name in ("Kalman filter", "Viterbi", "Baum-Welch", "HMM", "regime detection"):
        conn.execute(
            "INSERT INTO entities (doc_id, name, type) VALUES (?,?,'method')", (doc, name))
    obj, _ = state.upsert_object(conn, type_="project", title="regime-ml", body={})
    state.add_links(conn, obj, [state.ObjectLink("doc", "uregime-ml", "implements")])
    conn.commit()

    facets, doc_ids = _project_facets(conn, obj, "regime-ml")
    kinds = {f for f, _ in facets}
    assert "synthesis" in kinds and "concepts" in kinds
    assert sum(1 for f, _ in facets if f.startswith("section:")) >= 4
    # `result` and `limitations` were previously dropped on the floor.
    synthesis = next(t for f, t in facets if f == "synthesis")
    assert "sharpe improved" in synthesis and "regime lag" in synthesis
    # The whole project is now represented by far more than its pitch.
    assert sum(len(t) for _, t in facets) > 5 * len(synthesis)


# ---------- open problems, harvest window, cross-encoder ----------


def test_open_threads_become_their_own_facets(conn):
    """An open problem is the most discriminative query a project has, and it was unused.

    Everything else in a profile describes what the project IS, and that retrieves more
    descriptions of the same thing. "Whether rebalancing heuristic is overfit to training data"
    describes what he NEEDS, which is what a method paper supplies.
    """
    import json

    from locus.discover.profiles import _open_problem_facets

    conn.execute(
        "INSERT INTO objects (type, title, status, body, created_at, updated_at) "
        "VALUES ('project','Alpha Fund','active',?,'2026-07-31','2026-07-31')",
        (json.dumps({
            "open_threads": [
                "Out-of-sample persistence of cascade mean-reversion patterns",
                "short",                       # a stub, not a problem statement
            ],
            "learnings": ["cascades cluster near close"],
            "approach": "detect stop-loss cascades from order book imbalance data",
        }),),
    )
    conn.commit()
    oid = conn.execute("SELECT id FROM objects").fetchone()["id"]

    facets = dict(_open_problem_facets(conn, oid, "Alpha Fund"))
    assert "thread:0" in facets and "out-of-sample persistence" in facets["thread:0"].lower()
    assert "thread:1" not in facets, "a stub thread is not a problem statement"
    assert "learnings" in facets and "approach" in facets
    # Each thread is its own vector: averaging unrelated problems describes none of them.
    assert facets["thread:0"].startswith("Alpha Fund.")


def test_harvest_stops_at_the_date_cutoff():
    """Coverage should be a time window, not an accident of publication volume."""
    feed = _FEED.replace("2026-07-30T10:00:00Z", "2026-06-01T10:00:00Z")
    calls: list[str] = []

    def fetch(url):
        calls.append(url)
        return feed

    assert arxiv.harvest(["q-fin.PM"], per_category=200, since="2026-07-01",
                         fetch=fetch, pause_s=0) == []
    assert len(calls) == 1, "must stop paging once it reaches papers older than the cutoff"


def test_ranking_survives_the_reranker_being_absent(conn, monkeypatch):
    """The `rerank` extra is optional; without it the cosine ordering must still stand."""
    import locus.discover.rank as R

    _seed_profile(conn, "a project", unit(axis=0))
    _seed_candidate(conn, "arxiv:1", "A Paper", unit(axis=0))
    monkeypatch.setattr(R, "_cross_scores", lambda pairs: [])

    top = R.rank(conn, limit=3)
    assert len(top) == 1 and top[0].cross_score is None
    assert top[0].score == pytest.approx(top[0].fit, abs=1e-6)


def test_the_cross_encoder_decides_the_order_when_available(conn, monkeypatch):
    """The second stage every corpus query already runs through, finally applied to discovery.

    Bi-encoder cosine cannot separate "this method applies to that problem" from "these texts
    share vocabulary" — that is what `retrieve/rerank.py` exists for, and discovery was running
    without it.
    """
    import locus.discover.rank as R

    _seed_profile(conn, "a project", unit(axis=0))
    _seed_candidate(conn, "arxiv:near", "Cosine Favourite", unit(axis=0))
    _seed_candidate(conn, "arxiv:far", "Cross-Encoder Favourite", blend(0, 6, 0.15))

    # The reranker disagrees with the cosine ordering, and it must win.
    monkeypatch.setattr(
        R, "_cross_scores",
        lambda pairs: [5.0 if "Cross-Encoder" in t else -5.0 for _q, t in pairs],
    )
    top = R.rank(conn, limit=2)
    assert top[0].external_id == "arxiv:far"
    assert top[0].cross_score == 5.0


# ---------- literature search ----------


def test_search_is_relevance_sorted_not_date_sorted():
    """Methods are old. A date-sorted feed can never reach the canonical treatment.

    Measured: a relevance search for regime switching returns work from 2008, 2014 and 2020 —
    none of which a recency browse could ever surface.
    """
    url = arxiv.build_search("regime switching")
    assert "sortBy=relevance" in url and "submittedDate" not in url


def test_search_falls_back_from_exact_phrase_to_content_words():
    """An exact phrase that finds nothing means nobody phrased it his way, not that nobody works
    on it: 18 book-derived phrases returned 9 papers quoted, 79 with the fallback."""
    urls: list[str] = []

    def fetch(url):
        urls.append(url)
        return "" if len(urls) == 1 else _FEED   # quoted attempt returns nothing

    from locus.discover.queries import SearchTerm

    got = arxiv.search([SearchTerm("liquidity-aware portfolio optimization", "reading", "book")],
                       fetch=fetch, pause_s=0)
    assert len(urls) == 2, "must retry with the loose query"
    assert "%22" in urls[0], "first attempt is the quoted phrase"
    assert "AND" in urls[1], "fallback is an AND of content words"
    assert got and got[0][1].term == "liquidity-aware portfolio optimization"


def test_search_terms_reject_document_internal_names():
    """`Procedure 6.2 ...` and `IS/OOS split at 2019-01-01` are real entities and useless queries."""
    from locus.discover.queries import _usable

    for bad in ("Procedure 6.2 Sizing alphas into positions", "IS/OOS split at 2019-01-01",
                "get_positions", "CAC 40", "AEX", "Beta", "stock return = market + idio"):
        assert not _usable(bad, set()), bad
    for good in ("liquidity-aware portfolio optimization", "Marginal Contribution to Factor Risk",
                 "walk-forward cross-validation", "Crowding"):
        assert _usable(good, set()), good


def test_the_concept_that_found_a_paper_becomes_its_reason(conn):
    """A real query beats a similarity score, and it is a fact he can check."""
    from locus.discover.queries import SearchTerm

    rank.store(conn, [(arxiv.parse(_FEED)[0],
                       SearchTerm("sticky HMM priors", "reading", "Advanced Portfolio Management"))])
    row = conn.execute("SELECT found_term, found_kind, found_label FROM discovery_candidates").fetchone()
    assert (row["found_term"], row["found_kind"]) == ("sticky HMM priors", "reading")

    s = rank.Scored(1, "arxiv:1", "T", "", "", "", "abs", 0.7, 0.6, 0.1, "project", "P",
                    found_term="sticky HMM priors", found_kind="reading",
                    found_label="Advanced Portfolio Management")
    assert "sticky HMM priors" in s.why and "which you annotated" in s.why


def test_slots_interleave_across_channels(conn):
    """His reading supplies more terms than his projects; strict priority gave it every slot."""
    for i in range(6):
        _seed_profile(conn, f"read{i}", unit(axis=i), kind="project")
        _seed_candidate(conn, f"arxiv:r{i}", f"Read {i}", unit(axis=i))
        conn.execute("UPDATE discovery_candidates SET found_term=?, found_kind='reading' "
                     "WHERE external_id=?", (f"term{i}", f"arxiv:r{i}"))
    _seed_profile(conn, "a project", unit(axis=40))
    _seed_candidate(conn, "arxiv:p", "Project Match", unit(axis=40))
    conn.commit()

    kinds = {s.found_kind for s in rank.rank(conn, limit=8)}
    assert "reading" in kinds and None in kinds, "both channels must be represented"


def test_search_terms_interleave_so_a_truncated_budget_still_covers_projects(conn):
    """Concatenating sources meant projects were NEVER searched.

    The live run took 18 terms off the front of a reading-first list and got 18 reading terms:
    79 papers harvested, all from one book, zero project or gap concepts. A request budget always
    runs out somewhere, so the ordering must be balanced at every prefix, not only at the end.
    """
    import locus.discover.queries as Q

    monkey = {
        "reading_terms": [Q.SearchTerm(f"read {i}", "reading", "book") for i in range(9)],
        "project_terms": [Q.SearchTerm(f"proj {i}", "project", "regime-ml") for i in range(5)],
        "gap_terms": [Q.SearchTerm(f"gap {i}", "gap", "work") for i in range(4)],
    }
    for name, value in monkey.items():
        setattr(Q, name, lambda conn, *, limit=0, _v=value: _v)

    kinds = [t.source_kind for t in Q.all_terms(conn)[:6]]
    assert set(kinds) == {"reading", "project", "gap"}, kinds
    assert kinds[0] == "reading", "reading still leads within each round"


# ---------- OpenAlex ----------


_OA = """{"results":[{
  "id":"https://openalex.org/W123","display_name":"Trading and Exchanges",
  "publication_date":"2002-10-01","cited_by_count":1500,
  "doi":"https://doi.org/10.1234/x",
  "abstract_inverted_index":{"Market":[0],"microstructure":[1],"for":[2],"practitioners":[3],
      "covering":[4],"order":[5],"books":[6],"and":[7],"liquidity":[8],"provision":[9],
      "in":[10],"modern":[11],"electronic":[12],"venues":[13],"worldwide":[14],"today":[15]},
  "primary_location":{"source":{"display_name":"Oxford University Press"}},
  "open_access":{"oa_url":"https://example.org/x.pdf"},
  "authorships":[{"author":{"display_name":"L Harris"}}]},
 {"id":"https://openalex.org/W999","display_name":"No Abstract","cited_by_count":3}]}"""


def test_openalex_reconstructs_inverted_abstracts_and_skips_unusable():
    from locus.discover import openalex

    works = openalex.parse(_OA)
    assert len(works) == 1, "a work with no abstract cannot be ranked — skip it"
    w = works[0]
    assert w.abstract.startswith("Market microstructure for practitioners")
    assert w.cited_by == 1500 and w.venue == "Oxford University Press"
    assert w.pdf_url.endswith(".pdf") and w.external_id == "openalex:W123"


def test_openalex_query_filters_to_works_with_abstracts():
    from locus.discover import openalex

    url = openalex.build_query("market microstructure", mailto="a@b.c")
    assert "has_abstract" in url and "mailto=a%40b.c" in url


def test_citation_bonus_is_monotonic_in_citations():
    """More-cited still ranks above less-cited; centring changes the origin, not the order."""
    low, mid, high = rank._citation_bonuses([10, 1_000, 100_000])
    assert high > mid > low


# ---------- the judge ----------


def test_judge_drops_only_the_floor_and_never_reorders(conn, monkeypatch):
    """A filter, not a ranker: measured, it uses 1-3 of its scale so it cannot order the good."""
    from locus.discover import judge as J

    # One profile each, so the per-profile diversity cap is not what does the filtering.
    for i, name in enumerate(("keep-a", "junk", "keep-b")):
        _seed_profile(conn, f"project {i}", unit(axis=i))
        _seed_candidate(conn, f"arxiv:{name}", name, unit(axis=i))

    scores = {"junk": 1, "keep-a": 3, "keep-b": 2}
    monkeypatch.setattr(
        J, "score",
        lambda items, **kw: [J.Verdict(scores[t], "") for _l, _f, t, _a in items],
    )
    kept = [s.title for s in rank.rank(conn, limit=3, judge={"model": "m", "host": "h"})]
    assert "junk" not in kept and set(kept) == {"keep-a", "keep-b"}


def test_a_broken_judge_passes_everything_through(conn):
    """A judge that cannot answer must degrade to no filtering, never to an empty reading list."""
    from locus.discover import judge as J

    def exploding(prompts):
        raise RuntimeError("ollama down")

    verdicts = J.score([("p", "f", "t", "a")], model="m", host="h",
                       judge_fn=lambda ps: [J.Verdict(3, "x")])
    assert verdicts[0].score == 3
    # A count mismatch is also survivable.
    assert J.score([("p", "f", "t", "a")], model="m", host="h",
                   judge_fn=lambda ps: [])[0].score == 3


# ---------- the annotation sweep ----------


def test_sweep_skips_unread_papers_and_records_folder(conn):
    """A paper still in Proposed has nothing written on it, so it costs no download."""
    from locus.reading import proposals as P
    from locus.reading.sweep import sweep

    P.link_target(conn, source_uri="vault/incoming/paper/x.pdf", doc_uuid="u1",
                  device_path="2026-08-01 X.pdf", linked_by="delivery")

    def runner(args):
        return 0, "[f] Reading/Proposed/2026-08-01 X\n", ""

    out = sweep(conn, runner=runner)
    assert [(r.status, r.folder) for r in out] == [("unread", "Proposed")]
    row = conn.execute("SELECT device_folder, last_swept FROM reading_targets").fetchone()
    assert row["device_folder"] == "Proposed" and row["last_swept"]


def test_sweep_reports_a_reading_that_left_the_folders(conn):
    from locus.reading import proposals as P
    from locus.reading.sweep import sweep

    P.link_target(conn, source_uri="vault/incoming/paper/x.pdf", doc_uuid="u1",
                  device_path="2026-08-01 X.pdf", linked_by="delivery")
    out = sweep(conn, runner=lambda a: (0, "[f] Reading/Finished/2026-08-01 Other\n", ""))
    assert out[0].status == "gone"


def test_only_a_truly_marked_concept_claims_to_be_marked(conn):
    """The `reading` tier must not say "you marked this" — it is almost always false.

    Measured 2026-08-01: the portfolio book carries 1,212 entities against 26 marks, and
    `positive feedback investment strategies` was proposed as "a concept you marked" while
    appearing in none of them. Two tiers now, and only one makes the claim.
    """
    marked = rank.Scored(1, "a", "T", "", "", "", "abs", 0.7, 0.6, 0.1, "project", "P",
                         found_term="factor-mimicking portfolios", found_kind="marked",
                         found_label="Advanced Portfolio Management")
    read = rank.Scored(1, "a", "T", "", "", "", "abs", 0.7, 0.6, 0.1, "project", "P",
                       found_term="dollar volatility", found_kind="reading",
                       found_label="Advanced Portfolio Management")
    assert "underlined" in marked.why
    assert "underlined" not in read.why and "annotated" in read.why


def test_citation_prior_is_centred_so_unknown_is_not_a_penalty(conn):
    """Raw log-citations demoted every arXiv preprint for its SOURCE, not its quality.

    Measured 2026-08-01: the median known count in the live pool was 601, worth +0.42 at the
    configured weight, while arXiv reports no count at all and scored 0.00. Centring maps unknown
    to the middle of the field instead of the bottom of it.
    """
    b = dict(zip(("unknown", "median", "high"),
                 rank._citation_bonuses([None, 100, 100, 100_000])))
    assert b["unknown"] == 0.0, "no citation data must mean no opinion"
    assert b["median"] == pytest.approx(0.0, abs=1e-9), "a median work is the reference point"
    # And the whole term stays small: it orders relevant papers, it cannot promote irrelevant ones.
    assert abs(rank._citation_bonuses([None, 0, 121_773])[2]) < 4.0


def test_openalex_searches_titles_and_abstracts_not_fulltext():
    """`search=` matched 719,908 works for one finance phrase, led by medical guidelines."""
    from locus.discover import openalex

    url = openalex.build_query("Marginal Contribution to Factor Risk")
    assert "title_and_abstract.search" in url
    assert "search=Marginal" not in url, "the loose fulltext parameter must not be used"


def test_openalex_backs_off_on_rate_limiting(monkeypatch):
    """A silently-truncated harvest is the worst failure shape: less coverage, no error."""
    import urllib.error

    from locus.discover import openalex

    calls = {"n": 0}

    def flaky(req, timeout=0):
        calls["n"] += 1
        if calls["n"] < 3:
            raise urllib.error.HTTPError(req.full_url, 429, "Too Many Requests", {}, None)

        class R:
            def read(self):
                return b'{"results":[]}'

            def __enter__(self):
                return self

            def __exit__(self, *a):
                return False

        return R()

    monkeypatch.setattr(openalex.urllib.request, "urlopen", flaky)
    monkeypatch.setattr(openalex.time if hasattr(openalex, "time") else __import__("time"),
                        "sleep", lambda s: None)
    assert openalex._default_fetch("https://x") == '{"results":[]}'
    assert calls["n"] == 3, "must retry through the 429s rather than give up or crash"


# ---------- step 3: citation mining ----------


_WORK = """{"referenced_works": ["https://openalex.org/W1", "https://openalex.org/W2"]}"""
_WORK_B = """{"referenced_works": ["https://openalex.org/W2", "https://openalex.org/W3"]}"""


def test_co_citation_is_distinguished_from_a_single_citation(conn):
    """A work TWO of his sources point at is consensus; one is a suggestion. Different channels."""
    from locus.discover import citations

    seen: list[str] = []

    def fetch(url):
        seen.append(url)
        return _WORK if "1111.11111" in url else _WORK_B

    cited = citations.referenced_works(
        [("arxiv:1111.11111", "Paper A"), ("arxiv:2222.22222", "Paper B")], fetch=fetch
    )
    by_id = {c.work_id: c for c in cited}
    assert by_id["W2"].channel == "co_citation", "cited by both papers"
    assert by_id["W1"].channel == "citation" and by_id["W3"].channel == "citation"
    assert by_id["W2"].citing_titles == ("Paper A", "Paper B")


def test_a_document_with_no_identifier_is_skipped_not_guessed(conn):
    """A title search would attribute someone else's bibliography to his reading."""
    from locus.discover import citations

    conn.execute(
        "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
        "category, ingest_model) VALUES "
        "('h1','pdf','vault/incoming/papers/2605.30363v1.pdf','r','Has ID','paper','t')"
    )
    conn.execute(
        "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
        "category, ingest_model) VALUES "
        "('h2','pdf','vault/incoming/paper/A Book.pdf','r','No ID','paper','t')"
    )
    conn.commit()
    ids = citations.corpus_identifiers(conn)
    # The DataCite DOI is the selector OpenAlex resolves; `arxiv:<id>` 404s (verified live).
    assert ids == [("doi:10.48550/arXiv.2605.30363", "Has ID")]


def test_one_unreadable_bibliography_does_not_abandon_the_rest(conn):
    """A bibliography we cannot read is a coverage gap, never a reason to drop the others."""
    from locus.discover import citations

    def fetch(url):
        if "9999" in url:
            raise OSError("429")
        return _WORK

    cited = citations.referenced_works(
        [("arxiv:9999.99999", "Broken"), ("arxiv:1111.11111", "Fine")], fetch=fetch
    )
    assert {c.work_id for c in cited} == {"W1", "W2"}


def test_referenced_works_are_batched_within_the_api_ceiling():
    from locus.discover import citations

    urls: list[str] = []

    def fetch(url):
        urls.append(url)
        return '{"results":[]}'

    citations.resolve([f"W{i}" for i in range(120)], fetch=fetch, batch=50)
    assert len(urls) == 3, "120 ids must go out as 50 + 50 + 20"
    assert "openalex_id%3AW0%7CW1" in urls[0], "ids are pipe-separated in one filter"


def test_citation_reasons_do_not_pretend_to_be_searches():
    """"cited by 3 papers you keep" is a fact about a reference, not a similarity score."""
    s = rank.Scored(1, "a", "T", "", "", "", "abs", 0.7, 0.6, 0.1, "project", "P",
                    found_term="X", found_kind="co_citation", found_label="3 papers you keep")
    assert s.why == "cited by 3 papers you keep"
    assert "found by searching" not in s.why


def test_the_openalex_key_never_reaches_a_log_or_an_error(monkeypatch):
    """The key travels as a query parameter and HTTPError stringifies the whole URL.

    Failures are exactly when logging happens, so an unredacted message would write the
    credential into the journal on every rate-limited request.
    """
    from locus.discover import openalex

    monkeypatch.setenv("OPENALEX_API_KEY", "SECRET123")
    assert "SECRET123" in openalex.build_query("regime switching")
    leaked = "boom https://api.openalex.org/works?api_key=SECRET123"
    assert openalex.redact(leaked) == "boom https://api.openalex.org/works?api_key=***"
    assert "SECRET123" not in openalex.redact(leaked)
