"""`locus decide` — clear every pending decision in one pass, one tab per kind.

Why this is a terminal app and not a page. Asked where blessings should live, he chose the
terminal and was specific: "everything pending, but in neat sections by type in a tui (use
textual)... they should ALWAYS be two separate approval things". The daily page is for thinking
with the source at hand; this is for yes/no with no thinking in it, cleared as fast as he can
press a key. Until it existed the only route through 48 proposed objects was `locus objects
--bless <id>`, one id at a time, which is exactly why 48 had accumulated.

    left / right     move between kinds        y / enter   accept
    j / k / up/down  move within a kind        n / x       reject
    e                type a correction         u           undo the last
    q                quit                      esc         cancel an edit (never quits mid-edit)

THREE THINGS THIS LAYOUT IS FIXING, all of them found by him using it:

**Tabs, not one long list.** One scroll holding every kind meant the only way to reach a reading
decision was past forty-eight concepts. Kinds are independent judgements and now get independent
pages; the tab bar doubles as the only place a count appears, where it is progress through work he
is doing rather than a backlog he is reminded of.

**The edit box is DOCKED.** It used to be mounted into the scrolling body, so with 48 cards it
appeared below the fold — he pressed `e`, saw nothing, and every subsequent key did nothing
because `_editing` was still True with no visible way out. That is the "does nothing and freezes"
he reported. It is now docked above the footer, always on screen, and `esc` cancels.

**The background is the terminal's.** `ansi_default` everywhere, so the app inherits whatever
transparency the terminal has rather than painting an opaque rectangle over it — the same theme
trick the digest project uses (`digest/tui.py`).
"""

from __future__ import annotations

import sqlite3

from locus.decide import queue as Q

_INSTALL_HINT = (
    "`locus decide` needs the [tui] extra: uv pip install -e '.[tui]' (installs textual)."
)

# Every surface colour is `ansi_default`, which Textual renders as "no colour at all" — the
# terminal's own background shows through, including transparency. Setting `background` alone is
# not enough: `surface` and `panel` are what widgets actually paint with.
_THEME_NAME = "locus"
_THEME_VARIABLES = {
    "background": "ansi_default",
    "surface": "ansi_default",
    "panel": "ansi_default",
    "boost": "ansi_default",
    "block-cursor-blurred-background": "ansi_default",
    "block-hover-background": "ansi_default",
    "ansi-background": "ansi_default",
    "ansi-foreground": "ansi_default",
}

_CSS = """
Screen { layout: vertical; background: ansi_default; }
#tabs { dock: top; height: 1; padding: 0 2; background: ansi_default; }
#body { padding: 1 2; background: ansi_default; }
#edit { dock: bottom; height: 3; margin: 0 2; display: none; background: ansi_default; }
#edit.open { display: block; }
#footer { dock: bottom; height: 1; padding: 0 2; color: $text-muted; background: ansi_default; }
.card { padding: 0 2; margin-bottom: 1; border-left: outer $primary-background;
        background: ansi_default; }
.card.selected { border-left: outer $accent; }
.empty { padding: 2 4; background: ansi_default; }
.help { padding: 0 2; margin-bottom: 1; color: $text-muted; background: ansi_default; }
"""

_TAB_TITLES = {
    Q.KIND_OBJECT: "Proposed",
    Q.KIND_DUPLICATE: "Duplicates",
    Q.KIND_INTENT: "Marks",
    Q.KIND_ABANDONED: "Reading",
}

# What each tab is FOR, and what a yes or a no there actually DOES.
#
# The four kinds share one pair of keys and mean four different things by them: `y` on Duplicates
# archives an object, `y` on Reading merely defers one, and `n` on Marks is not a rejection at all
# — it records that the passage was an idea of his and builds a thread from it. The card can carry
# the label (`y = merge`) but not the consequence, and the consequence is what he is deciding on.
# Clearing fast (the whole point of the TUI) is only safe when a key press holds no surprise.
#
# Each entry is (what the tab is, an example, what yes means, what no means, a caveat or "").
_TAB_HELP = {
    Q.KIND_OBJECT: (
        "Concepts, projects and questions the structurer pulled out of your documents. It only "
        "proposes — blessing is what makes one real enough to come back to you.",
        '"regime detection" · grounded in 3 documents that name it',
        "bless it — a thread you genuinely think about",
        "drop for good — boilerplate, a stray phrase, or someone else's idea",
        "e replaces the agent's reasoning with your wording, which no later pass overwrites.",
    ),
    Q.KIND_DUPLICATE: (
        "Two objects that are one concept under two names — a difference of casing, or the same "
        "thing written down twice.",
        '"Bootstrap" = "bootstrap" · one idea, entered twice',
        "merge — folds the newer into the oldest, which carries the longer history",
        "keep separate — genuinely different, and this pair is never raised again",
        "A merge archives rather than deletes, so u puts both back.",
    ),
    Q.KIND_INTENT: (
        "Passages you marked while reading, where the intent pass could not tell what you meant. "
        "Under the confidence floor nothing happens to them until you answer.",
        'an underline guessed "important" at 45% · important, not understood, or an idea?',
        "take the guess — recorded as yours, and acted on now",
        "it was an idea of yours — becomes a thread linked to that passage",
        "Move past one to leave it alone: important is the reading where nothing need happen.",
    ),
    Q.KIND_ABANDONED: (
        "Reading on the tablet you have not marked in a fortnight. The only question being asked "
        "is whether it was the wrong paper.",
        "a paper with no marks in 21 days · links to Alpha Fund",
        "still reading — defers it, and asks again later",
        "wasn't useful — tunes what the discovery loop proposes next",
        "",
    ),
}


def _import_textual():
    try:
        import textual  # noqa: F401
    except ImportError as exc:                     # pragma: no cover - only without the extra
        raise RuntimeError(_INSTALL_HINT) from exc


def build_app(conn: sqlite3.Connection):
    """Construct the Textual app. Imported lazily so the CLI works without the extra."""
    _import_textual()

    from textual.app import App, ComposeResult
    from textual.binding import Binding
    from textual.containers import VerticalScroll
    from textual.theme import Theme
    from textual.widgets import Input, Label, Static

    class DecideApp(App):
        CSS = _CSS
        TITLE = "locus decide"
        BINDINGS = [
            Binding("right,l", "next_tab", "next kind", show=False),
            Binding("left,h", "prev_tab", "prev kind", show=False),
            Binding("j,down", "next", "next", show=False),
            Binding("k,up", "prev", "prev", show=False),
            Binding("y,enter", "accept", "accept"),
            Binding("n,x", "reject", "reject"),
            Binding("e", "edit", "correct"),
            Binding("u", "undo", "undo"),
            Binding("q", "quit", "quit"),
            # `escape` is NOT bound to quit: it cancels an edit. Quitting out of a half-typed
            # correction is the one thing esc must not do here.
            Binding("escape", "cancel_edit", "cancel", show=False),
        ]

        def __init__(self) -> None:
            super().__init__()
            self.conn = conn
            queue = Q.pending(conn)
            self.sections: dict[str, list[Q.Decision]] = {
                kind: queue.sections.get(kind, []) for kind in Q.TUI_KINDS
            }
            self.tab = 0
            self.cursor = 0
            self.resolved: list[tuple[Q.Decision, str]] = []
            self._editing = False

        # --- state ------------------------------------------------------------------------------

        @property
        def kind(self) -> str:
            return Q.TUI_KINDS[self.tab]

        @property
        def items(self) -> list[Q.Decision]:
            return self.sections[self.kind]

        @property
        def current(self) -> Q.Decision | None:
            items = self.items
            return items[self.cursor] if 0 <= self.cursor < len(items) else None

        @property
        def total(self) -> int:
            return sum(len(v) for v in self.sections.values())

        # --- layout -----------------------------------------------------------------------------

        def compose(self) -> ComposeResult:
            yield Static("", id="tabs")
            yield VerticalScroll(id="body")
            yield Input(placeholder="your wording — enter to apply and accept, esc to cancel",
                        id="edit")
            yield Label("", id="footer")

        def on_mount(self) -> None:
            self.register_theme(Theme(
                name=_THEME_NAME, primary="#ffffff", dark=True, ansi=True,
                variables=dict(_THEME_VARIABLES),
            ))
            self.theme = _THEME_NAME
            self._render()

        def _render(self) -> None:
            self.query_one("#tabs", Static).update(self._tab_bar())

            body = self.query_one("#body", VerticalScroll)
            # `remove_children()` completes asynchronously, so a re-render that mounts a widget
            # carrying the SAME id crashes with DuplicateIds — hit by switching tabs. The
            # empty-state is therefore identified by CLASS, which has no such constraint.
            body.remove_children()
            # Above the cards on EVERY tab, including the empty ones: a tab with nothing pending
            # is exactly where "what would appear here?" is the open question.
            body.mount(self._help(Static, self.kind))
            if not self.total:
                body.mount(Static(
                    "Nothing is waiting on you.\n\n"
                    "An empty queue is a valid state — it does not mean the system did nothing.",
                    classes="empty",
                ))
            elif not self.items:
                body.mount(Static(
                    f"Nothing pending under {_TAB_TITLES[self.kind]}.\n\n"
                    "Use left/right to move to another kind.", classes="empty",
                ))
            else:
                for i, item in enumerate(self.items):
                    body.mount(self._card(Static, item, selected=(i == self.cursor)))
            self.query_one("#footer", Label).update(
                "←/→ kind · j/k move · y accept · n reject · e correct · u undo · q quit"
            )
            cards = self.query(".card")
            if 0 <= self.cursor < len(cards):
                cards[self.cursor].scroll_visible(animate=False)

        def _tab_bar(self) -> str:
            parts = []
            for i, kind in enumerate(Q.TUI_KINDS):
                n = len(self.sections[kind])
                label = f"{_TAB_TITLES[kind]} {n}" if n else _TAB_TITLES[kind]
                parts.append(f"[reverse b] {label} [/]" if i == self.tab else f"[dim] {label} [/]")
            return "  ".join(parts)

        @staticmethod
        def _help(Static, kind: str):
            """The standing description of one tab — what it holds and what y/n do to it."""
            what, example, yes, no, caveat = _TAB_HELP[kind]
            lines = [what, f"e.g.  {example}", f"[b]y[/b]  {yes}", f"[b]n[/b]  {no}"]
            if caveat:
                lines.append(caveat)
            return Static("\n".join(lines), classes="help")

        @staticmethod
        def _card(Static, item: Q.Decision, *, selected: bool):
            lines = [f"[b]{item.title}[/b]"]
            if item.detail:
                lines.append(item.detail)
            if item.grounding:
                lines.append(f"[dim]{item.grounding}[/dim]")
            lines.append(f"[dim]y = {item.accept_label}   ·   n = {item.reject_label}[/dim]")
            return Static("\n".join(lines), classes="card selected" if selected else "card")

        # --- moving -----------------------------------------------------------------------------

        def action_next_tab(self) -> None:
            if not self._editing:
                self.tab = (self.tab + 1) % len(Q.TUI_KINDS)
                self.cursor = 0
                self._render()

        def action_prev_tab(self) -> None:
            if not self._editing:
                self.tab = (self.tab - 1) % len(Q.TUI_KINDS)
                self.cursor = 0
                self._render()

        def action_next(self) -> None:
            if self.items and not self._editing:
                self.cursor = min(self.cursor + 1, len(self.items) - 1)
                self._render()

        def action_prev(self) -> None:
            if self.items and not self._editing:
                self.cursor = max(self.cursor - 1, 0)
                self._render()

        # --- deciding ---------------------------------------------------------------------------

        def _apply(self, *, accept: bool, note: str = "") -> None:
            item = self.current
            if item is None:
                return
            self.items.pop(self.cursor)
            outcome = Q.resolve(self.conn, item, accept=accept, note=note)
            self.resolved.append((item, outcome))
            self.cursor = min(self.cursor, max(0, len(self.items) - 1))
            self._render()

        def action_accept(self) -> None:
            if not self._editing:
                self._apply(accept=True)

        def action_reject(self) -> None:
            if not self._editing:
                self._apply(accept=False)

        def action_edit(self) -> None:
            """Open the docked correction box.

            His wording replaces the agent's through `state.apply_owner_edit`, which carries a
            durable marker a later agent pass cannot overwrite — the same asymmetry the daily page
            enforces.
            """
            if self._editing or self.current is None:
                return
            self._editing = True
            box = self.query_one("#edit", Input)
            box.value = ""
            box.add_class("open")
            box.focus()

        def action_cancel_edit(self) -> None:
            if self._editing:
                self._close_edit()

        def _close_edit(self) -> None:
            self._editing = False
            box = self.query_one("#edit", Input)
            box.remove_class("open")
            self.set_focus(None)

        def on_input_submitted(self, event) -> None:
            note = event.value
            self._close_edit()
            self._apply(accept=True, note=note)

        def action_undo(self) -> None:
            """Put the last decision back and reverse it in the DATABASE.

            A single-key interface where `n` sits beside `y` is a trap without one. A merge is
            reversed by un-archiving what it folded away, which is possible only because
            `merge_objects` archives rather than deletes.
            """
            if self._editing or not self.resolved:
                return
            from locus.agent import state

            item, _ = self.resolved.pop()
            if item.kind == Q.KIND_OBJECT:
                state.set_status(self.conn, item.ref, "proposed")
            elif item.kind == Q.KIND_DUPLICATE:
                for oid in item.extra:
                    state.set_status(self.conn, oid, "active")
            self.tab = Q.TUI_KINDS.index(item.kind)
            self.sections[item.kind].insert(0, item)
            self.cursor = 0
            self._render()

    return DecideApp()


def run(conn: sqlite3.Connection) -> list[tuple[Q.Decision, str]]:
    """Run the app to completion and return what was decided (for the CLI's summary)."""
    app = build_app(conn)
    app.run()
    return app.resolved
