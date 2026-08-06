"""Does the CONNECT section alone fit ONE page? (invariant §8, isolated measurement).

Renders ONLY the Connect section markdown as its own document with the production
geometry and counts pages — total-document page counts confound Connect with whatever the
other sections are doing (clearing daily_shown for a worst-case test also changes Ideas).

argv[1] = DB · argv[2..] ignored. Prints CONNECT_PAGES: n.
"""
import sys
import tempfile
from datetime import date
from pathlib import Path

sys.path.insert(0, ".")
from locus.agent import compose_daily as cd
from locus.db.connection import get_connection
from locus.reading.md2pdf import PageGeometry, render_markdown_to_pdf

conn = get_connection(sys.argv[1])
page = cd.compose(conn, today=date(2026, 8, 7))
md = cd.render(page)

# The renderer joins sections with an explicit pagebreak block; split on it and keep Connect.
parts = md.split("```{=typst}\n#pagebreak()\n```")
connect = next((p for p in parts if "# Consider" in p), None)
assert connect is not None, "no Connect section rendered"
n_items = connect.count("`#anc[")
print(f"connect items: {n_items}, section chars: {len(connect)}")

out = Path(tempfile.mkdtemp()) / "connect-only.pdf"
render_markdown_to_pdf(connect, out, geometry=PageGeometry())
from pypdf import PdfReader

print(f"CONNECT_PAGES: {len(PdfReader(str(out)).pages)}")
