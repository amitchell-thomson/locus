"""`locus decide` — clear every pending decision in one pass, in neat sections by type.

Why this is a terminal app and not a page. Asked where blessings should live, he chose the
terminal over the tablet and was specific about the shape: "everything pending, but in neat
sections by type in a tui (use textual)... they should ALWAYS be two separate approval things".
The daily page is for thinking with the source at hand; this is for yes/no with no thinking in it,
cleared as fast as he can press a key. Until it existed the only way through 48 proposed objects
was `locus objects --bless <id>`, one id at a time, which is why 48 had accumulated.

The whole app is a thin shell over `decide/queue.py`: the queue decides WHAT is pending and which
surface owns it, and this decides only how it looks and which key does what. That split is what
lets the invariant be tested without a terminal.

Keys are single-press and unshifted, because the point is speed:

    j / k / arrows   move            y / enter   accept (bless, still reading)
    n / x            reject          e           type a correction, then accept
    u                undo the last   q           quit

An empty queue is a valid, calm state and says so — §9's rule applies to this surface too. There
is no count of what is left anywhere except the header, where it is a progress indicator he is
actively working through rather than a backlog he is reminded of.
"""

from __future__ import annotations

import sqlite3

from locus.decide import queue as Q

_INSTALL_HINT = (
    "`locus decide` needs the [tui] extra: uv pip install -e '.[tui]' (installs textual)."
)

_CSS = """
Screen { layout: vertical; }
#header { dock: top; height: 3; padding: 1 2; background: $panel; }
#footer { dock: bottom; height: 1; padding: 0 2; color: $text-muted; }
#body { padding: 1 2; }
.section { margin-bottom: 1; text-style: bold; color: $accent; }
.card { padding: 1 2; border: round $primary-background; margin-bottom: 1; }
.card.selected { border: round $accent; background: $boost; }
.title { text-style: bold; }
.detail { color: $text-muted; }
.grounding { color: $text-disabled; text-style: italic; }
#done { padding: 2 4; }
"""


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
    from textual.widgets import Input, Label, Static

    class DecideApp(App):
        CSS = _CSS
        TITLE = "locus decide"
        BINDINGS = [
            Binding("j,down", "next", "next", show=False),
            Binding("k,up", "prev", "prev", show=False),
            Binding("y,enter", "accept", "accept"),
            Binding("n,x", "reject", "reject"),
            Binding("e", "edit", "correct"),
            Binding("u", "undo", "undo"),
            Binding("q,escape", "quit", "quit"),
        ]

        def __init__(self) -> None:
            super().__init__()
            self.conn = conn
            self.items = Q.pending(conn).flat()
            self.cursor = 0
            self.resolved: list[tuple[Q.Decision, str]] = []
            self._editing = False

        # --- layout ---------------------------------------------------------------------------

        def compose(self) -> ComposeResult:
            yield Static("", id="header")
            yield VerticalScroll(id="body")
            yield Label("", id="footer")

        def on_mount(self) -> None:
            self._render()

        def _render(self) -> None:
            body = self.query_one("#body", VerticalScroll)
            body.remove_children()

            if not self.items:
                header = "Nothing to decide."
                body.mount(Static(
                    "Nothing is waiting on you.\n\n"
                    "An empty queue is a valid state — it does not mean the system did nothing.",
                    id="done",
                ))
            else:
                done = len(self.resolved)
                header = f"{done + 1} of {done + len(self.items)}"
                current = None
                for i, item in enumerate(self.items):
                    if item.kind != current:
                        current = item.kind
                        body.mount(Static(_section_title(current), classes="section"))
                    body.mount(_card(Static, item, selected=(i == self.cursor)))
            self.query_one("#header", Static).update(header)
            self.query_one("#footer", Label).update(
                "y accept · n reject · e correct · u undo · q quit"
            )
            self._scroll_to_cursor()

        def _scroll_to_cursor(self) -> None:
            cards = self.query(".card")
            if 0 <= self.cursor < len(cards):
                cards[self.cursor].scroll_visible(animate=False)

        # --- actions --------------------------------------------------------------------------

        def action_next(self) -> None:
            if self.items:
                self.cursor = min(self.cursor + 1, len(self.items) - 1)
                self._render()

        def action_prev(self) -> None:
            if self.items:
                self.cursor = max(self.cursor - 1, 0)
                self._render()

        def _apply(self, *, accept: bool, note: str = "") -> None:
            if not self.items:
                return
            item = self.items.pop(self.cursor)
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
            """Type a correction. It lands whether or not the item is then blessed.

            Same asymmetry the daily page enforces: his wording replaces the agent's rationale
            through `apply_owner_edit`, which carries a durable marker so a later agent pass
            cannot quietly overwrite it.
            """
            if self._editing or not self.items:
                return
            self._editing = True
            box = Input(placeholder="your wording (enter to apply and bless, esc to cancel)")
            self.query_one("#body", VerticalScroll).mount(box)
            box.focus()

        def on_input_submitted(self, event) -> None:
            self._editing = False
            event.input.remove()
            self._apply(accept=True, note=event.value)

        def action_undo(self) -> None:
            """Put the last decision back in the queue and reverse it.

            A single-key interface without an undo is a trap: `n` is next to `y`, and archiving an
            object he meant to bless would otherwise be unrecoverable from inside the app.
            """
            if not self.resolved:
                return
            item, _ = self.resolved.pop()
            if item.kind == Q.KIND_OBJECT:
                from locus.agent import state

                state.set_status(self.conn, item.ref, "proposed")
            self.items.insert(self.cursor, item)
            self._render()

    def _section_title(kind: str) -> str:
        return {
            Q.KIND_OBJECT: "Proposed — bless or drop",
            Q.KIND_ABANDONED: "Reading you have not touched",
        }.get(kind, kind)

    def _card(Static, item: Q.Decision, *, selected: bool):
        lines = [f"[b]{item.title}[/b]"]
        if item.detail:
            lines.append(item.detail)
        if item.grounding:
            lines.append(f"[dim]{item.grounding}[/dim]")
        lines.append(f"[dim]y = {item.accept_label}   ·   n = {item.reject_label}[/dim]")
        card = Static("\n".join(lines), classes="card selected" if selected else "card")
        return card

    return DecideApp()


def run(conn: sqlite3.Connection) -> list[tuple[Q.Decision, str]]:
    """Run the app to completion and return what was decided (for the CLI's summary)."""
    app = build_app(conn)
    app.run()
    return app.resolved
