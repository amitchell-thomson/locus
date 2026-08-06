"""Measure the vertical gap between ruled writing lines on a rendered daily page.

WHY. The keep-this line — the last ruled line of a connection card, the one carrying the tick box
— sat 30.2pt below its neighbour on the 2026-08-06 delivered page while the other three rules sat
19.8pt apart, so the item with a decision on it visibly had different line spacing. The cause was
invisible in the source (Typst collapses `above` against the previous block's `below`, and the
tick box made the block ~10pt tall), and no test could see it: the markdown was correct either
way. Only the rendered geometry shows it, which is what this reads.

Run it after ANY change to `rule_gap_em`, `_keep_rule`, or how `_render_think` emits rules:

    uv run python scripts/analysis/daily_rule_spacing.py [path/to/page.pdf]

With no argument it renders a synthetic one-card page. Every gap between consecutive rules should
be one `rule_gap_em` (19.8pt at the 1.8em default); the keep-this rule is the short one (~72%
width) and must match the rest.
"""

from __future__ import annotations

import sys
from pathlib import Path

from locus.agent import compose_daily as cd
from locus.config import load
from locus.reading.md2pdf import PageGeometry, render_markdown_to_pdf


def rules_on_page(pdf_path: Path, page_no: int) -> list[tuple[float, float, float]]:
    """Horizontal strokes on one page as (y, x0, x1), top to bottom."""
    import fitz

    out: list[tuple[float, float, float]] = []
    with fitz.open(pdf_path) as doc:
        for drawing in doc[page_no].get_drawings():
            for item in drawing["items"]:
                if item[0] != "l":
                    continue
                p1, p2 = item[1], item[2]
                if abs(p1.y - p2.y) < 0.5 and abs(p2.x - p1.x) > 50:
                    out.append((round(p1.y, 1), round(p1.x, 1), round(p2.x, 1)))
    return sorted(out)


def _synthetic_pdf(out: Path) -> Path:
    """One connection card with a tick — the shape that exposed the bug."""
    item = cd.ThreadItem(
        anchor="C1", section=cd.SECTION_CONNECT, kind="connection",
        headline="A connection long enough to wrap onto a second line so the card is realistic.",
        context="a source · idea for a project", tick=True,
    )
    page = cd.DailyPage(page_date="synthetic", threads=[item])
    cfg = load()
    render_markdown_to_pdf(
        cd.render(page), out,
        geometry=PageGeometry(
            width_in=cfg.reading.page_width_in, height_in=cfg.reading.page_height_in,
            margin_in=cfg.reading.margin_in, font_pt=cfg.reading.font_pt,
            rule_gap_em=cfg.daily.rule_gap_em, accent=cfg.daily.accent,
            sans_font=cfg.daily.sans_font, running_header="synthetic",
        ),
    )
    return out


def main() -> None:
    if len(sys.argv) > 1:
        pdf, pages = Path(sys.argv[1]), None
    else:
        pdf, pages = _synthetic_pdf(Path("vault/notes/_generated/_rule_spacing.pdf")), [0]

    import fitz

    with fitz.open(pdf) as doc:
        pages = pages or list(range(doc.page_count))
    for page_no in pages:
        print(f"\n--- page {page_no + 1} ---")
        prev = None
        for y, x0, x1 in rules_on_page(pdf, page_no):
            gap = f"  gap={y - prev:6.1f}pt" if prev is not None else ""
            short = "  <- keep-this (short)" if x1 - x0 < 400 else ""
            print(f"y={y:7.1f}  len={x1 - x0:6.1f}{gap}{short}")
            prev = y


if __name__ == "__main__":
    main()
