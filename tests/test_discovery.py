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
