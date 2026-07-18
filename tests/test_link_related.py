"""Related-document ranking — code-symbol segregation (Phase 2, CLAUDE.md §1.2 Link).

Model-free: seeds a tiny corpus and builds the alias substrate deterministically
(`build_aliases(use_llm=False)`), then asserts the ranking contract:

  - A repo's AST identifiers (single-token names that only ever appear in code-file sections)
    are classified as code-symbols and demoted to a downweighted code<->code tiebreak, so they
    never drive a cross-domain link and never displace a shared DOMAIN concept.
  - Domain concepts (multi-word, or attested in a narrative/paper) drive ranking for any pair.
"""

from pathlib import Path

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.link.aliases import build_aliases
from locus.link.related import _CANON_CTE, related_documents


def _seed(db: Path) -> None:
    """Two code repos + one paper.

    repo1 (doc 1) and repo2 (doc 2) share the AST identifier `build_signals` (a code-symbol:
    only ever in a `.py` section of a code doc) AND the concept `mean reversion` (in each
    repo's README). repo3 (doc 3) shares ONLY `build_signals` with repo1. The paper (doc 4)
    shares the concept `mean reversion`. A code-symbol must not out-rank the concept, and the
    paper must link to the repo via the concept (code<->paper concept bridge).
    """
    migrate(db)
    conn = get_connection(db)
    with conn:
        docs = [
            ("h1", "code", "locusdrop:repo1", "Repo One"),
            ("h2", "code", "locusdrop:repo2", "Repo Two"),
            ("h3", "code", "locusdrop:repo3", "Repo Three"),
            ("h4", "pdf", "2500.00001v1", "A Mean Reversion Paper"),
        ]
        for h, st, uri, title in docs:
            conn.execute(
                "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
                "ingest_model, thesis, method, result, limitations) VALUES (?,?,?,?,?,?,?,?,?,?)",
                (h, st, uri, f"{h}.x", title, "t", "x", "x", "x", "x"),
            )
        # sections: code repos get a code-file section + a README; the paper gets a plain one.
        # (doc_id, position, title, file_path)
        sections = [
            (1, 0, "pipe.py", "pipe.py"),
            (1, 1, "README.md", "README.md"),
            (2, 0, "pipe.py", "pipe.py"),
            (2, 1, "README.md", "README.md"),
            (3, 0, "pipe.py", "pipe.py"),
            (4, 0, "body", None),
        ]
        for doc_id, pos, title, fp in sections:
            conn.execute(
                "INSERT INTO sections (doc_id, position, title, summary, file_path) VALUES (?,?,?,?,?)",
                (doc_id, pos, title, "s", fp),
            )
        # entities: build_signals only in .py sections of code docs (=> code-symbol);
        # 'mean reversion' in READMEs + the paper (=> concept). section ids are 1..6 in order.
        ents = [
            (1, 1, "build_signals", "method"),     # repo1 code identifier
            (1, 2, "mean reversion", "concept"),   # repo1 concept (README)
            (2, 3, "build_signals", "method"),     # repo2 code identifier
            (2, 4, "mean reversion", "concept"),   # repo2 concept (README)
            (3, 5, "build_signals", "method"),     # repo3 shares ONLY the identifier
            (4, 6, "mean reversion", "concept"),   # the paper's concept
        ]
        for doc_id, sec_id, name, typ in ents:
            conn.execute(
                "INSERT INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
                (doc_id, sec_id, name, typ),
            )
    conn.close()


def test_code_symbol_classified_and_concept_not(tmp_path):
    db = tmp_path / "locus.db"
    _seed(db)
    conn = get_connection(db)
    try:
        build_aliases(conn, use_llm=False, use_cache=False, log=lambda _m: None)
        is_sym = lambda n: conn.execute(
            f"WITH {_CANON_CTE} SELECT 1 FROM code_symbols WHERE canonical_name=?", (n,)
        ).fetchone() is not None
        assert is_sym("build_signals")        # single-token, only in .py sections of code docs
        assert not is_sym("mean reversion")   # multi-word + attested in README/paper narrative
    finally:
        conn.close()


def test_concept_outranks_code_symbol_and_bridges_to_paper(tmp_path):
    db = tmp_path / "locus.db"
    _seed(db)
    conn = get_connection(db)
    try:
        build_aliases(conn, use_llm=False, use_cache=False, log=lambda _m: None)
        related = related_documents(conn, 1, top_n=5)  # repo1
        order = [r.doc_id for r in related]

        # The paper (4) links to the repo via the shared concept — a code identifier could not
        # bridge code<->paper, but the concept does.
        assert 4 in order
        # repo2 (shares the concept) must out-rank repo3 (shares only the code identifier).
        assert order.index(2) < order.index(3)
        # And the concept-shared neighbours lead: the displayed shared term for repo2 is the
        # concept, not the code identifier.
        repo2 = next(r for r in related if r.doc_id == 2)
        assert "mean reversion" in repo2.shared_names
    finally:
        conn.close()
