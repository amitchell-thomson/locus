"""Fitting each section to its page by measurement (`locus/agent/layout.py`).

These tests RENDER. That is the point: every layout number this module replaces was a static
estimate of height, and an estimate is exactly what cannot be checked by asserting on markdown —
the markdown is identical whether the section fits on one page or spills onto two. The failure
being pinned here is typographic, so the assertions are made against a real PDF.
"""

from __future__ import annotations

from datetime import date, timedelta
from pathlib import Path

import pytest

from locus.agent import compose_daily as cd
from locus.agent import layout as L
from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate

pytest.importorskip("pypandoc", reason="the [reading] extra renders the PDF")
pytest.importorskip("typst")
pytest.importorskip("pymupdf")

TODAY = date(2026, 8, 6)


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "layout.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


@pytest.fixture()
def geometry():
    from locus.config import load
    from locus.reading.md2pdf import PageGeometry

    cfg = load()
    return PageGeometry(
        width_in=cfg.reading.page_width_in, height_in=cfg.reading.page_height_in,
        margin_in=cfg.reading.margin_in, font_pt=cfg.reading.font_pt,
        rule_gap_em=cfg.daily.rule_gap_em, accent=cfg.daily.accent,
        sans_font=cfg.daily.sans_font, running_header="2026-08-06",
    )


def _jotted(conn, text: str, *, at: str) -> int:
    """An owner-authored thread — the same seeding `test_compose_daily` uses, on purpose.

    A second definition of "a developable idea" would drift from the one the Develop page is
    actually built against, and then these tests would be fitting a shape the page never has.
    """
    oid, _ = state.upsert_object(conn, type_="idea", title=text[:60], now=lambda: at)
    state.apply_owner_edit(conn, oid, {"idea": text}, source="test")
    state.set_status(conn, oid, "active")
    return oid


def _short_ideas(conn, n: int) -> None:
    for i in range(n):
        _jotted(conn, f"thought {i}: a short one", at=f"2026-0{(i % 8) + 1}-01T00:00:00+00:00")


def _long_ideas(conn, n: int) -> None:
    body = (
        "a thought worth developing that runs to a realistic length, because a short fixture is "
        "the one shape that cannot overflow, and this one carries the kind of sentence he "
        "actually writes when he is thinking on paper about a paper he has just read"
    )
    # THE NUMBER LEADS. `_jotted` titles an object from the first 60 characters and
    # `upsert_object` deduplicates on the title, so a distinguishing suffix on identical long
    # prose silently collapses every card into one — which looks exactly like a section that
    # refused to grow.
    for i in range(n):
        _jotted(conn, f"idea {i}: {body}", at=f"2026-0{(i % 8) + 1}-01T00:00:00+00:00")


def _pages(markdown: str, geometry) -> int:
    return L.PageFitter(geometry).pages_for(markdown)


# --- what he asked for --------------------------------------------------------------------------


def test_short_cards_fill_the_page_with_more_cards(conn, geometry):
    """The complaint: "a third develop card could definitely fit" while the queue had one waiting.

    `_FIT["ideas"]` is 3 and `_pack` caps at 4 on a line-equivalents estimate, so a day of short
    cards printed three and left the sheet 40% white.
    """
    _short_ideas(conn, 6)
    page = cd.compose(conn, today=TODAY)
    before = len([t for t in page.threads if t.section in cd._IDEAS_SECTIONS])

    fits = {f.key: f for f in L.fit_page(conn, page, geometry=geometry, today=TODAY)}
    after = len([t for t in page.threads if t.section in cd._IDEAS_SECTIONS])
    assert after > before, "short cards left room for another and the fit must take it"
    assert fits["ideas"].cards == after


def test_leftover_space_becomes_writing_lines_when_no_card_fits(conn, geometry):
    """His actual feature request: if a further card will not fit but white space remains, the
    cards already there get MORE ruled lines rather than the page printing blank paper."""
    _long_ideas(conn, 2)          # only two exist, so no third card can be found
    page = cd.compose(conn, today=TODAY)
    fits = {f.key: f for f in L.fit_page(conn, page, geometry=geometry, today=TODAY)}
    assert fits["ideas"].cards == 2
    assert fits["ideas"].lines > L._FLOOR_LINES["ideas"], "spare height should become lines"


def test_a_card_never_gets_fewer_lines_than_today(conn, geometry):
    """The floor is his rule: filling the page by shrinking the writing space is a different page.

    Checked on the shape most likely to tempt it — many long cards, where trading lines for cards
    would fit more of them.
    """
    _long_ideas(conn, 8)
    page = cd.compose(conn, today=TODAY)
    for fit in L.fit_page(conn, page, geometry=geometry, today=TODAY):
        assert fit.lines >= L._FLOOR_LINES[fit.key], f"{fit.key} went under its floor"


# --- the constraint the whole layout rests on ---------------------------------------------------


def test_no_section_overflows_however_long_its_cards_are(conn, geometry):
    """One section per physical page, which is what the estimates existed to protect.

    The fit is only allowed to grow a section while it still measures one page, so the worst
    realistic content must still render one page per section.
    """
    _long_ideas(conn, 8)
    page = cd.compose(conn, today=TODAY)
    L.fit_page(conn, page, geometry=geometry, today=TODAY)
    body = cd.render(page)
    sections = body.count("#pagebreak()") + 1
    assert _pages(body, geometry) == sections, "a fitted section spilled onto a second page"


def test_growing_stops_before_it_spills(conn, geometry):
    """One more line than the fit chose must NOT fit — otherwise it stopped early and left white
    space, which is the thing being fixed."""
    _long_ideas(conn, 3)
    page = cd.compose(conn, today=TODAY)
    fits = {f.key: f for f in L.fit_page(conn, page, geometry=geometry, today=TODAY)}
    chosen = fits["ideas"].lines
    if chosen >= L._MAX_LINES:
        pytest.skip("hit the search bound, not the page bound")
    page.lines["ideas"] = chosen + 1
    assert _pages(cd.render_section(page, "ideas"), geometry) > 1


# --- the wiring that makes it safe --------------------------------------------------------------


def test_anchors_are_renumbered_for_what_is_actually_printed(conn, geometry):
    """The fit changes what is on the page, and `daily_anchors`/`daily_shown` must describe THAT.

    A label printed against one card and stored against another routes his ink to the wrong
    record, and `items_shown` would retire cards he never saw.
    """
    _short_ideas(conn, 6)
    page = cd.compose(conn, today=TODAY)
    L.fit_page(conn, page, geometry=geometry, today=TODAY)

    printed = [t.anchor for t in page.threads if t.section in cd._IDEAS_SECTIONS]
    assert printed == [f"D{i}" for i in range(1, len(printed) + 1)], "gaps or stale numbering"
    anchors = {a.anchor for a in page.anchors}
    assert anchors == set(printed) | {
        a.anchor for a in page.anchors if not a.anchor.startswith("D")
    }
    assert len(page.anchors) == len(anchors), "an anchor was recorded twice"
    body = cd.render(page)
    for anchor in printed:
        assert f"#anc[{anchor}]" in body
    # Nothing is retired that is not printed.
    keys = {k for k, _ in page.items_shown()}
    assert keys == {t.item_key for t in page.threads} | {
        r.item_key for r in page.recalls
    } | {a.item_key for a in page.answered}


def test_the_fit_reuses_the_pool_and_never_re_derives_connections(conn, geometry):
    """`connection_candidates` costs ~39s (measured live), and it is the whole cost of composing.

    Paying it twice — once to compose, once to fit — would make filling the page the most
    expensive step of the day by two orders of magnitude, which is why `compose` leaves its
    candidates on `page.pool`.
    """
    _short_ideas(conn, 5)
    page = cd.compose(conn, today=TODAY)
    assert page.pool, "compose must leave its candidates for the fit"

    calls = []
    real = cd.build_connections

    def counted(*a, **kw):
        calls.append(1)
        return real(*a, **kw)

    cd.build_connections = counted
    try:
        L.fit_page(conn, page, geometry=geometry, today=TODAY)
    finally:
        cd.build_connections = real
    assert calls == [], "the fit re-derived the connection pool"


def test_a_broken_renderer_leaves_the_composed_page_alone(conn, geometry):
    """Degrade, never block (§7). A page that renders slightly short beats no page."""
    _short_ideas(conn, 5)
    page = cd.compose(conn, today=TODAY)
    before = list(page.threads)

    class _Broken(L.PageFitter):
        def pages_for(self, markdown: str) -> int:
            raise RuntimeError("no typst here")

    fits = L.fit_page(
        conn, page, geometry=geometry, today=TODAY, fitter=_Broken(geometry)
    )
    assert all(not f.measured for f in fits)
    assert page.threads == before, "a failed fit must not change the page"
    assert page.lines == {}, "and must leave the static estimate in charge"
