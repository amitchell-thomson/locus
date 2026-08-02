"""Relevance for material he chose himself (`reading/relevance.py`).

Model-free and network-free: the embedder is injected, so what is asserted is which TEXT gets
scored and how the link is stored — the parts that decide whether the link is interesting or
meaningless.
"""

from __future__ import annotations

import struct
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.reading import relevance as R


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "rel.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _profile(conn, label, text, *, kind="project"):
    with conn:
        cur = conn.execute(
            "INSERT INTO discovery_profiles (subject_kind, subject_key, facet, label, text, "
            "built_at) VALUES (?,?,'synthesis',?,?,'2026-07-31')",
            (kind, "1", label, text),
        )
        conn.execute(
            "INSERT INTO discovery_profile_vectors (profile_id, embedding) VALUES (?,?)",
            (cur.lastrowid, struct.pack("768f", *_vec(1.0))),
        )
    return cur.lastrowid


def _vec(lead: float):
    return [lead] + [0.0] * 767


def _target(conn, *, path="/Reading/In-Progress/A Book", uri="raw/book.pdf",
            proposal_id=None, folder="In-Progress", marks=0):
    with conn:
        cur = conn.execute(
            "INSERT INTO reading_targets (doc_uuid, device_path, source_uri, proposal_id, "
            "linked_by, created_at, device_folder, marks) VALUES (?,?,?,?,'manual',"
            "'2026-08-01',?,?)",
            (f"u-{path}", path, uri, proposal_id, folder, marks),
        )
    return cur.lastrowid


def _mark(conn, uri, text, note=""):
    with conn:
        conn.execute(
            "INSERT INTO pdf_annotations (source_uri, pdf_page, kind, bbox_key, covered_text, "
            "note, in_margin, captured_at) VALUES (?,1,'underline',?,?,?,0,'2026-07-30')",
            (uri, f"k{len(text)}{note}", text, note),
        )


def _embedder(calls: list):
    def embed(text: str):
        calls.append(text)
        return _vec(1.0)

    return embed


# --- what gets scored --------------------------------------------------------------------------


def test_his_marked_passages_are_what_a_book_is_scored_on(conn):
    """The point of the feature: an abstract says what the AUTHOR thinks a book is about; the
    marks say which parts made HIM stop, and only one of those is about his work."""
    tid = _target(conn, uri="raw/apm.pdf", marks=2)
    _mark(conn, "raw/apm.pdf", "every hedge fund holds a copy of the same portfolio",
          note="is this crowding?")
    _profile(conn, "Alpha Fund", "cascade detection over crowded positions")

    calls: list[str] = []
    out = R.link_target(conn, tid, embed_fn=_embedder(calls))

    assert out.ok and out.subject_label == "Alpha Fund"
    assert "26" not in out.detail and "passage" in out.detail
    scored = calls[0]
    assert "every hedge fund holds a copy" in scored
    assert "is this crowding?" in scored, "what he wrote beside a passage counts too"


def test_a_paper_with_no_marks_falls_back_to_its_abstract(conn):
    with conn:
        conn.execute(
            "INSERT INTO reading_proposals (kind, dedupe_key, title, why, why_kind, evidence_key, "
            "status, created_at, abstract) VALUES ('paper','k','P','w','discovery','e',"
            "'accepted','2026-08-01','an abstract about regimes')"
        )
    tid = _target(conn, proposal_id=1)
    _profile(conn, "regime-ml", "hidden markov regimes")

    calls: list[str] = []
    out = R.link_target(conn, tid, embed_fn=_embedder(calls))
    assert out.ok and "abstract" in out.detail
    assert "an abstract about regimes" in calls[0]


def test_a_book_with_nothing_but_a_title_says_so(conn):
    """Thin evidence must LOOK thin rather than be padded into confidence."""
    tid = _target(conn, path="/Reading/In-Progress/Some Book")
    _profile(conn, "Alpha Fund", "cascade detection")
    out = R.link_target(conn, tid, embed_fn=_embedder([]))
    assert out.ok and out.detail == "scored on its title alone"


# --- storing the link --------------------------------------------------------------------------


def test_the_link_and_its_fit_are_stored_on_the_target(conn):
    tid = _target(conn)
    _profile(conn, "Alpha Fund", "cascade detection")
    R.link_target(conn, tid, embed_fn=_embedder([]))

    row = conn.execute("SELECT * FROM reading_targets WHERE id=?", (tid,)).fetchone()
    assert row["subject_kind"] == "project"
    assert row["subject_label"] == "Alpha Fund"
    assert row["fit"] is not None, "the cosine is the checkable fact under the prose"
    assert row["title"] == "A Book"


def test_an_already_linked_target_is_not_rescored(conn):
    tid = _target(conn)
    _profile(conn, "Alpha Fund", "cascade detection")
    R.link_target(conn, tid, embed_fn=_embedder([]))
    assert R.targets_needing_a_link(conn) == []


def test_owner_only_excludes_delivered_papers(conn):
    with conn:
        conn.execute(
            "INSERT INTO reading_proposals (kind, dedupe_key, title, why, why_kind, "
            "evidence_key, status, created_at) VALUES ('paper','k','P','w','discovery','e',"
            "'accepted','2026-08-01')"
        )
    mine = _target(conn, path="/Reading/In-Progress/My Book")
    _target(conn, path="/Reading/In-Progress/Delivered", proposal_id=1)
    assert R.targets_needing_a_link(conn, only_owner=True) == [mine]


def test_no_profiles_yet_is_reported_not_guessed(conn):
    """Before `locus discover --profiles` has run there is nothing to compare against, and
    inventing a link would be worse than admitting that."""
    tid = _target(conn)
    out = R.link_target(conn, tid, embed_fn=_embedder([]))
    assert not out.ok and "no profile" in out.detail


def test_an_embedding_failure_degrades_rather_than_raising(conn):
    tid = _target(conn)
    _profile(conn, "Alpha Fund", "cascade detection")

    def boom(text: str):
        raise RuntimeError("ollama is down")

    out = R.link_target(conn, tid, embed_fn=boom)
    assert not out.ok


# --- what "in progress" means -------------------------------------------------------------------


def test_in_progress_reads_reading_targets_not_proposals(conn):
    """The first cut read `reading_proposals`, so the one document he was genuinely reading and
    annotating — a book he added himself — was the single thing the section left out."""
    _target(conn, path="/Reading/In-Progress/His Own Book")
    _target(conn, path="/Reading/Finished/Done With This", folder="Finished")
    titles = [
        R.title_for(t["device_path"], t["source_uri"]) for t in R.in_progress(conn)
    ]
    assert titles == ["His Own Book"]


def test_title_falls_back_when_the_device_path_went_stale(conn):
    """The 2026-08-02 reorganisation moved every document and nothing rewrote the stored paths."""
    assert R.title_for("", "/vault/incoming/paper/Advanced Portfolio Management.pdf") == \
        "Advanced Portfolio Management"
    assert R.title_for("/Reading/In-Progress/A Book", None) == "A Book"
    assert R.title_for("", None) == "(untitled)"
