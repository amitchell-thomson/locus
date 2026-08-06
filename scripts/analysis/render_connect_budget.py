"""Re-measure the Connect page's line budget with REAL redesigned prose (invariant §8).

The `_FIT` calibration note says two ~300-char connection items are "the tallest thing the
page can carry" — the redesigned writer targets 420 chars (hard cap 600), so the budget
must be re-measured by rendering a real PDF and counting pages, not asserted.

Composes the page from the post-e2e throwaway DB (real stored prose from the live Sonnet
run) and renders through the real toolchain. Prints the per-page section headings.

    uv run python scripts/analysis/render_connect_budget.py <e2e-db> <out-pdf>
"""
import sys
from datetime import date
from pathlib import Path

sys.path.insert(0, ".")
from locus.agent import compose_daily as cd
from locus.db.connection import get_connection
from locus.reading.md2pdf import PageGeometry, render_markdown_to_pdf

DB = sys.argv[1]
OUT = Path(sys.argv[2] if len(sys.argv) > 2 else "/tmp/connect-budget.pdf")

conn = get_connection(DB)
page = cd.compose(conn, today=date(2026, 8, 7))
md = cd.render(page)

connect_start = md.find("# Connect")
next_h1 = md.find("\n# ", connect_start + 1)
section = md[connect_start: next_h1 if next_h1 != -1 else None]
print(f"Connect section: {len(section)} chars, {section.count('(C')} anchors")
print(section[:400])

render_markdown_to_pdf(md, OUT, geometry=PageGeometry())
print(f"\nrendered -> {OUT}")

try:
    from pypdf import PdfReader

    n = len(PdfReader(str(OUT)).pages)
except ImportError:
    import subprocess

    n = None
    out = subprocess.run(["pdfinfo", str(OUT)], capture_output=True, text=True)
    for line in out.stdout.splitlines():
        if line.startswith("Pages:"):
            n = int(line.split()[-1])
print(f"PAGES: {n}")
print("page-break count in markdown:", md.count("#pagebreak"))
