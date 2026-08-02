"""Preview tomorrow's daily page from the LIVE database — no persist, no push, no device.

The safe way to look at the real page. `locus daily` would write `daily_pages`, `daily_anchors`
and the `daily_shown` ledger, which would mark everything as offered and change what tomorrow's
page shows; this composes and renders only.

    uv run python scripts/analysis/preview_daily_live.py
"""
from datetime import date
from pathlib import Path
from locus.agent import compose_daily as cd
from locus.db.connection import get_connection
from locus.config import load
from locus.reading.md2pdf import PageGeometry, render_markdown_to_pdf

cfg = load()
c = get_connection(cfg.paths.db)
page = cd.compose(c, today=date(2026, 8, 3))
print(f"readings={len(page.readings)} threads={len(page.threads)} recalls={len(page.recalls)}")
print("status:", page.status.render())
out = Path("eval-artifacts/daily-live-preview.pdf")
render_markdown_to_pdf(cd.render(page), out, geometry=PageGeometry(rule_gap_em=cfg.daily.rule_gap_em))
import pymupdf
d = pymupdf.open(out)
print("pages:", d.page_count)
for i in range(d.page_count):
    d[i].get_pixmap(dpi=100).save(f"/home/alec/.claude/jobs/f3b5108d/tmp/live-p{i+1}.png")
c.close()
