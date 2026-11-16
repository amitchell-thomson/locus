"""P8: syllabi as structure: topics grounded in the page, linked to lectures or marked uncovered."""

from __future__ import annotations

from datetime import datetime, timezone

import pytest

from locus.heuris import db as hdb
from locus.heuris import syllabus as syl
from locus.heuris.index import Item, index
from tests.test_heuris_index import fake_embed

BODY = """b1401: Signal Analysis; 4 Lectures, 1 Tutorial Sheet
Understanding of the Fourier transform and the discrete Fourier transform, principles and design of
analogue and digital filters, stochastic models and spectral estimation.
Learning Outcomes:
1. Basics of signals and simple filter theory, frequency response, transfer functions, design of
simple analogue filters, the Butterworth filter.
"""


def write(root, body=BODY, module="signal-analysis", course="b14"):
    p = root / f"{course}-information-engineering-systems" / module / "syllabus.md"
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(f"---\ncourse: {course}\ncodes: [b1401]\ncanvas_module: Signal Analysis\n"
                 f"edition: 3rd Year Syllabus 2026-27\nverbatim: true\n---\n\n"
                 f"The department's syllabus for this submodule, word for word. It is the authority.\n\n{body}")
    return p


def topics(*pairs):
    return lambda body: syl._Topics(topics=[syl._Topic(topic=t, quote=q) for t, q in pairs])


GOOD = topics(("Fourier transform", "Understanding of the Fourier transform"),
              ("Butterworth filter", "design of\nsimple analogue filters, the Butterworth filter"),
              ("Wavelets", "wavelet transforms and multiresolution"))   # not in the page


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    yield c
    c.close()


def test_a_syllabus_is_an_item_and_our_note_is_not_part_of_it(tmp_path):
    write(tmp_path)
    [it] = list(syl.items(tmp_path))
    assert it.ref == "syllabus:b14/signal-analysis" and it.kind == "syllabus" and it.thing == "b14"
    assert it.text.startswith("b1401: Signal Analysis") and "authority" not in it.text
    assert it.at.year == 2026 and "Signal Analysis" in it.title


def test_topics_must_quote_the_page_and_are_read_again_only_when_it_changes(conn, tmp_path):
    write(tmp_path)
    calls = []

    def extract(body):
        calls.append(body)
        return GOOD(body)

    rep = syl.extract_changed(conn, syl.read_all(tmp_path), extract=extract)
    rows = [tuple(r) for r in conn.execute("SELECT topic, status FROM syllabus ORDER BY ord")]
    # the invented wavelets topic is dropped; a quote across a line break still counts
    assert rows == [("Fourier transform", "current"), ("Butterworth filter", "current")] and rep.dropped == 1
    syl.extract_changed(conn, syl.read_all(tmp_path), extract=extract)
    assert len(calls) == 1                                   # unchanged: not read again
    write(tmp_path, BODY + "2. The z-transform.\n")
    syl.extract_changed(conn, syl.read_all(tmp_path), extract=topics(("z-transform", "The z-transform")))
    assert [r[0] for r in conn.execute("SELECT topic FROM syllabus WHERE status='current'")] == ["z-transform"]
    assert conn.execute("SELECT COUNT(*) FROM syllabus WHERE status='old'").fetchone()[0] == 2   # history kept


def _lib(ref, doc, title, text, thing="b14"):
    return Item(ref=ref, kind="source", at=datetime(2026, 11, 1, tzinfo=timezone.utc), title=title, text=text,
                thing=thing, area="Studies", parent=f"src:{doc}", meta={"unit": "section", "doc": doc, "doc_title": title})


def test_a_topic_links_to_its_courses_lectures_or_is_uncovered(conn, tmp_path):
    write(tmp_path)
    index(conn, "library", [
        _lib("src:1#s0", 1, "B14 Signal Analysis Lecture 1", "The Fourier transform of a rectangle is a sinc."),
        _lib("src:1#s1", 1, "B14 Signal Analysis Lecture 1", "More on the Fourier transform."),
        _lib("src:2#s0", 2, "Third-year syllabus 2026-27", "the Fourier transform, the Butterworth filter"),
        _lib("src:3#s0", 3, "B15 Lecture", "Fourier transform in control", thing="b15"),
    ], embed=fake_embed)
    syl.extract_changed(conn, syl.read_all(tmp_path), extract=GOOD)

    def score(query, texts):     # a stand-in cross-encoder: high when the topic's words appear
        return [5.0 if "fourier transform" in t.lower() and "fourier" in query.lower() else -3.0 for t in texts]

    rep = syl.link(conn, embed=fake_embed, score=score)
    assert rep.linked == 2 and rep.covered == 1
    out = syl.render(conn, "b14")
    fourier, butter = out.split("Butterworth")[0], out.split("Butterworth")[1]
    # one link per document (its best section), never the syllabus PDF, never another course's
    assert fourier.count("[src:1#") == 1 and "src:2" not in out and "src:3" not in out
    assert "UNCOVERED" in butter and "UNCOVERED" not in fourier
    # nothing changed in the library: not linked again
    assert syl.link(conn, embed=fake_embed, score=score).linked == 0
    index(conn, "library", [_lib("src:4#s0", 4, "B14 Lecture 2", "Butterworth filter design")], embed=fake_embed)
    assert syl.link(conn, embed=fake_embed, score=score).linked == 2


def test_education_is_proposed_once_per_course_never_written(conn, tmp_path):
    write(tmp_path)
    write(tmp_path, module="image-processing")
    rep = syl.propose_education(conn, syl.read_all(tmp_path))
    assert rep.proposed == ["b14"]
    row = tuple(conn.execute("SELECT subject, slot, value, status FROM facts").fetchone())
    assert row[:2] == ("me", "education:b14") and row[3] == "proposed"
    assert "Year 3 (Part B), 2026-27" in row[2] and "Signal Analysis" in row[2]
    assert syl.propose_education(conn, syl.read_all(tmp_path)).proposed == []
    # and he can approve it (the route was once one the ledger refused to write)
    from locus.heuris import facts as F
    fid = conn.execute("SELECT id FROM facts WHERE status='proposed'").fetchone()[0]
    assert F.approve(conn, fid).fact.status == "current"


def test_not_yet_linked_is_not_uncovered_and_one_run_at_a_time(conn, tmp_path):
    write(tmp_path)
    syl.extract_changed(conn, syl.read_all(tmp_path), extract=GOOD)
    assert "not linked yet" in syl.render(conn, "b14") and "UNCOVERED" not in syl.render(conn, "b14")
    lock = tmp_path / ".syllabus.lock"
    held = syl._lock(lock)
    with pytest.raises(syl.Busy):
        syl.run(conn, tmp_path, extract=GOOD, embed=fake_embed, score=lambda q, t: [], lock=lock)
    held.close()
    rep = syl.run(conn, tmp_path, extract=GOOD, embed=fake_embed, score=lambda q, t: [-1.0] * len(t), lock=lock,
                  propose=False)
    assert rep.linked == 2 and "UNCOVERED" in syl.render(conn, "b14")
