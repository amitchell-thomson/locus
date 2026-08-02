"""The `locus decide` Textual app, driven headlessly.

Textual's `run_test()` pilot presses real keys against a real app with no terminal, so these are
the actual bindings rather than a re-implementation of them. The app is a thin shell over
`decide/queue.py`; what is worth asserting here is that a key press reaches the database, and that
`u` genuinely reverses one — a single-key interface where `n` sits beside `y` is a trap without it.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate

pytest.importorskip("textual", reason="`locus decide` needs the [tui] extra")


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "app.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _proposed(conn, title, *, created_at="2026-01-01T00:00:00+00:00"):
    oid, _ = state.upsert_object(
        conn, type_="concept", title=title, body={"why": "because"}, now=lambda: created_at
    )
    state.add_links(
        conn, oid, [state.ObjectLink("entity", state.entity_key(title, "concept"), "about")]
    )
    return oid


def _status(conn, oid):
    return state.get_object(conn, oid).status


@pytest.mark.asyncio
async def test_y_blesses_and_moves_on(conn):
    from locus.decide.app import build_app

    a = _proposed(conn, "alpha", created_at="2026-01-01T00:00:00+00:00")
    b = _proposed(conn, "beta", created_at="2026-01-02T00:00:00+00:00")
    app = build_app(conn)

    async with app.run_test() as pilot:
        await pilot.press("y")
        assert _status(conn, a) == "active"
        assert _status(conn, b) == "proposed", "only the selected one is decided"
        await pilot.press("n")
        assert _status(conn, b) == "archived"
        assert app.items == [], "the queue drains as he clears it"


@pytest.mark.asyncio
async def test_u_undoes_the_last_decision(conn):
    """`n` is next to `y`; archiving something he meant to bless must be recoverable."""
    from locus.decide.app import build_app

    oid = _proposed(conn, "alpha")
    app = build_app(conn)

    async with app.run_test() as pilot:
        await pilot.press("n")
        assert _status(conn, oid) == "archived"
        await pilot.press("u")
        assert _status(conn, oid) == "proposed", "an undo must reach the database, not just the UI"
        assert len(app.items) == 1


@pytest.mark.asyncio
async def test_j_and_k_move_the_cursor_without_deciding_anything(conn):
    from locus.decide.app import build_app

    a = _proposed(conn, "alpha", created_at="2026-01-01T00:00:00+00:00")
    _proposed(conn, "beta", created_at="2026-01-02T00:00:00+00:00")
    app = build_app(conn)

    async with app.run_test() as pilot:
        await pilot.press("j")
        assert app.cursor == 1
        await pilot.press("k")
        assert app.cursor == 0
        assert _status(conn, a) == "proposed", "navigating decides nothing"


@pytest.mark.asyncio
async def test_an_empty_queue_says_so_calmly(conn):
    from locus.decide.app import build_app

    app = build_app(conn)
    async with app.run_test():
        assert app.items == []
        rendered = str(app.query_one(".empty").render())
        assert "Nothing is waiting on you" in rendered
        assert "does not mean the system did nothing" in rendered


@pytest.mark.asyncio
async def test_e_opens_a_docked_box_that_cannot_hide_below_the_fold(conn):
    """The bug he hit: the box used to be mounted into the scrolling body, so with 48 cards it
    rendered off-screen — `e` looked like it did nothing, and every later key was a no-op because
    `_editing` stayed True with no visible way out."""
    from locus.decide.app import build_app

    oid = _proposed(conn, "alpha")
    app = build_app(conn)

    async with app.run_test() as pilot:
        box = app.query_one("#edit")
        assert not box.has_class("open"), "hidden until asked for"
        await pilot.press("e")
        assert app._editing and box.has_class("open")
        assert box.styles.dock == "bottom", "docked, so it is on screen whatever the scroll"

        await pilot.press("n", "e", "w")
        assert box.value == "new", "keys reach the box, not the reject binding"
        assert _status(conn, oid) == "proposed", "typing must not decide anything"

        await pilot.press("enter")
        assert _status(conn, oid) == "active"
        assert state.get_object(conn, oid).body["why"] == "new"
        assert not app._editing and not box.has_class("open")


@pytest.mark.asyncio
async def test_escape_cancels_an_edit_rather_than_quitting(conn):
    """Quitting out of a half-typed correction is the one thing esc must not do."""
    from locus.decide.app import build_app

    oid = _proposed(conn, "alpha")
    app = build_app(conn)

    async with app.run_test() as pilot:
        await pilot.press("e")
        await pilot.press("x", "y")
        await pilot.press("escape")
        assert not app._editing, "the edit is cancelled"
        assert app.is_running, "...and the app is still up"
        assert _status(conn, oid) == "proposed", "nothing was decided"
        await pilot.press("y")
        assert _status(conn, oid) == "active", "keys work again afterwards"


@pytest.mark.asyncio
async def test_left_and_right_move_between_kinds(conn):
    from locus.decide.app import build_app
    from locus.decide import queue as Q

    _proposed(conn, "alpha")
    app = build_app(conn)

    async with app.run_test() as pilot:
        assert app.kind == Q.KIND_OBJECT
        await pilot.press("right")
        assert app.kind == Q.KIND_DUPLICATE
        await pilot.press("right")
        assert app.kind == Q.KIND_ABANDONED
        await pilot.press("right")
        assert app.kind == Q.KIND_OBJECT, "it wraps"
        await pilot.press("left")
        assert app.kind == Q.KIND_ABANDONED


@pytest.mark.asyncio
async def test_a_kind_with_nothing_pending_says_so_without_hiding_the_tabs(conn):
    from locus.decide.app import build_app

    _proposed(conn, "alpha")
    app = build_app(conn)
    async with app.run_test() as pilot:
        await pilot.press("right")
        assert "Nothing pending under Duplicates" in str(app.query_one(".empty").render())
        assert app.query_one("#tabs"), "the tab bar stays, so he can move on"


@pytest.mark.asyncio
async def test_the_background_is_the_terminals_own(conn):
    """`ansi_default` renders as no colour at all, so terminal transparency shows through."""
    from locus.decide.app import build_app

    app = build_app(conn)
    async with app.run_test():
        theme = app.get_theme(app.theme)
        assert theme.ansi is True
        for key in ("background", "surface", "panel"):
            assert theme.variables[key] == "ansi_default", f"{key} would paint over the terminal"
