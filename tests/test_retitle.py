"""Corpus-level retitling (locus/retitle.py).

Model-free: the Claude topic distiller is exercised through an injected fake client (the
judge.py / adjudicate.py pattern); the deterministic helpers run as-is.
"""

from pathlib import Path
from types import SimpleNamespace

import pytest

from locus import retitle
from locus.db.connection import get_connection
from locus.db.migrate import migrate


# --- deterministic helpers -----------------------------------------------------------------


@pytest.mark.parametrize("filename,expected", [
    ("P1_ODEs_Lecture6.pdf", "Lecture 6"),
    ("P1-Calc3-L1-Examples.pptx", "Lecture 1 (Examples)"),
    ("Slides lecture 2.pdf", "Lecture 2"),
    ("P1 2-2 Lecture 3.pdf", "Lecture 3"),
    ("Lecture1.ipynb", "Lecture 1"),
    ("Session4.ipynb", "Session 4"),
    ("Week 07 notes.pdf", "Week 7"),
    ("Problem Sheet 2.pdf", "Sheet 2"),
    ("Alec_CV_2026.pdf", None),  # no sequence marker
    ("black_scholes.py", None),
])
def test_parse_sequence(filename, expected):
    assert retitle.parse_sequence(filename) == expected


def test_module_context_skips_buckets_and_years():
    base = "/h/vault/incoming/coursework/oxford-year1/P1 Mathematics/Ordinary Differential Equations 1"
    assert retitle.module_context(f"{base}/P1_ODEs_Lecture1.pdf") == "Ordinary Differential Equations 1"
    # A file sitting directly in a category bucket has no module.
    assert retitle.module_context("vault/incoming/career/Alec_CV_2026.pdf") is None
    # Year folders are buckets too.
    assert retitle.module_context("x/incoming/coursework/oxford-year2/A1 Mathematics/sheet.pdf") == "A1 Mathematics"


@pytest.mark.parametrize("module,seq,topic,expected", [
    ("Calculus 3", "Lecture 1", "Vector Calculus", "Calculus 3 — Lecture 1: Vector Calculus"),
    ("Calculus 3", None, "Vector Calculus", "Calculus 3 — Vector Calculus"),
    (None, "Lecture 5", "Modern Portfolio Theory", "Lecture 5: Modern Portfolio Theory"),
    (None, None, "Attention Is All You Need", "Attention Is All You Need"),
])
def test_compose(module, seq, topic, expected):
    assert retitle.compose(module, seq, topic) == expected


def test_fallback_topic_strips_purpose_preamble():
    t = retitle._fallback_topic("To explain frequency response functions, Fourier series.", "x")
    assert t == "Frequency response functions, Fourier series"


def test_break_collisions_guarantees_uniqueness():
    rep = retitle.RetitleReport()
    # three docs compose to the same title; distinct dates/stems disambiguate
    items = [
        (1, "Same Title", "2024-01-01", "a/x1.pdf"),
        (2, "Same Title", "2024-02-02", "a/x2.pdf"),
        (3, "Same Title", None, "a/x3.pdf"),
        (9, "Unique", "2024-01-01", "a/u.pdf"),
    ]
    out = retitle._break_collisions(items, rep)
    assert out[9] == "Unique"  # singletons untouched
    assert len(set(out.values())) == 4  # all distinct
    assert rep.collisions_broken == 3


# --- integration: build_titles with a fake client ------------------------------------------


class _FakeClient:
    """Anthropic-shaped fake: maps the filename in the user prompt to a fixed verdict."""

    def __init__(self, by_filename: dict[str, dict]):
        self._by_filename = by_filename
        self.calls = 0
        self.messages = self

    def create(self, **kwargs):
        self.calls += 1
        user = kwargs["messages"][0]["content"]
        for fn, verdict in self._by_filename.items():
            if fn in user:
                block = SimpleNamespace(type="tool_use", input=verdict)
                return SimpleNamespace(content=[block])
        raise AssertionError(f"no fake verdict matched prompt: {user[:80]}")


@pytest.fixture()
def conn(tmp_path: Path, monkeypatch):
    # Keep the prior-title backup sidecar inside tmp, never the real vault.
    monkeypatch.setattr(retitle, "_backup_path", lambda: tmp_path / "title_backup.json")
    db = tmp_path / "r.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _seed(conn, doc_id, source_uri, title, thesis, date="2024-11-01"):
    conn.execute(
        "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title,"
        " ingest_model, thesis, method, result, source_date, category)"
        " VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        (doc_id, f"h{doc_id}", "pdf", source_uri, f"p{doc_id}", title, "m",
         thesis, "method", "result", date, "coursework"),
    )


def test_build_titles_composes_and_breaks_collisions(conn):
    base = "/h/vault/incoming/coursework/oxford-year1/P1 Mathematics/Ordinary Differential Equations 1"
    _seed(conn, 1, f"{base}/P1_ODEs_Lecture1.pdf", "P1 ODEs 1", "To solve ODEs.")
    _seed(conn, 2, f"{base}/P1_ODEs_Lecture6.pdf", "P1 ODEs 1", "To explain Fourier series.")
    fake = _FakeClient({
        "Lecture1": {"keep_existing": False, "topic": "Solving ODEs"},
        "Lecture6": {"keep_existing": False, "topic": "Fourier Series"},
    })
    report = retitle.build_titles(conn, use_llm=True, client=fake, log=lambda *_: None)

    titles = {r["id"]: r["title"] for r in conn.execute("SELECT id,title FROM documents")}
    assert titles[1] == "Ordinary Differential Equations 1 — Lecture 1: Solving ODEs"
    assert titles[2] == "Ordinary Differential Equations 1 — Lecture 6: Fourier Series"
    assert report.changed == 2 and report.api_calls == 2


def test_keep_existing_title_is_left_verbatim(conn):
    _seed(conn, 1, "/h/vault/incoming/papers/attention.pdf", "Attention Is All You Need", "Transformers.")
    fake = _FakeClient({"attention.pdf": {"keep_existing": True, "topic": "Attention Is All You Need"}})
    retitle.build_titles(conn, use_llm=True, client=fake, log=lambda *_: None)
    assert conn.execute("SELECT title FROM documents WHERE id=1").fetchone()["title"] == "Attention Is All You Need"


def test_cache_avoids_second_api_call(conn):
    _seed(conn, 1, "/h/vault/incoming/papers/p.pdf", "banner", "To study X.")
    fake = _FakeClient({"p.pdf": {"keep_existing": False, "topic": "Study of X"}})
    r1 = retitle.build_titles(conn, use_llm=True, client=fake, log=lambda *_: None)
    r2 = retitle.build_titles(conn, use_llm=True, client=fake, log=lambda *_: None)
    assert r1.api_calls == 1 and r2.api_calls == 0 and r2.cache_hits == 1


def test_no_llm_uses_deterministic_topic(conn):
    base = "/h/vault/incoming/coursework/oxford-year1/Calculus/calc"
    _seed(conn, 1, f"{base}/Slides lecture 1.pdf", "banner", "To introduce vector calculus and its operators.")
    retitle.build_titles(conn, use_llm=False, log=lambda *_: None)
    title = conn.execute("SELECT title FROM documents WHERE id=1").fetchone()["title"]
    assert title == "calc — Lecture 1: Vector calculus and its operators"


def test_no_llm_keeps_title_without_structural_signal(conn):
    # A paper in a bucket (no module, no sequence) keeps its stored title under --no-llm.
    _seed(conn, 1, "/h/vault/incoming/papers/attention.pdf", "Attention Is All You Need", "To study X.")
    retitle.build_titles(conn, use_llm=False, log=lambda *_: None)
    assert conn.execute("SELECT title FROM documents WHERE id=1").fetchone()["title"] == "Attention Is All You Need"


def test_rollback_restores_prior_titles(conn):
    _seed(conn, 1, "/h/vault/incoming/papers/p.pdf", "Original Title", "To study X.")
    fake = _FakeClient({"p.pdf": {"keep_existing": False, "topic": "Study of X"}})
    retitle.build_titles(conn, use_llm=True, client=fake, log=lambda *_: None)
    assert conn.execute("SELECT title FROM documents WHERE id=1").fetchone()["title"] == "Study of X"
    retitle.rollback(conn, log=lambda *_: None)
    assert conn.execute("SELECT title FROM documents WHERE id=1").fetchone()["title"] == "Original Title"
