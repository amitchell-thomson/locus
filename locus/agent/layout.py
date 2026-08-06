"""Fill each page of the daily by RENDERING it and counting pages, not by estimating.

WHY THIS EXISTS. Every layout number in `compose_daily` is a static estimate of height —
`_FIT` (cards per section), `_LINE_BUDGET`/`_lines_for` (ruled lines per card), `_pack`
(line-equivalents at 90 chars a line), `_CONNECT_CHAR_BUDGET` (1000 characters of prose). Each was
measured once, against the content of the day it was measured on, and then frozen. Content is not
frozen: a day of short cards leaves the Develop page 40% white while a fourth card is sitting in
the queue, and a day of long ones would have spilled if the estimate had been generous enough to
admit it. The estimate cannot be right for both, and the failure is silent in both directions.

Rendering costs ~50ms (measured 2026-08-06: 0.05-0.09s for a full five-page page), so the loop
below can ask the real question — "does this section still fit on one page?" — a few dozen times
per compose and still be the cheapest step in the pipeline. That makes the layout numbers
observations rather than predictions, which is what §3 asks for everywhere else.

WHAT IT OPTIMISES, in his words: "as many cards as possible" first, without ever giving a card
fewer ruled lines than it gets today; then, if another card will not fit but white space remains,
spend that space on MORE LINES for the cards already there. So: grow cards at the floor, then grow
lines at the winning card count. Never below `_FLOOR_LINES`, which is what each section prints
today — a page that fits more cards by shrinking the writing space is not the trade he asked for.

DEGRADES TO THE ESTIMATE. Rendering needs the `reading` extra (pandoc + typst). Anything that
fails here leaves `page.lines` empty and the page as `compose` built it, which is exactly the
pre-fit behaviour — a page that renders slightly short is worth more than no page.
"""

from __future__ import annotations

import logging
import sqlite3
import tempfile
from dataclasses import dataclass
from datetime import date
from pathlib import Path

from locus.agent import compose_daily as cd

log = logging.getLogger(__name__)

# Ruled lines each section prints today, and the floor this pass may never go under (his rule).
# `answered` carries rules under each answer even though nothing is asked of him — a correction
# has to have somewhere to go.
_FLOOR_LINES = {"ideas": 3, "connect": 3, "recall": 2, "answered": 3, "open": 4}

# The back page has one region and no cards, so only its line count is fitted. It is also the one
# page whose spare space cannot be known in advance: it carries the status line and the recall
# answer key, and the answer key's height is decided by how many cards the Recall fit kept. Hence
# last in `_SECTIONS`.
_CARDLESS = frozenset({"open"})
_SECTIONS = ("ideas", "connect", "recall", "answered", "open")

# Upper bounds. Not height limits — the render decides height — but a stop for the search, and a
# statement that a page of fifteen one-line cards is not the goal either. Cards are also bounded
# by what the builders actually return, which on most days is the binding constraint.
_MAX_CARDS = {"ideas": 6, "connect": 5, "recall": 6, "answered": 5}
# A stop for the line search, deliberately ABOVE anything a page can hold (a 679pt text block at
# 19.8pt between rules tops out around 30 rules before any text). It has to be: at 8 a two-card
# Develop page stopped growing with a third of the sheet still white, and the bound rather than
# the page was deciding the layout — the exact failure this module replaces. The cost of a
# generous bound is one 40ms render per line it does not take.
_MAX_LINES = 20


@dataclass(frozen=True)
class SectionFit:
    """What the measurement established for one section."""

    key: str
    cards: int
    lines: int
    measured: bool = True

    def render(self) -> str:
        how = "measured" if self.measured else "estimated"
        return f"{self.key}: {self.cards} card(s) x {self.lines} line(s) ({how})"


class PageFitter:
    """Renders a section's markdown and answers one question: does it fit on a single page?"""

    def __init__(self, geometry, *, page_date: str = "") -> None:
        self._geometry = geometry
        self._page_date = page_date

    def pages_for(self, markdown: str) -> int:
        from locus.reading.md2pdf import render_markdown_to_pdf

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "fit.pdf"
            render_markdown_to_pdf(markdown, out, geometry=self._geometry)
            import fitz

            with fitz.open(out) as doc:
                return doc.page_count

    def fits(self, markdown: str) -> bool:
        return self.pages_for(markdown) <= 1


def _candidates(
    conn: sqlite3.Connection, page: cd.DailyPage, *, today: date, seen: set[str]
) -> dict[str, list]:
    """Every item each section could print, generously — the fit decides how many actually do.

    Deliberately more than any section will take, and free of the proxies that stand in for height
    elsewhere: `_pack`'s line-equivalent estimate for ideas and `_CONNECT_CHAR_BUDGET` for
    connections. Both exist to predict what a render can simply be asked, and applying them here
    as well would cap the page below what measurably fits — the complaint that prompted this
    module.

    REUSES `page.pool`, which `compose` filled on the way past. Rebuilding it costs 39 seconds,
    almost all of it `connection_candidates` re-deriving every cross-corpus pair, and paying that
    twice per compose would make the fit the most expensive step in the day by two orders of
    magnitude. The rebuild below is the fallback for a page that did not come from `compose`.
    """
    if page.pool:
        return {k: list(v) for k, v in page.pool.items()}
    return {
        "ideas": cd.build_develop(conn, limit=cd._POOL_CARDS, seen=seen),
        "connect": cd.build_connections(
            conn, limit=cd._POOL_CARDS, seen=seen, char_budget=None
        ),
        "recall": cd.build_recalls(
            conn, today=today, limit=cd._POOL_CARDS, seen=seen, record=False
        ),
        "answered": cd.build_answered(conn, limit=cd._POOL_CARDS, seen=seen),
    }


def _apply(page: cd.DailyPage, pool: dict[str, list], key: str, cards: int, lines: int) -> None:
    """Put `cards` items of one section onto `page` at `lines` ruled lines each."""
    page.lines[key] = lines
    if key in _CARDLESS:
        return                                   # one fixed region; only its line count moves
    if key == "ideas":
        page.threads = pool["ideas"][:cards] + [
            t for t in page.threads if t.section not in cd._IDEAS_SECTIONS
        ]
    elif key == "connect":
        page.threads = [
            t for t in page.threads if t.section in cd._IDEAS_SECTIONS
        ] + pool["connect"][:cards]
    elif key == "recall":
        page.recalls = pool["recall"][:cards]
    else:
        page.answered = pool["answered"][:cards]


def _fit_section(
    page: cd.DailyPage, pool: dict[str, list], key: str, fitter: PageFitter
) -> SectionFit:
    """Most cards that fit at the floor, then most lines that fit at that card count.

    CARDS FIRST, LINES SECOND, because they are not interchangeable: a card is something of his to
    react to, and a line is room to react in. Trading a card for lines would be a different page.

    The search is linear and stops at the first miss — height is monotone in both knobs, so
    anything past a miss also misses. At most `_MAX_CARDS + _MAX_LINES` renders per section, and a
    render is ~50ms.
    """
    floor = _FLOOR_LINES[key]
    if key in _CARDLESS:
        cards = 1
    else:
        available = len(pool.get(key) or [])
        if not available:
            return SectionFit(key, 0, floor, measured=False)
        cards = 1
        for n in range(2, min(available, _MAX_CARDS[key]) + 1):
            _apply(page, pool, key, n, floor)
            if not fitter.fits(cd.render_section(page, key)):
                break
            cards = n

    lines = floor
    for extra in range(floor + 1, _MAX_LINES + 1):
        _apply(page, pool, key, cards, extra)
        if not fitter.fits(cd.render_section(page, key)):
            break
        lines = extra

    _apply(page, pool, key, cards, lines)
    # ONE CARD MAY NOT FIT AT ALL — a single connection note can run 500 characters. Say so rather
    # than silently printing a page that spills, which breaks one-section-per-page.
    if not fitter.fits(cd.render_section(page, key)):
        log.warning("layout: %s overflows one page even at %d card(s)", key, cards)
    return SectionFit(key, cards, lines)


def fit_page(
    conn: sqlite3.Connection,
    page: cd.DailyPage,
    *,
    geometry,
    today: date | None = None,
    fitter: PageFitter | None = None,
) -> list[SectionFit]:
    """Grow every section of `page` to fill its sheet. Mutates `page`; returns what it settled on.

    Runs AFTER `compose` and BEFORE `render`/`persist`, and re-numbers the anchors itself: the
    sections it grows change what is printed, and `daily_anchors` and `daily_shown` must describe
    the page he is actually handed (`assign_anchors`).

    Composition stays aggregate-only — a render is deterministic and free, and no model is called.
    """
    today = today or date.today()
    fitter = fitter or PageFitter(geometry, page_date=page.page_date)
    seen = cd._shown_keys(conn)
    pool = _candidates(conn, page, today=today, seen=seen)

    out: list[SectionFit] = []
    for key in _SECTIONS:
        # SNAPSHOT FIRST. `_fit_section` applies a trial selection to the page before it measures
        # it, so a renderer that throws mid-search leaves the page holding a trial rather than
        # what `compose` decided — more cards than were ever proved to fit, which is precisely the
        # overflow this module exists to prevent. Degrading has to mean "as composed", not
        # "wherever the search happened to stop".
        snapshot = (list(page.threads), list(page.recalls), list(page.answered))
        try:
            out.append(_fit_section(page, pool, key, fitter))
        except Exception as exc:              # a page that renders short beats no page at all
            log.warning("layout: could not fit %s, keeping the estimate: %s", key, exc)
            page.lines.pop(key, None)
            page.threads, page.recalls, page.answered = snapshot
            out.append(SectionFit(key, len(pool.get(key) or []), _FLOOR_LINES[key], measured=False))
    cd.assign_anchors(page)
    return out
